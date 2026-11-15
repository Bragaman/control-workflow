"""Distribution, isolated install/update and Claude lifecycle acceptance with fake herdr."""
import json
import hashlib
import fcntl
import platform
import io
import tarfile
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
            old_link = destination / 'control-task'
            original_target = os.readlink(old_link)
            (destination / 'control-update').unlink()
            self.call(script, 'skills', '--destination', destination)
            self.assertEqual(os.readlink(old_link), original_target)
            self.call(script, 'skills', '--destination', destination, '--action', 'check')
            self.assertTrue((destination / 'control-update' / 'SKILL.md').is_file())
            self.assertEqual(len(list(destination.iterdir())), 10)
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

    def test_release_update_and_failures(self):
        prefix = self.root / 'program'
        package = self.root / 'package'
        self.call(BINARY, 'install', '--prefix', prefix)
        self.call(BINARY, 'package', '--prefix', package)
        script = prefix / 'bin/control-workflow'
        manifest = prefix / 'distribution-rust.json'
        old = json.loads(manifest.read_text()); version = old['version']; old['version'] = '0.0.0'
        manifest.write_text(json.dumps(old))
        tag = 'v' + version
        arch = 'aarch64' if platform.machine() == 'arm64' else platform.machine()
        platform_name = ('macos' if sys.platform == 'darwin' else 'linux') + '-' + arch
        name = f'control-workflow-{tag}-{platform_name}.tar.gz'
        releases = self.root / 'releases'; releases.mkdir()
        subprocess.run(['tar', '--format', 'ustar', '-czf', str(releases / name), '-C', str(package), '.'], check=True)
        checksum = hashlib.sha256((releases / name).read_bytes()).hexdigest()
        (releases / 'SHA256SUMS').write_text(f'{checksum}  {name}\n')
        (releases / 'latest').write_text(json.dumps({'tag_name': tag}))
        fakebin = self.root / 'fakebin'; fakebin.mkdir()
        curl = fakebin / 'curl'
        curl.write_text("#!/usr/bin/env python3\nimport os, pathlib, shutil, sys\na=sys.argv; url=a[-1]; name='latest' if url.endswith('/latest') else url.rsplit('/',1)[-1]\nshutil.copyfile(pathlib.Path(os.environ['FAKE_RELEASES']) / name, a[a.index('--output')+1])\n")
        curl.chmod(0o755)
        self.env['PATH'] = str(fakebin) + os.pathsep + self.env['PATH']
        self.env['FAKE_RELEASES'] = str(releases)
        state = self.root / 'data'; state.mkdir(); (state / 'checkpoint').write_text('saved')
        config = self.root / 'config.json'; config.write_text('{"schema_version":1}')
        args = ['--state-root', state, '--config', config, 'update']
        checked = json.loads(self.call(script, *args, '--check').stdout)
        self.assertEqual((checked['current'], checked['available']), ('0.0.0', tag))
        (releases / 'SHA256SUMS').write_text(f'{"0" * 64}  {name}\n')
        self.call(script, *args, ok=False)
        self.assertEqual(json.loads(manifest.read_text())['version'], '0.0.0')
        original_archive = (releases / name).read_bytes()
        with tarfile.open(releases / name, 'w:gz') as archive:
            entry = tarfile.TarInfo('../escape'); entry.size = 1
            archive.addfile(entry, io.BytesIO(b'x'))
        bad_checksum = hashlib.sha256((releases / name).read_bytes()).hexdigest()
        (releases / 'SHA256SUMS').write_text(f'{bad_checksum}  {name}\n')
        self.call(script, *args, ok=False)
        self.assertFalse((self.root / 'escape').exists())
        (releases / name).write_bytes(original_archive)
        (releases / 'SHA256SUMS').write_text(f'{checksum}  {name}\n')
        lockname = hashlib.sha256(str(prefix).encode()).hexdigest()
        with (self.root / f'.workflow-install-{lockname}.lock').open('w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.call(script, *args, ok=False)
        self.assertEqual(json.loads(manifest.read_text())['version'], '0.0.0')
        updated = json.loads(self.call(script, *args).stdout)
        self.assertEqual(updated['status'], 'updated')
        self.assertTrue(Path(updated['installation']['backup']).is_dir())
        self.assertEqual((state / 'checkpoint').read_text(), 'saved')
        self.assertEqual(config.read_text(), '{"schema_version":1}')
        self.assertEqual(json.loads(self.call(script, *args).stdout)['status'], 'unchanged')

    def test_skill_conflict_is_atomic_and_remove_refuses_foreign_link(self):
        destination = self.root / 'skills'; destination.mkdir()
        foreign = destination / 'control-task'; foreign.mkdir()
        self.call(BINARY, 'skills', '--destination', destination, ok=False)
        self.assertEqual(list(destination.iterdir()), [foreign]); foreign.rmdir()
        self.call(BINARY, 'skills', '--destination', destination)
        foreign.unlink(); foreign.symlink_to(self.root / 'missing')
        self.call(BINARY, 'skills', '--destination', destination, '--action', 'remove', ok=False)
        self.assertEqual(len(list(destination.iterdir())), 10)

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
