"""Distribution, isolated install/update and Claude lifecycle acceptance with fake herdr."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import test_control as fixture
ROOT = fixture.ROOT
BINARY = fixture.BINARY


class ClaudeLifecycleTests(fixture.ControlTests):
    def setUp(self):
        super().setUp()
        config = Path(self.env['CONTROL_PLANE_CONFIG'])
        settings = json.loads(config.read_text())
        settings['adapter'] = 'herdr-claude'
        config.write_text(json.dumps(settings))
        self.env['FAKE_KIND'] = 'claude'

    def test_execution_profiles_reach_codex_and_run(self):
        self.create()
        run = self.start()
        args = next(c for c in self.calls() if c[:2] == ['agent', 'start'])
        self.assertEqual(args[args.index('--kind') + 1], 'claude')
        self.assertEqual(args[args.index('--permission-mode') + 1], 'default')
        self.assertEqual(args[args.index('--effort') + 1], 'medium')
        self.assertEqual(args[args.index('--model') + 1], 'fixture-implementation')
        self.assertEqual(args[args.index('--session-id') + 1], run['requested_agent_session_id'])
        self.assertFalse(run['capabilities']['session_id_capture'])
        self.assertNotIn('--dangerously-skip-permissions', args)

    def test_live_run_keeps_adapter_when_config_changes(self):
        self.create(); run = self.start()
        config = Path(self.env['CONTROL_PLANE_CONFIG'])
        settings = json.loads(config.read_text()); settings['adapter'] = 'herdr-codex'
        config.write_text(json.dumps(settings))
        self.env['FAKE_LIVE'] = '1'; self.env['FAKE_AUTO_ASSIGNMENT'] = '1'
        opened = self.cli('open', 'task1')[0]
        self.assertEqual(opened['run'], run['id'])
        self.assertEqual(len([c for c in self.calls() if c[:2] == ['agent', 'start']]), 1)
        self.assertEqual(self.cli('run', run['id'])['adapter'], 'herdr-claude')


class PublicInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='public-install-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('CONTROL_PLANE_', 'HERDR_'))}
        self.env.update(HOME=str(self.root), PYTHONDONTWRITEBYTECODE='1', XDG_CONFIG_HOME=str(self.root / 'config'),
                        XDG_STATE_HOME=str(self.root / 'state'))

    def call(self, script, *args, ok=True):
        result = subprocess.run([str(script), *map(str, args)], env=self.env,
                                cwd=self.root, text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode == 0, ok, result.stdout + result.stderr)
        return result

    def test_install_update_preserves_checkpoint_and_skill_connections(self):
        prefix = self.root / 'программа с пробелами'
        installed = json.loads(self.call(BINARY, 'install', '--prefix', prefix).stdout)
        self.assertFalse(installed['state_copied'])
        state = self.root / 'state-instance'; bindings = self.root / 'machine-bindings'
        config = self.root / 'user.json'; config.write_text('{"schema_version": 1}')
        script = prefix / 'bin/control-workflow'
        cli = ['--state-root', state, '--bindings-root', bindings, '--config', config]
        self.call(script, *cli, 'project-create', 'demo', '--name', 'Synthetic')
        checkpoint = state / 'checkpoint.md'; checkpoint.write_text('Saved checkpoint')
        before = {str(p.relative_to(state)): p.read_bytes() for p in state.rglob('*') if p.is_file()}
        config_before = config.read_bytes()
        for destination in (self.root / '.agents/skills', self.root / '.claude/skills'):
            self.call(script, 'skills', '--destination', destination)
            self.call(script, 'skills', '--destination', destination, '--action', 'check')
            self.assertEqual(len(list(destination.iterdir())), 8)
        updated = json.loads(self.call(BINARY, 'install', '--prefix', prefix).stdout)
        self.assertTrue(Path(updated['backup']).is_dir())
        self.assertEqual(before, {str(p.relative_to(state)): p.read_bytes() for p in state.rglob('*') if p.is_file()})
        self.assertEqual(config_before, config.read_bytes())
        self.call(script, *cli, 'environment')
        for destination in (self.root / '.agents/skills', self.root / '.claude/skills'):
            self.call(script, 'skills', '--destination', destination, '--action', 'check')
            self.call(script, 'skills', '--destination', destination, '--action', 'remove')
            self.assertEqual(list(destination.iterdir()), [])

    def test_install_refuses_modified_or_unmanaged_files(self):
        prefix = self.root / 'program'
        self.call(BINARY, 'install', '--prefix', prefix)
        unmanaged = prefix / 'personal.json'; unmanaged.write_text('personal')
        self.call(BINARY, 'install', '--prefix', prefix, ok=False)
        self.assertEqual(unmanaged.read_text(), 'personal'); unmanaged.unlink()
        managed = prefix / 'README.md'; managed.write_text('local changes')
        self.call(BINARY, 'install', '--prefix', prefix, ok=False)
        self.assertEqual(managed.read_text(), 'local changes')

    def test_skill_conflict_is_atomic_and_remove_refuses_foreign_link(self):
        destination = self.root / 'skills'; destination.mkdir()
        foreign = destination / 'control-task'; foreign.mkdir()
        self.call(BINARY, 'skills', '--destination', destination, ok=False)
        self.assertEqual(list(destination.iterdir()), [foreign]); foreign.rmdir()
        self.call(BINARY, 'skills', '--destination', destination)
        foreign.unlink(); foreign.symlink_to(self.root / 'missing')
        self.call(BINARY, 'skills', '--destination', destination, '--action', 'remove', ok=False)
        self.assertEqual(len(list(destination.iterdir())), 8)

    def test_default_paths_are_external_and_unknown_schema_fails_before_state(self):
        script = BINARY
        environment = json.loads(self.call(script, 'environment').stdout)
        expected = self.root / ('Library/Application Support/control-workflow/state' if sys.platform == 'darwin' else 'state/control-workflow')
        self.assertEqual(environment['state_root'], str(expected))
        self.assertFalse((ROOT / 'projects').exists())
        state = self.root / 'unused'; config = self.root / 'bad.json'
        config.write_text('{"schema_version": 2}')
        self.call(script, '--state-root', state, '--config', config, 'project-create', 'demo', '--name', 'Demo', ok=False)
        self.assertFalse(state.exists())

    def test_default_model_uses_cli_configuration_for_both_adapters(self):
        for adapter, kind in [('herdr-codex', 'codex'), ('herdr-claude', 'claude')]:
            with self.subTest(adapter=adapter):
                case = fixture.ControlTests()
                case.setUp()
                try:
                    config = Path(case.env['CONTROL_PLANE_CONFIG'])
                    settings = json.loads(config.read_text())
                    settings['adapter'] = adapter
                    settings['profiles']['implementation']['normal']['model'] = 'default'
                    config.write_text(json.dumps(settings))
                    case.env['FAKE_KIND'] = kind
                    case.create('default-model')
                    case.start('default-model')
                    args = next(c for c in case.calls() if c[:2] == ['agent', 'start'])
                    self.assertNotIn('--model', args)
                    self.assertTrue(args[-1].startswith('default-model: '))
                finally:
                    case.doCleanups()
