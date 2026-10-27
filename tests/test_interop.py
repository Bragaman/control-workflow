"""Native multi-process files, locks, recovery and persisted-format compatibility."""
import fcntl
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import math
import random
import struct
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from urllib.parse import unquote, urlsplit
import test_control as fixture
from mcp_client import Client

BINARY = fixture.BINARY
SOURCE = Path(__file__).resolve().parents[1]


class InteropTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='workflow-interop-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.state = self.root / 'state'
        self.bindings = self.root / 'bindings'
        self.config = self.root / 'config.json'
        self.config.write_text('{}')
        self.args = ['--state-root', str(self.state), '--bindings-root', str(self.bindings), '--config', str(self.config)]
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('CONTROL_PLANE_', 'HERDR_'))}
        (self.root / 'bin').mkdir()
        fake = self.root / 'bin/herdr'
        fake.write_text(fixture.FAKE_HERDR)
        fake.chmod(0o755)
        self.log = self.root / 'calls.jsonl'
        self.env.update(HERDR_ENV='1', HERDR_SOCKET_PATH=str(self.log) + '.sock', FAKE_LOG=str(self.log),
                        PATH=str(fake.parent) + os.pathsep + os.environ['PATH'], PYTHONDONTWRITEBYTECODE='1')
        self.cli('project-create', 'demo', '--name', 'Demo')

    def argv(self, *args):
        return [str(BINARY)] + self.args + list(args)

    def cli(self, *args):
        p = subprocess.run(self.argv(*args), env=self.env, capture_output=True, text=True, timeout=15)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        return json.loads(p.stdout)

    def run_ready(self):
        self.cli('task-create', 'one', '--project', 'demo', '--title', 'Interop', '--goal', 'Verify', '--acceptance', 'Contracts pass')
        r = self.cli('start', 'one')['run']
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            state = self.cli('run', r)
            if state['status'] == 'running' and not state['launcher_alive']:
                return state
            time.sleep(.02)
        self.fail(state)

    def test_native_processes_obey_same_flock(self):
        self.state.mkdir(exist_ok=True)
        with (self.state / '.control.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            processes = [subprocess.Popen(self.argv('project-create', name, '--name', name),
                                          env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                         for name in ('first', 'second')]
            try:
                time.sleep(.15)
                self.assertTrue(all(p.poll() is None for p in processes))
                self.assertFalse((self.state / 'projects/second.json').exists())
                self.assertFalse((self.state / 'projects/first.json').exists())
            finally:
                fcntl.flock(lock, fcntl.LOCK_UN)
            for p in processes:
                out, err = p.communicate(timeout=10)
                self.assertEqual(p.returncode, 0, out + err)
        self.assertEqual(len(self.cli('projects')), 3)

    def test_concurrent_cli_open_claims_exactly_one_run(self):
        self.cli('task-create', 'one', '--project', 'demo', '--title', 'Interop', '--goal', 'Verify', '--acceptance', 'Pass')
        processes = [subprocess.Popen(self.argv('open', 'one'), env=self.env, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True) for _ in range(2)]
        results = []
        for p in processes:
            out, err = p.communicate(timeout=10)
            self.assertEqual(p.returncode, 0, out + err)
            results.append(json.loads(out)[0])
        self.assertEqual(results[0]['run'], results[1]['run'])
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            r = self.cli('run', results[0]['run'])
            if not r['launcher_alive']:
                break
            time.sleep(.02)
        self.assertEqual(r['status'], 'running')
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual(sum(c[:2] == ['agent', 'start'] or c[2:4] == ['agent', 'start'] for c in calls), 1)

    def test_concurrent_mcp_servers_claim_one_executor(self):
        self.cli('task-create', 'one', '--project', 'demo', '--title', 'Concurrent MCP',
                 '--goal', 'One run', '--acceptance', 'One executor')
        clients = [Client(Path(BINARY).resolve(), self.args, self.env) for _ in range(2)]
        for client in clients:
            self.addCleanup(client.close)
        with ThreadPoolExecutor(max_workers=2) as pool:
            futures = [pool.submit(client.call, 'open', {'ids': ['one']}) for client in clients]
            results = [f.result(timeout=15) for f in futures]
        self.assertTrue(all(not error for _, error in results), results)
        self.assertEqual(results[0][0][0]['run'], results[1][0][0]['run'])
        run = results[0][0][0]['run']
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            state = self.cli('run', run)
            if state['status'] == 'running' and not state['launcher_alive']:
                break
            time.sleep(.02)
        self.assertEqual(state['status'], 'running')
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual(sum(c[:2] == ['agent', 'start'] or c[2:4] == ['agent', 'start'] for c in calls), 1)

    def test_snapshots_unknown_values_and_review_across_processes(self):
        r = self.run_ready()
        project_file = self.state / 'projects/demo.json'
        p = json.loads(project_file.read_text())
        p['future'] = {'ratio': 1e-6, 'count': 123456789012345678901234567890, 'extra': 'Straße Σς'}
        project_file.write_text(json.dumps(p))
        self.cli('project-set', 'demo', '--description', 'kept')
        self.assertEqual(self.cli('show', 'projects', 'demo')['future'], p['future'])
        report = self.root / 'report.md'
        report.write_text('Проверено [данные](data.txt).')
        self.cli('finish', '--run', r['id'], '--report', str(report))
        attempt = self.cli('review-request', '--run', r['id'])
        snap = json.loads(Path(attempt['snapshot']).read_text())
        expected = hashlib.sha256(json.dumps(snap['manifest'], sort_keys=True, ensure_ascii=False).encode()).hexdigest()
        self.assertEqual(attempt['version'], expected)
        self.cli('review-read', '--run', r['id'], '--id', attempt['id'], '--section', 'manifest')
        self.cli('review-claim', '--run', r['id'], '--id', attempt['id'], '--actor', 'reviewer')
        verdict = self.root / 'verdict.json'
        verdict.write_text(json.dumps(dict(version=attempt['version'], status='passed', conclusion='Checked',
                                          findings=[], suggestions=[], evidence=['Interop'], limitations=[])))
        self.cli('review-conclude', '--run', r['id'], '--id', attempt['id'], '--actor', 'reviewer', '--result', str(verdict))
        self.assertEqual(self.cli('review-show', '--run', r['id'])['attempts'][-1]['status'], 'passed')

    def test_feedback_reservation_repairs_interrupted_paired_writes(self):
        r = self.run_ready()
        report = self.root / 'report.md'; report.write_text('Checked')
        self.cli('finish', '--run', r['id'], '--report', str(report))
        a = self.cli('review-request', '--run', r['id'])
        self.cli('review-claim', '--run', r['id'], '--id', a['id'], '--actor', 'tester')
        verdict = self.root / 'verdict.json'
        verdict.write_text(json.dumps(dict(version=a['version'], status='changes_requested', conclusion='Fix',
                                          findings=['Fix error'], suggestions=[], evidence=['Reproduced'], limitations=[])))
        self.cli('review-conclude', '--run', r['id'], '--id', a['id'], '--actor', 'tester', '--result', str(verdict))
        self.cli('review-feedback', '--run', r['id'], '--id', a['id'])
        task_file = self.state / 'tasks/one.json'
        task = json.loads(task_file.read_text()); task['status'] = 'review'; task_file.write_text(json.dumps(task))
        # Model a crash after the run write but before the task write; retry repairs it.
        self.cli('review-feedback', '--run', r['id'], '--id', a['id'])
        self.assertEqual(self.cli('show', 'tasks', 'one')['status'], 'running')
        self.assertEqual(self.cli('run', r['id'])['status'], 'running')
        self.assertEqual(len(self.cli('review-show', '--run', r['id'])['attempts']), 1)

    def test_warm_mcp_reads_external_updates_and_does_not_invoke_python(self):
        with_client = Client(Path(BINARY).resolve(), self.args, self.env)
        self.addCleanup(with_client.close)
        self.assertEqual(with_client.call('projects', {})[0][0]['name'], 'Demo')
        self.cli('project-set', 'demo', '--name', 'External')
        self.assertEqual(with_client.call('projects', {})[0][0]['name'], 'External')
        env = {**self.env, 'PATH': '/does-not-exist'}
        client = Client(Path(BINARY).resolve(), self.args, env)
        self.addCleanup(client.close)
        self.assertEqual(client.call('memory-add', {'project': 'demo', 'title': 'Native', 'text': 'Straße'})[1], False)
        self.assertEqual(client.call('memory-list', {'project': 'demo', 'query': 'STRASSE', 'limit': 10})[0]['total'], 1)

    def test_halfway_snapshot_can_cross_cli_and_mcp(self):
        run = self.run_ready()
        project_file = self.state / 'projects/demo.json'
        project = json.loads(project_file.read_text())
        project['future_ratio'] = [1461205785224690.2, -1250201187231379.2, 11949781134863.562]
        project_file.write_text(json.dumps(project))
        report = self.root / 'report.md'; report.write_text('Checked')
        self.cli('finish', '--run', run['id'], '--report', str(report))
        attempt = self.cli('review-request', '--run', run['id'])
        self.cli('review-read', '--run', run['id'], '--id', attempt['id'], '--section', 'manifest')
        client = Client(Path(BINARY).resolve(), self.args, self.env)
        self.addCleanup(client.close)
        frozen, failed = client.call('review-read', {'run': run['id'], 'id': attempt['id'], 'section': 'manifest'})
        self.assertFalse(failed, frozen)
        self.assertEqual(frozen['version'], attempt['version'])
        project['future_ratio'].append(1465010394193143.2)
        project_file.write_text(json.dumps(project))
        native = self.cli('review-request', '--run', run['id'])
        self.assertNotEqual(native['version'], attempt['version'])
        self.cli('review-read', '--run', run['id'], '--id', native['id'], '--section', 'manifest')
        claim, failed = client.call('review-claim', {'run': run['id'], 'id': native['id'], 'actor': 'interop'})
        self.assertFalse(failed, claim)

    def test_relative_colon_links_survive_finish_and_memory(self):
        run = self.run_ready()
        report = self.root / 'reports/report.md'; report.parent.mkdir()
        target = report.parent / 'attachments/result:2026.txt'
        target.parent.mkdir(); target.write_text('evidence')
        report.write_text('[данные](attachments/result:2026.txt?download=1#part)\n'
                          '[reference]: <attachments/result:2026.txt>\n'
                          '[web](https://example.test/a:b) [mail](mailto:person@example.test)\n'
                          '`[code](attachments/result:2026.txt)`\n')
        native = self.cli('memory-add', '--project', 'demo', '--title', 'Links', '--file', str(report))
        link = Path(native['file']).read_text().split('](')[1].split(')')[0]
        self.assertEqual((Path(native['file']).parent / unquote(urlsplit(link).path)).resolve(), target)
        client = Client(Path(BINARY).resolve(), self.args, self.env)
        self.addCleanup(client.close)
        saved, failed = client.call('memory-add', {'project': 'demo', 'title': 'Links', 'file': str(report)})
        self.assertFalse(failed, saved)
        self.assertEqual(Path(saved['file']).read_text(), Path(native['file']).read_text())
        self.cli('finish', '--run', run['id'], '--report', str(report))
        result = self.state / 'runs' / run['id'] / 'result.md'
        link = result.read_text().split('](')[1].split(')')[0]
        self.assertEqual((result.parent / unquote(urlsplit(link).path)).resolve(), target)
        self.assertTrue((result.parent / unquote(urlsplit(link).path)).exists())

    def test_canonical_float_corpus_matches_python(self):
        run = self.run_ready()
        rng = random.Random(20261027)
        values = [0.0, -0.0, 1461205785224690.2, -1250201187231379.2, 11949781134863.562]
        for _ in range(10000):
            value = struct.unpack('>d', rng.getrandbits(64).to_bytes(8, 'big'))[0]
            if math.isfinite(value):
                values.append(value)
        for exponent in range(-1074, 1024):
            value = math.ldexp(1.0, exponent)
            for neighbor in (math.nextafter(value, 0.0), value, math.nextafter(value, math.inf)):
                if math.isfinite(neighbor):
                    values.extend((neighbor, -neighbor))
        for exponent in range(-323, 309):
            value = float('1e' + str(exponent))
            for neighbor in (math.nextafter(value, 0.0), value, math.nextafter(value, math.inf)):
                if math.isfinite(neighbor):
                    values.extend((neighbor, -neighbor))
        project_file = self.state / 'projects/demo.json'
        project = json.loads(project_file.read_text()); project['future_floats'] = values
        project_file.write_text(json.dumps(project))
        report = self.root / 'report.md'; report.write_text('Checked')
        self.cli('finish', '--run', run['id'], '--report', str(report))
        attempt = self.cli('review-request', '--run', run['id'])
        snapshot = json.loads(Path(attempt['snapshot']).read_text())
        expected = hashlib.sha256(json.dumps(snapshot['manifest'], sort_keys=True, ensure_ascii=False).encode()).hexdigest()
        self.assertEqual(attempt['version'], expected)
        self.cli('review-read', '--run', run['id'], '--id', attempt['id'])

    def test_integer_negative_zero_snapshot_can_cross_cli_and_mcp(self):
        run = self.run_ready()
        project_file = self.state / 'projects/demo.json'
        text = project_file.read_text().rstrip()
        project_file.write_text(text[:-1] + ', "future_integer_zero": -0, "future_float_zero": -0.0}')
        report = self.root / 'report.md'; report.write_text('Checked')
        self.cli('finish', '--run', run['id'], '--report', str(report))
        client = Client(Path(BINARY).resolve(), self.args, self.env)
        self.addCleanup(client.close)
        attempt, failed = client.call('review-request', {'run': run['id']})
        self.assertFalse(failed, attempt)
        frozen = json.loads(Path(attempt['snapshot']).read_text())
        self.assertEqual(frozen['manifest']['project']['future_integer_zero'], 0)
        self.assertEqual(math.copysign(1.0, frozen['manifest']['project']['future_float_zero']), -1.0)
        expected = hashlib.sha256(json.dumps(frozen['manifest'], sort_keys=True, ensure_ascii=False).encode()).hexdigest()
        self.assertEqual(attempt['version'], expected)
        self.cli('review-read', '--run', run['id'], '--id', attempt['id'], '--section', 'manifest')
        saved, failed = client.call('review-read', {'run': run['id'], 'id': attempt['id'], 'section': 'manifest'})
        self.assertFalse(failed, saved)
        self.assertEqual(saved['version'], expected)

    def test_python_nonfinite_snapshots_cross_cli_and_strict_mcp(self):
        self.env['CONTROL_WORKFLOW_PROGRAM'] = str(SOURCE)
        run = self.run_ready()
        project_file = self.state / 'projects/demo.json'
        text = project_file.read_text().rstrip()
        project_file.write_text(text[:-1] + ', "future_values": [1e309,-1e309,NaN,Infinity,-Infinity],'
                                ' "future_string": "Infinity", "a/~": {"value": NaN},'
                                ' "future_integer": 123456789012345678901234567890}')
        self.cli('project-set', 'demo', '--description', 'Preserved')
        current = self.cli('show', 'projects', 'demo')
        self.assertTrue(math.isnan(current['future_values'][2]))
        self.assertEqual(current['future_integer'], 123456789012345678901234567890)
        report = self.root / 'report.md'; report.write_text('Checked')
        self.cli('finish', '--run', run['id'], '--report', str(report))
        attempt = self.cli('review-request', '--run', run['id'])
        self.cli('review-read', '--run', run['id'], '--id', attempt['id'], '--section', 'manifest')
        client = Client(Path(BINARY).resolve(), self.args, self.env)
        self.addCleanup(client.close)
        response = client.request('tools/call', {'name': 'review-read', 'arguments':
                                  {'run': run['id'], 'id': attempt['id'], 'section': 'manifest'}})['result']
        self.assertFalse(response.get('isError'), response)
        data = response['structuredContent']['result']
        tags = response['structuredContent']['nonFiniteNumbers']
        labels = ['Infinity', '-Infinity', 'NaN', 'Infinity', '-Infinity']
        self.assertEqual(data['manifest']['project']['future_values'], labels)
        for index, label in enumerate(labels):
            self.assertEqual(tags['/manifest/project/future_values/' + str(index)], label)
        self.assertEqual(tags['/manifest/project/a~1~0/value'], 'NaN')
        self.assertNotIn('/manifest/project/future_string', tags)
        self.assertEqual(data['manifest']['project']['future_integer'], 123456789012345678901234567890)
        raw = json.loads(response['content'][0]['text'])
        self.assertTrue(math.isnan(raw['manifest']['project']['future_values'][2]))
        claim, failed = client.call('review-claim', {'run': run['id'], 'id': attempt['id'], 'actor': 'interop'})
        self.assertFalse(failed, claim)
        verdict = self.root / 'verdict.json'
        verdict.write_text(json.dumps(dict(version=attempt['version'], status='passed', conclusion='Checked',
                                          findings=[], suggestions=[], evidence=['Interop'], limitations=[])))
        self.cli('review-conclude', '--run', run['id'], '--id', attempt['id'], '--actor', 'interop', '--result', str(verdict))
        project = json.loads(project_file.read_text()); project['future_revision'] = 1
        project_file.write_text(json.dumps(project))
        native, failed = client.call('review-request', {'run': run['id']})
        self.assertFalse(failed, native)
        self.cli('review-read', '--run', run['id'], '--id', native['id'])
