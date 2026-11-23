"""Exercise hc disconnects and identity checks at the CLI and stdio MCP boundaries."""
import json
from pathlib import Path
import subprocess
import unittest

import test_control as fixture
from mcp_client import Client

# Each connect pins one peer, as the real core does. Faults can drop the
# acknowledgement after spawning, or disconnect before any reply is visible.
FAKE_HC = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root = Path(os.environ['CONTROL_PLANE_ROOT'])
peer = (root / 'core').read_text()
state = root / (peer + '.json')
status = {'core': {'id': peer}}
if sys.argv[1:] == ['--json', 'server', 'start']:
    print(json.dumps(status))
    sys.exit()
assert sys.argv[1:] == ['connect', '--session', 'workers']
for line in sys.stdin:
    req = json.loads(line)
    method, p = req['method'], req['params']
    reply = {'jsonrpc': '2.0', 'id': req['id']}
    fault = (root / 'fault').read_text()
    t = json.loads(state.read_text()) if state.exists() else None
    if method == 'server.status':
        result = status
    elif method == 'terminal.run':
        with (root / 'spawns').open('a') as f:
            print(p['name'], file=f)
        if fault == 'before-run':
            sys.exit(1)
        t = dict(address='t1', created_ms=123, state='live', status={'state':'working'}, **p)
        state.write_text(json.dumps(t))
        if fault in ('lost-reply', 'disconnected'):
            sys.exit(1)
        result = {'address':'t1'}
    elif method == 'terminal.get':
        if fault == 'disconnected': sys.exit(1)
        if t is None or p['terminal'] not in (t['name'],t['address']):
            reply['error'] = {'code': -32000,'data': {'kind':'not_found'}}
        result = t
    elif method == 'terminal.list':
        result = {'terminals': [t, {'parent':'t1', 'die_with_parent':True}] if fault=='child' else [t]}
    elif method == 'terminal.kill':
        assert t['address'] == p['terminal']
        t['state'] = 'finished'
        state.write_text(json.dumps(t))
        result = t
    else:
        raise AssertionError(method)
    if 'error' not in reply: reply['result'] = result
    print(json.dumps(reply), flush=True)
