"""Portable policy and explicit model selection through the public CLI/MCP."""
import json
from pathlib import Path
import subprocess
import sys
import unittest
import test_control as fixture
from mcp_client import Client


class ModelPolicyTests(unittest.TestCase):
    setUp = fixture.ControlTests.setUp
    cli = fixture.ControlTests.cli
    create = fixture.ControlTests.create
    wait_run = fixture.ControlTests.wait_run
    calls = fixture.ControlTests.calls

    def policy_config(self, **overrides):
        config = Path(self.env['CONTROL_PLANE_CONFIG'])
        config.write_text(json.dumps({'model_policy': 'builtin', **overrides}))
        return config

    def test_claude_identifiers_in_legacy_and_named_profiles(self):
        models = ['sonnet[1m]', 'opus[1m]',
                  'arn:aws:bedrock:us-east-1:123456789012:inference-profile/example-model']
        env = {**self.env, 'FAKE_KIND': 'claude'}
        for named in [False, True]:
            for index, model in enumerate(models):
                with self.subTest(named=named, model=model):
                    config = Path(self.env['CONTROL_PLANE_CONFIG'])
                    settings = {'adapter': 'herdr-claude'}
                    if named:
                        settings.update(model_policy='builtin', model_overrides={
                            'profiles': {'terra': {'model': model}}})
                    else:
                        settings['profiles'] = {'implementation': {'normal': {
                            'model': model, 'reasoning_effort': 'medium'}}}
                    config.write_text(json.dumps(settings))
                    self.assertEqual(self.cli('environment')['adapter'], 'herdr-claude')
                    key = f'claude-{int(named)}-{index}'
                    task = self.create(key, '--no-code', *(['--profile', 'terra'] if named else []))
                    self.assertEqual(task['model'], model)
                    run = self.cli('start', key, env=env)['run']
                    self.assertEqual(self.wait_run(run)['model'], model)
                    starts = [a for a in self.calls() if a[:2] == ['agent', 'start']]
                    self.assertEqual(starts[-1][starts[-1].index('--model') + 1], model)

    def test_mcp_launch_uses_snapshot_after_external_policy_is_removed(self):
        policy = self.root / 'policy.json'
        policy.write_text((fixture.ROOT / 'config/model-policy.json').read_text())
        self.policy_config(model_policy=str(policy))
        self.create('task1', '--profile', 'luna', '--no-code')
        client = Client(fixture.BINARY, env=self.env)
        self.addCleanup(client.close)
        client.request('tools/list')
        policy.unlink()
        result, failed = client.call('open', {'ids': ['task1'], 'profile': 'terra'})
        self.assertFalse(failed)
        run = self.wait_run(result[0]['run'])
        self.assertEqual((run['profile'], run['model'], run['reasoning_effort']),
                         ('terra', 'gpt-6.1-terra', 'medium'))
        self.assertEqual(run['model_selection']['policy'], str(policy))
        task, failed = client.call('show', {'kind': 'tasks', 'id': 'task1'})
        self.assertFalse(failed)
        self.assertEqual(task['profile'], 'luna')

    def test_cli_launch_uses_snapshot_if_policy_disappears_after_resolution(self):
        policy = self.root / 'policy.json'
        policy.write_text((fixture.ROOT / 'config/model-policy.json').read_text())
        self.policy_config(model_policy=str(policy))
        self.create('task1', '--no-code')
        fake = self.root / 'bin/herdr'
        hook = 'if a[:3] == ["session", "list", "--json"]:\n'
        fake.write_text(fake.read_text().replace(
            hook, hook + '    Path(os.environ["REMOVE_POLICY"]).unlink(missing_ok=True)\n'))
        env = {**self.env, 'REMOVE_POLICY': str(policy)}
        run = self.cli('open', 'task1', '--profile', 'terra', env=env)[0]['run']
        self.assertFalse(policy.exists())
        self.assertEqual(self.wait_run(run)['model'], 'gpt-6.1-terra')

    @unittest.skipUnless(sys.platform.startswith('linux'), 'wrapper uses Linux process identity')
    def test_incomplete_launch_persists_selection_and_allows_compatible_reuse(self):
        self.policy_config()
        self.create('legacy', '--profile', 'luna', '--no-code')
        run = self.cli('start', 'legacy')['run']
        self.wait_run(run)
        state = self.root / 'runs' / run / 'state.json'
        record = json.loads(state.read_text())
        record['status'] = 'launching'
        for key in ['model', 'reasoning_effort', 'profile', 'profile_source', 'model_selection']:
            record.pop(key, None)
        state.write_text(json.dumps(record))
        # Emulate an old pending run, preserving real launcher ownership across exec.
        wrapper = self.root / 'legacy-launch.py'
        wrapper.write_text('''import json, os, sys
from pathlib import Path
path = Path(sys.argv[1]); record = json.loads(path.read_text()); pid = os.getpid()
record['worker_pid'] = pid
record['worker_identity'] = {
    'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
    'start_ticks': Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]}
path.write_text(json.dumps(record))
os.execv(sys.argv[2], [sys.argv[2], '_launch', record['id']])
''')
        result = subprocess.run([sys.executable, str(wrapper), str(state), str(fixture.BINARY)],
                                env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        saved = json.loads(state.read_text())
        self.assertEqual(saved['status'], 'running')
        self.assertEqual((saved.get('model'), saved.get('reasoning_effort'), saved.get('profile')),
                         ('gpt-6-luna', 'medium', 'luna'))
        self.assertEqual(saved['model_selection']['source'], 'task')
        argv = saved['launch_argv']
        self.assertEqual(argv[argv.index('--model') + 1], saved['model'])
        self.assertIn('model_reasoning_effort=medium', argv)
        live = {**self.env, 'FAKE_LIVE': '1', 'FAKE_AUTO_ASSIGNMENT': '1'}
        reused = self.cli('open', 'legacy', '--profile', 'luna', env=live)[0]
        self.assertEqual((reused['run'], reused['action']), (run, 'existing'))

    def test_task_selection_rules_edit_and_auto(self):
        self.policy_config()
        task = self.create('task1', '--profile', 'terra', '--complexity', 'complex')
        self.assertEqual((task['profile'], task['model'], task['complexity']),
                         ('terra', 'gpt-6.1-terra', 'complex'))
        self.assertEqual(task['model_selection']['source'], 'task')
        task = self.cli('task-set', 'task1', '--goal', 'Другая цель')
        self.assertEqual(task['profile'], 'terra')
        task = self.cli('task-set', 'task1', '--profile', 'auto')
        self.assertEqual((task['profile'], task['profile_source']), ('sol-high', 'rule'))
        task = self.cli('task-set', 'task1', '--complexity', 'normal')
        self.assertEqual(task['profile'], 'sol')
        failed = self.cli(
            'task-set', 'task1', '--profile', 'missing', ok=False)
        self.assertIn('Неизвестный профиль', failed['error'])
        self.assertEqual(self.cli('show', 'tasks', 'task1')['profile'], 'sol')

    def test_run_override_live_reuse_and_changed_policy(self):
        self.policy_config()
        self.create('task1', '--profile', 'terra', '--no-code')
        run = self.cli('open', 'task1', '--profile', 'sol-high')[0]['run']
        r = self.wait_run(run)
        self.assertEqual((r['profile'], r['model'], r['reasoning_effort']),
                         ('sol-high', 'gpt-6.1-sol', 'high'))
        self.assertEqual(r['model_selection']['source'], 'run')
        self.assertEqual(self.cli('show', 'tasks', 'task1')['profile'], 'terra')
        live = {**self.env, 'FAKE_LIVE': '1', 'FAKE_AUTO_ASSIGNMENT': '1'}
        self.assertEqual(self.cli('open', 'task1', '--profile', 'sol-high', env=live)[0]['run'], run)
        failed = self.cli('open', 'task1', '--profile', 'terra', env=live, ok=False)[0]
        self.assertIn('уже использует профиль', failed['error'])
        self.policy_config(model_overrides={'profiles': {'sol-high': {'reasoning_effort': 'medium'}}})
        self.assertEqual(self.cli('open', 'task1', env=live)[0]['run'], run)
        self.assertEqual(self.cli('open', 'task1', '--profile', 'sol-high', env=live, ok=False)[0]['action'], 'error')
        self.assertEqual(self.cli('run', run)['reasoning_effort'], 'high')
        starts = [a for a in self.calls() if a[:2] == ['agent', 'start']]
        self.assertEqual(len(starts), 1)
        self.assertEqual(starts[0][starts[0].index('--model') + 1], 'gpt-6.1-sol')

    def test_two_instances_share_file_and_sparse_override_preserves_settings(self):
        policy = self.root / 'model-policy.json'
        policy.write_text((fixture.ROOT / 'config/model-policy.json').read_text())
        first = self.root / 'a'; second = self.root / 'different' / 'b'
        results = []
        for directory in [first, second]:
            directory.mkdir(parents=True)
            config = directory / 'config.json'
            config.write_text(json.dumps({'model_policy': str(policy), 'git_mode': 'branch',
                'agent_start_timeout_ms': 90000,
                'model_overrides': {'profiles': {'luna': {'reasoning_effort': 'high'}}}}))
            prefix = ['--state-root', str(directory / 'state'), '--bindings-root', str(directory / 'bindings'),
                      '--config', str(config)]
            self.cli(*prefix, 'project-create', 'lab', '--name', 'Lab')
            t = self.cli(*prefix, 'task-create', 'task1', '--project', 'lab', '--title', 'Demo',
                '--goal', 'Demo', '--acceptance', 'Demo', '--profile', 'luna')
            results.append((t['model'], t['reasoning_effort'], t['git_mode']))
            env = self.cli(*prefix, 'environment')
            self.assertEqual(env['model_profiles']['profiles']['terra']['model'], 'gpt-6.1-terra')
            self.assertEqual(env['model_profiles']['profiles']['sol-high']['reasoning_effort'], 'high')
            self.assertEqual(env['bindings_root'], str(directory / 'bindings'))
        self.assertEqual(results, [('gpt-6-luna', 'high', 'branch')] * 2)
        # Relative selection resolves against config, not the invoking cwd.
        config = first / 'config.json'
        config.write_text(json.dumps({'model_policy': '../model-policy.json'}))
        self.assertEqual(self.cli('--config', str(config), 'environment')['model_policy'], str(policy))

    def test_unavailable_and_forbidden_models_do_not_fallback(self):
        self.policy_config()
        self.create('task1', '--profile', 'luna', '--no-code')
        failed_env = {**self.env, 'FAKE_FAIL': '1'}
        run = self.cli('start', 'task1', '--profile', 'astra', env=failed_env)['run']
        r = self.wait_run(run, 'launch_failed')
        self.assertEqual((r['model'], r['profile']), ('gpt-6-astra', 'astra'))
        self.assertEqual(len([a for a in self.calls() if a[:2] == ['agent', 'start']]), 1)
        self.policy_config(allowed_models=['gpt-6-luna'])
        self.create('task2', '--profile', 'luna', '--no-code')
        calls = len(self.calls())
        self.assertIn('allowed_models', self.cli('open', 'task2', '--profile', 'terra', ok=False)[0]['error'])
        self.assertEqual(len(self.calls()), calls)
        self.assertIsNone(self.cli('show', 'tasks', 'task2')['active_run'])

    def test_legacy_task_and_config_keep_old_behavior_until_opt_in(self):
        t = self.create('task1', '--no-code')
        self.assertEqual(t['model'], 'fixture-implementation')
        file = self.root / 'tasks/task1.json'
        t = json.loads(file.read_text())
        for key in ['profile', 'profile_source', 'model_selection']:
            t.pop(key, None)
        file.write_text(json.dumps(t))
        self.policy_config()
        run = self.cli('start', 'task1')['run']
        self.assertEqual(self.wait_run(run)['profile'], 'sol')

    def test_mcp_profile_contract_and_snapshot(self):
        self.policy_config()
        client = Client(fixture.BINARY, env=self.env)
        self.addCleanup(client.close)
        tools = {v['name']: v for v in client.request('tools/list')['result']['tools']}
        for name in ['task-create', 'task-set', 'open', 'start']:
            self.assertEqual(tools[name]['inputSchema']['properties']['profile']['type'], 'string')
        task, failed = client.call('task-create', {'id': 'task1', 'project': 'lab', 'title': 'Demo',
            'goal': 'Demo', 'acceptance': 'Demo', 'profile': 'terra', 'code': False})
        self.assertFalse(failed)
        self.assertEqual(task['profile'], 'terra')
        task, failed = client.call('task-set', {'id': 'task1', 'profile': 'luna'})
        self.assertFalse(failed)
        self.assertEqual(task['profile'], 'luna')
        result, failed = client.call('open', {'ids': ['task1'], 'profile': 'sol-high'})
        self.assertFalse(failed)
        run = self.wait_run(result[0]['run'])
        self.assertEqual(run['profile'], 'sol-high')
        self.assertEqual(run['model_selection']['source'], 'run')
        result, failed = client.call('open', {'ids': ['task1'], 'profile': 'missing'})
        self.assertTrue(failed)
        self.assertEqual(result[0]['action'], 'error')

    def test_invalid_policy_fails_before_state_is_created(self):
        for overrides in [
            {'profiles': {}},  # conflict with legacy matrix
            {'model_overrides': {'rules': {'research': {'complex': 'unknown'}}}},
            {'model_overrides': {'profiles': {'luna': {'reasoning_effort': 'typo'}}}},
        ]:
            config = self.policy_config(**overrides)
            state = self.root / 'unused'
            result = subprocess.run([str(fixture.BINARY), '--config', str(config), '--state-root', str(state),
                'environment'], env=self.env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertFalse(state.exists())
