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
document = root / (peer + '-desktop.json')
status = {'core': {'id': peer}}
fault = (root / 'fault').read_text()
if sys.argv[1:] == ['--json', 'server', 'start']:
    if fault == 'core-lock':
        print(json.dumps({'error': {'kind': 'unavailable',
              'message': 'cannot open core lock: Read-only file system (os error 30)'}}), file=sys.stderr)
        sys.exit(5)
    print(json.dumps(status))
    sys.exit()
assert sys.argv[1:] == ['connect', '--session', 'workers']
connection_errors = {'socket-denied': 'Permission denied (os error 13)',
                     'socket-timeout': 'core did not stop or start within 15 s'}
if fault in connection_errors:
    print(json.dumps({'error': {'kind': 'unavailable', 'message': connection_errors[fault]}}), file=sys.stderr)
    sys.exit(5)
for line in sys.stdin:
    req = json.loads(line)
    method, p = req['method'], req['params']
    reply = {'jsonrpc': '2.0', 'id': req['id']}
    fault = (root / 'fault').read_text()
    paths = [state, *root.glob(peer + '-t*.json')]
    terminals = {path: json.loads(path.read_text()) for path in paths if path.exists()}
    selected = next(((path, t) for path, t in terminals.items()
                     if p.get('terminal') in (t['name'], t['address'])), (None, None))
    path, t = selected
    if method == 'server.status':
        result = status
    elif method == 'terminal.run':
        with (root / 'spawns').open('a') as f:
            print(p['name'], file=f)
        if fault == 'before-run':
            sys.exit(1)
        address = f't{len(terminals) + 1}'
        path = state if address == 't1' else root / f'{peer}-{address}.json'
        t = dict(address=address, created_ms=123, state='live', status={'state':'working'}, **p)
        path.write_text(json.dumps(t))
        if fault in ('lost-reply', 'disconnected'):
            sys.exit(1)
        result = {'address':address}
    elif method == 'terminal.get':
        if fault == 'disconnected': sys.exit(1)
        if t is None:
            reply['error'] = {'code': -32000,'data': {'kind':'not_found'}}
        result = t
    elif method == 'terminal.list':
        members = list(terminals.values())
        if fault == 'child':
            members.append({'parent':'t1', 'die_with_parent':True})
        result = {'terminals': members}
    elif method == 'terminal.kill':
        assert t['address'] == p['terminal']
        t['state'] = 'finished'
        path.write_text(json.dumps(t))
        result = t
    elif method == 'doc.get':
        assert p == {'name':'herdr-app'}
        result = json.loads(document.read_text()) if document.exists() else {'value':None, 'version':0}
        if fault == 'doc-unavailable':
            reply['error'] = {'data':{'kind':'unavailable'}}
    elif method == 'doc.set':
        assert p['name'] == 'herdr-app'
        if fault == 'doc-race' and not document.exists():
            # A viewer adopted the terminal after doc.get and before doc.set.
            value = p['value']
            value['spaces'][0]['name'] = 'Viewer space name'
            value['spaces'][0]['tabs'][0]['name'] = ''
            value['spaces'][0]['viewer_extra'] = {'keep': True}
            document.write_text(json.dumps({'value':value, 'version':1}))
            reply['error'] = {'data':{'kind':'conflict'}}
        else:
            version = json.loads(document.read_text())['version'] if document.exists() else 0
            assert p['version'] == version
            result = {'version':version + 1}
            document.write_text(json.dumps({**result, 'value':p['value']}))
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

    def desktop(self):
        return json.loads((self.root / 'original-desktop.json').read_text())

    def test_layout_groups_by_first_repository_and_keeps_tabs_separate(self):
        repos = [self.root / parent / 'repo' for parent in ('first', 'second')]
        for repo in repos:
            repo.mkdir(parents=True)
            fixture.ControlTests.git(self, repo, 'init', '-b', 'main')
            fixture.ControlTests.git(self, repo, '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                                     'commit', '--allow-empty', '-m', 'init')
        alias = self.root / 'alias'
        alias.symlink_to(repos[0], target_is_directory=True)
        self.create('first', '--code', '--repo', str(repos[0]), '--repo', str(repos[1]))
        self.create('same', '--code', '--repo', str(alias))
        self.create('second', '--code', '--repo', str(repos[1]))
        self.cli('task-set', 'second', '--title', '  Другой репозиторий  ')
        self.cli('task-set', 'task1', '--title', '   ')
        ids = ['first', 'same', 'second', 'task1']
        titles = ['Проверка', 'Проверка', 'Другой репозиторий', 'task1']
        opened = self.cli('open', *ids)
        runs = [self.wait_run(item['run']) for item in opened]
        first, same, second, no_code = runs
        self.assertEqual(first['cwd'], first['worktrees'][0]['path'])
        self.assertEqual(first['workspace'], same['workspace'])
        self.assertNotEqual(first['workspace'], second['workspace'])
        self.assertTrue(first['workspace'].startswith('repo-repo-'))
        self.assertTrue(second['workspace'].startswith('repo-repo-'))
        self.assertEqual(no_code['workspace'], 'workflow-lab')
        self.assertEqual(len({run['pane'] for run in runs}), 4)
        self.assertEqual(len({run['tab'] for run in runs}), 4)
        spaces = {space['id']: space for space in self.desktop()['value']['spaces']}
        self.assertEqual(len(spaces), 3)
        self.assertEqual(spaces[first['workspace']]['folder'], str(repos[0]))
        self.assertEqual(spaces[second['workspace']]['folder'], str(repos[1]))
        self.assertNotEqual(spaces[first['workspace']]['name'], spaces[second['workspace']]['name'])
        self.assertEqual(spaces[no_code['workspace']]['name'], 'lab')
        for task, title, run in zip(ids, titles, runs):
            with self.subTest(task=task):
                suffix = '' if run['pane'] == 't1' else f"-{run['pane']}"
                terminal = json.loads((self.root / f'original{suffix}.json').read_text())
                labels = terminal['labels']
                compact = self.cli('run', run['id'], '--compact')['hc']
                self.assertEqual(compact, self.cli('open', task)[0]['hc'])
                self.assertEqual(labels['herdr.space'], compact['space'])
                self.assertEqual(labels['herdr.tab'], compact['tab'])
                self.assertEqual(labels['workflow.run'], run['id'])
                self.assertEqual(compact['attach']['argv'][-1], f"original/{run['pane']}")
                tab = next(t for t in spaces[compact['space']]['tabs'] if t['id'] == compact['tab'])
                self.assertEqual(tab['name'], title)
                self.assertEqual(tab['root']['children'][0]['content']['terminal'], run['pane'])
        self.assertEqual(len((self.root / 'spawns').read_text().splitlines()), 4)
        self.assertEqual(self.cli('complete', 'first', '--status', 'cancelled')['cleanup']['status'], 'closed')
        remaining = {space['id']: space for space in self.desktop()['value']['spaces']}
        self.assertEqual([t['id'] for t in remaining[first['workspace']]['tabs']], [same['tab']])
        self.assertEqual(remaining[second['workspace']], spaces[second['workspace']])
        for task in ids[1:]:
            self.assertEqual(self.cli('complete', task, '--status', 'cancelled')['cleanup']['status'], 'closed')
        self.assertEqual(self.desktop()['value']['spaces'], [])

    def test_desktop_concurrent_adoption_preserves_viewer_changes(self):
        self.fault.write_text('doc-race')
        run = self.wait_run(self.cli('start', 'task1')['run'])
        document = self.desktop()
        self.assertEqual(document['version'], 2)
        space, = document['value']['spaces']
        self.assertEqual(space['name'], 'Viewer space name')
        self.assertEqual(space['viewer_extra'], {'keep': True})
        tab, = space['tabs']
        self.assertEqual((tab['id'], tab['name']), (run['tab'], 'Проверка'))
        self.cli('open', 'task1')
        self.assertEqual(self.desktop(), document)

    def test_desktop_failure_preserves_running_terminal_and_recovers_on_open(self):
        self.fault.write_text('doc-unavailable')
        run = self.wait_run(self.cli('start', 'task1')['run'])
        info = self.cli('open', 'task1')[0]['hc']
        self.assertEqual(info, self.cli('run', run['id'], '--compact')['hc'])
        self.assertIn('doc.get', info['desktop_layout_error'])
        self.assertEqual(info['attach']['argv'][-1], 'original/t1')
        self.fault.write_text('')
        reopened = self.cli('open', 'task1')[0]
        self.assertEqual((reopened['action'], reopened['run']), ('existing', run['id']))
        self.assertNotIn('desktop_layout_error', reopened['hc'])
        self.assertEqual(reopened['hc']['attach'], info['attach'])
        tab, = self.desktop()['value']['spaces'][0]['tabs']
        self.assertEqual((tab['id'], tab['name']), (run['tab'], 'Проверка'))
        self.assertEqual(len((self.root / 'spawns').read_text().splitlines()), 1)

    def test_desktop_repair_uses_launch_title_after_task_rename(self):
        self.cli('task-set', 'task1', '--agent-review-policy', 'off')
        self.fault.write_text('doc-unavailable')
        run = self.wait_run(self.cli('start', 'task1')['run'])
        report = self.root / 'report.md'
        report.write_text('Результат')
        self.cli('finish', '--run', run['id'], '--report', str(report))
        self.cli('task-set', 'task1', '--title', 'Новое название')
        self.fault.write_text('')
        result = self.cli('open', 'task1')[0]
        self.assertEqual(result['action'], 'result')
        self.assertNotIn('desktop_layout_error', result['hc'])
        tab, = self.desktop()['value']['spaces'][0]['tabs']
        self.assertEqual((tab['id'], tab['name']), (run['tab'], 'Проверка'))

    def test_desktop_repair_accepts_run_without_saved_tab_name(self):
        self.fault.write_text('doc-unavailable')
        run = self.wait_run(self.cli('start', 'task1')['run'])
        run['hc_desktop'].pop('tab_name')
        (self.root / 'runs' / run['id'] / 'state.json').write_text(json.dumps(run))
        self.fault.write_text('')
        result = self.cli('open', 'task1')[0]
        self.assertEqual((result['action'], result['run']), ('existing', run['id']))
        tab, = self.desktop()['value']['spaces'][0]['tabs']
        self.assertEqual((tab['id'], tab['name']), (run['tab'], 'task1'))

    def test_desktop_repair_and_cleanup_keep_moved_user_tab(self):
        self.fault.write_text('doc-unavailable')
        run = self.wait_run(self.cli('start', 'task1')['run'])
        document = {'version':1, 'value':{'spaces':[{
            'id':'user-space', 'name':'User space', 'tabs':[{
                'id':'user-tab', 'name':'User tab', 'root':{'type':'block', 'content':{'terminal':run['pane']}}
            }]
        }]}}
        (self.root / 'original-desktop.json').write_text(json.dumps(document))
        self.fault.write_text('')
        self.assertNotIn('desktop_layout_error', self.cli('open', 'task1')[0]['hc'])
        self.assertEqual(self.desktop(), document)
        self.assertEqual(self.cli('complete', 'task1', '--status', 'cancelled')['cleanup']['status'], 'closed')
        self.assertEqual(self.desktop(), document)

    def test_desktop_cleanup_retries_without_closing_a_reused_terminal(self):
        self.wait_run(self.cli('start', 'task1')['run'])
        self.fault.write_text('doc-unavailable')
        closed = self.cli('complete', 'task1', '--status', 'cancelled')['cleanup']
        self.assertEqual(closed['status'], 'closed')
        self.assertIn('doc.get', closed['desktop_layout_error'])
        self.assertEqual(len(self.desktop()['value']['spaces']), 1)
        path = self.root / 'original.json'
        replacement = {**json.loads(path.read_text()), 'name':'foreign', 'state':'live', 'labels':{}}
        path.write_text(json.dumps(replacement))
        self.fault.write_text('')
        closed = self.cli('complete', 'task1', '--status', 'cancelled')['cleanup']
        self.assertTrue(closed['desktop_layout_done'])
        self.assertIsNone(closed['desktop_layout_error'])
        self.assertEqual(self.desktop()['value']['spaces'], [])
        self.assertEqual(json.loads(path.read_text()), replacement)

    def test_existing_hc_record_keeps_its_old_labels(self):
        run = self.wait_run(self.cli('start', 'task1')['run'])
        run.pop('hc_desktop')
        run['tab'] = 'workflow-' + run['id']
        record = self.root / 'runs' / run['id'] / 'state.json'
        record.write_text(json.dumps(run))
        path = self.root / 'original.json'
        terminal = json.loads(path.read_text())
        terminal['labels']['herdr.tab'] = run['tab']
        path.write_text(json.dumps(terminal))
        old = self.cli('run', run['id'], '--compact')['hc']
        self.assertEqual((old['space'], old['tab']), ('workflow-lab', run['tab']))
        self.assertEqual(old['attach']['argv'][-1], 'original/t1')
        self.assertEqual(self.cli('open', 'task1')[0]['hc'], old)
        self.assertEqual(json.loads(path.read_text()), terminal)
        self.assertEqual(self.cli('complete', 'task1', '--status', 'cancelled')['cleanup']['status'], 'closed')
        self.assertEqual(json.loads(path.read_text())['labels'], terminal['labels'])

    def test_open_denied_access_has_retry_without_recording_a_run(self):
        faults = [('core-lock', 'hc_access_denied', 'cannot open core lock'),
                  ('socket-denied', 'hc_access_denied', 'Permission denied'),
                  ('socket-timeout', 'hc_unavailable', 'core did not stop or start')]
        for fault, code, reason in faults:
            with self.subTest(fault=fault):
                self.fault.write_text(fault)
                err = self.cli('open', 'task1', ok=False)[0]
                self.assertEqual((err['action'], err['error_code']), ('error', code))
                self.assertIn(reason, err['error'])
                self.assertIn('open task1', err['next_action'])
                self.assertIn('--state-root', err['next_action'])
                self.assertNotIn('run', err)
                self.assertIsNone(self.cli('show', 'tasks', 'task1')['active_run'])
                self.assertFalse((self.root / 'runs').exists())
                self.assertFalse((self.root / 'spawns').exists())

    def test_denied_probe_preserves_running_terminal_and_saved_state(self):
        opened = self.cli('open', 'task1')[0]
        self.assertEqual(opened['action'], 'preparing')
        run = opened['run']
        self.wait_run(run)
        saved = self.root / 'runs' / run / 'state.json'
        original = saved.read_bytes()
        for fault, code in [('socket-denied', 'hc_access_denied'), ('socket-timeout', 'hc_unavailable')]:
            with self.subTest(fault=fault):
                self.fault.write_text(fault)
                err = self.cli('open', 'task1', ok=False)[0]
                view = self.cli('run', run, '--compact')
                self.assertEqual((err['error_code'], err['run_status']), (code, 'running'))
                self.assertEqual((view['status'], view['hc']['terminal_state']), ('running', 'unknown'))
                self.assertIsNone(view['startup_error_kind'])
                self.assertNotIn('attach', view['hc'])
                self.assertIn('open task1', err['next_action'])
                self.assertEqual(err['hc']['terminal_state'], 'unknown')
                self.assertEqual(view['hc']['error_code'], code)
                self.assertIn(f'run {run} --compact', view['hc']['next_action'])
                self.assertEqual(saved.read_bytes(), original)
        self.fault.write_text('')
        reopened = self.cli('open', 'task1')[0]
        self.assertEqual((reopened['run'], reopened['action']), (run, 'existing'))
        self.assertEqual(reopened['hc']['attach']['argv'][-1], 'original/t1')
        self.assertEqual(len((self.root / 'spawns').read_text().splitlines()), 1)

    def test_live_terminal_is_available_without_tmpdir(self):
        run = self.cli('open', 'task1')[0]['run']
        self.wait_run(run)
        env = {**self.env, 'TMPDIR': str(self.root / 'missing-tmpdir')}
        opened = self.cli('open', 'task1', env=env)[0]
        self.assertEqual((opened['run'], opened['action']), (run, 'existing'))
        compact = self.cli('run', run, '--compact', env=env)
        self.assertEqual(compact['hc'], opened['hc'])
        self.assertEqual(compact['hc']['terminal_state'], 'live')
        self.fault.write_text('socket-denied')
        denied = self.cli('open', 'task1', env=env, ok=False)[0]
        self.assertEqual(denied['error_code'], 'hc_access_denied')
        self.assertIn('open task1', denied['next_action'])
        compact = self.cli('run', run, '--compact', env=env)
        self.assertEqual(compact['hc']['error_code'], 'hc_access_denied')
        self.assertIn(f'run {run} --compact', compact['hc']['next_action'])

    def test_open_result_keeps_status_and_shows_denied_probe_retry(self):
        self.cli('task-set', 'task1', '--agent-review-policy', 'off')
        run = self.cli('open', 'task1')[0]['run']
        self.wait_run(run)
        report = self.root / 'report.md'
        report.write_text('Проверка доступа')
        self.cli('finish', '--run', run, '--report', str(report))
        self.fault.write_text('socket-denied')
        result = self.cli('open', 'task1')[0]
        self.assertEqual((result['action'], result['status']), ('result', 'review'))
        self.assertEqual(result['hc']['error_code'], 'hc_access_denied')
        self.assertEqual(result['hc']['terminal_state'], 'unknown')
        self.assertIsNone(result['startup_error_kind'])
        self.assertIn(f'run {run} --compact', result['next_action'])

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
        prior = self.cli('run', run)
        self.assertEqual(prior['pane'], 't1')
        self.assertEqual((reopened['hc']['space'], reopened['hc']['tab']), (prior['workspace'], prior['tab']))
        tab, = self.desktop()['value']['spaces'][0]['tabs']
        self.assertEqual((tab['id'], tab['name']), (prior['tab'], 'Проверка'))
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