'''


class CoreTransportTests(unittest.TestCase):
    cli = fixture.ControlTests.cli
    create = fixture.ControlTests.create

    def wait_run(self, key, state='running'):
        run = fixture.ControlTests.wait_run(self, key, state)
        # The launcher writes its outcome before releasing its lease.
        fixture.ControlTests.wait_launcher(self, key)
        return run

    def setUp(self):
        fixture.ControlTests.setUp(self)
        self.config = Path(self.env['CONTROL_PLANE_CONFIG'])
        self.settings = json.loads(self.config.read_text())
        self.adapter('hc-codex')
        hc = self.root / 'bin/hc'
        hc.write_text(FAKE_HC)
        hc.chmod(0o755)
        (self.root / 'core').write_text('original')
        self.fault = self.root / 'fault'
        self.fault.write_text('')
        bindings = self.root / 'bindings'
        bindings.mkdir(exist_ok=True)
        (bindings / 'runtime.json').write_text(json.dumps({'hc_session':'workers'}))
        self.env['HERDR_ENV'] = ''
        self.create()

    def adapter(self, name):
        self.settings['adapter'] = name
        self.config.write_text(json.dumps(self.settings))

    def test_lost_ack_rollback_and_foreign_terminal(self):
        self.fault.write_text('lost-reply')
        run = self.cli('start', 'task1')['run']
        self.wait_run(run)
        client = Client(fixture.BINARY, env=self.env)
        self.addCleanup(client.close)
        current, failed = client.call('run', {'id':run,'compact':True})
        self.assertFalse(failed)
        self.assertEqual(current['hc']['core_id'], 'original')
        self.assertEqual(current['hc']['address'], 't1')
        self.assertEqual(current['hc']['attach']['argv'][-2:], ['attach', 'original/t1'])
        self.assertEqual(current['hc']['agent_status']['state'], 'working')
        self.assertEqual(self.cli('open', 'task1')[0]['run'], run)
        self.adapter('herdr-codex')
        self.assertEqual(self.cli('open', 'task1')[0]['action'], 'existing')
        other = self.root / 'other.json'
        state = self.root / 'original.json'
        other.write_bytes(state.read_bytes())
        (self.root / 'core').write_text('other')
        self.assertEqual(self.cli('open', 'task1', ok=False)[0]['action'], 'error')
        self.cli('complete', 'task1', '--status', 'cancelled', ok=False)
        self.assertEqual(json.loads(other.read_text())['state'], 'live')
        (self.root / 'core').write_text('original')
        v = json.loads(state.read_text())
        for field, value in [('created_ms', 456), ('argv', ['/bin/sh']), ('labels', {})]:
            with self.subTest(field=field):
                state.write_text(json.dumps({**v, field:value}))
                self.cli('complete', 'task1', '--status', 'cancelled', ok=False)
                self.assertEqual(self.cli('show', 'tasks', 'task1')['status'], 'running')
        state.write_text(json.dumps(v))
        self.fault.write_text('child')
        self.cli('complete', 'task1', '--status', 'cancelled', ok=False)
        self.fault.write_text('')
        self.assertEqual(self.cli('complete', 'task1', '--status', 'cancelled')['cleanup']['status'], 'closed')
        self.assertEqual(json.loads(other.read_text())['state'], 'live')
        self.assertEqual(len((self.root / 'spawns').read_text().splitlines()), 1)

    def test_disconnected_launcher_recovered_from_journal(self):
        self.fault.write_text('disconnected')
        run = self.cli('start', 'task1')['run']
        self.wait_run(run, 'launch_failed')
        self.cli('open', 'task1', ok=False)
        self.fault.write_text('')
        reopened = self.cli('open', 'task1')[0]
        self.assertEqual((reopened['run'], reopened['action']), (run, 'recovered'))
        self.assertEqual(self.cli('run', run)['pane'], 't1')
        self.assertEqual(len((self.root / 'spawns').read_text().splitlines()), 1)

    def test_absence_after_unacknowledged_request_never_replays(self):
        self.fault.write_text('before-run')
        run = self.cli('start', 'task1')['run']
        self.wait_run(run, 'launch_failed')
        self.cli('open', 'task1', ok=False)
        self.cli('complete', 'task1', '--status', 'cancelled', ok=False)
        self.assertEqual(len((self.root / 'spawns').read_text().splitlines()), 1)

    def test_mixed_runs_after_switch_and_concurrent_open(self):
        self.adapter('herdr-codex')
        self.env['HERDR_ENV'] = '1'
        self.create('legacy')
        old = self.cli('start', 'legacy')['run']
        self.wait_run(old)
        self.adapter('hc-codex')
        self.env['FAKE_LIVE'] = '1'
        self.env['FAKE_ASSIGNMENT'] = str(self.root / 'runs' / old / 'assignment.md')
        # A saved legacy run must still probe its original transport.
        self.assertEqual(self.cli('open', 'legacy')[0]['run'], old)
        procs = [subprocess.Popen([str(fixture.BINARY),'open','task1'],env=self.env,
                                  stdout=subprocess.PIPE, text=True) for _ in range(2)]
        runs = [json.loads(p.communicate(timeout=10)[0])[0]['run'] for p in procs]
        self.assertEqual(runs[0], runs[1])
        self.wait_run(runs[0])
        self.assertEqual(len((self.root / 'spawns').read_text().splitlines()), 1)

    def test_proven_exit_reopens_on_selected_transport_and_preserves_history(self):
        run = self.cli('start', 'task1')['run']
        self.wait_run(run)
        state = self.root / 'original.json'
        v = json.loads(state.read_text())
        state.write_text(json.dumps({**v, 'state':'finished'}))
        self.adapter('herdr-codex')
        self.env['HERDR_ENV'] = '1'
        replacement = self.cli('open', 'task1')[0]['run']
        self.assertNotEqual(replacement, run)
        saved = self.wait_run(replacement)
        self.assertEqual(saved['previous_run'], run)
        self.assertEqual(saved['adapter'], 'herdr-codex')
        self.assertEqual(self.cli('run', run)['status'], 'interrupted')
