"""Integration checks for independent state, read-only resources and saved artifacts."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import unittest
from urllib.parse import unquote, urlsplit
import test_control as fixture

BINARY = fixture.BINARY


class RuntimeTests(unittest.TestCase):
    setUp = fixture.ControlTests.setUp
    cli = fixture.ControlTests.cli
    create = fixture.ControlTests.create
    start = fixture.ControlTests.start
    wait_run = fixture.ControlTests.wait_run
    calls = fixture.ControlTests.calls

    def test_two_states_from_arbitrary_cwd_and_cli_precedence(self):
        second = self.root / 'second'
        argv = [str(BINARY), '--state-root', str(second)]
        result = subprocess.run([*argv, 'project-create', 'other', '--name', 'Другой'],
                                env=self.env, cwd='/tmp', capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual([p['id'] for p in self.cli('projects')], ['lab'])
        self.assertEqual([p['id'] for p in self.cli('--state-root', str(second), 'projects')], ['other'])
        self.assertFalse((second / 'projects/lab.json').exists())
        info = self.cli('--state-root', str(second), 'environment')
        self.assertEqual(info['state_root'], str(second))
        for document in info['documents'].values():
            self.assertTrue(Path(document).is_file())
            self.assertFalse(Path(document).is_relative_to(second))

    def test_config_profile_binding_and_detached_children_keep_explicit_paths(self):
        state = self.root / 'state2'; bindings = self.root / 'machine2'
        config = self.root / 'user.json'
        config.write_text(json.dumps({'git_mode': 'branch', 'profiles': {'implementation': {
            'normal': {'model': 'configured-model', 'reasoning_effort': 'high'}}}}))
        prefix = ['--config', str(config), '--state-root', str(state), '--bindings-root', str(bindings)]
        self.cli(*prefix, 'project-create', 'lab', '--name', 'Другой экземпляр')
        task = self.cli(*prefix, 'task-create', 'task2', '--project', 'lab', '--title', 'Проверка',
                        '--goal', 'Проверка', '--acceptance', 'Проверка', '--no-code', '--task-kind', 'implementation')
        self.assertEqual(task['git_mode'], 'branch')
        self.assertEqual(task['model'], 'configured-model')
        run = self.cli(*prefix, 'open', 'task2')[0]['run']
        original = self.root; self.root = state
        try:
            r = self.wait_run(run)
            assignment = (state / 'runs' / run / 'assignment.md').read_text()
            self.assertIn('--state-root ' + str(state), assignment)
            self.assertIn('--bindings-root ' + str(bindings), assignment)
            self.assertNotIn('# Инструкции исполнителю', assignment)
            self.assertEqual(r['model'], 'configured-model')
            self.assertEqual(r['cwd'], str(state))
            self.assertEqual(r['adapter'], 'herdr-codex')
            args = next(c for c in self.calls() if c[:2] == ['agent', 'start'])
            self.assertEqual(args[args.index('--model') + 1], 'configured-model')
            self.assertTrue(args[-1].startswith('task2: '))
            self.assertTrue((bindings / 'projects/lab.json').exists())
            self.assertFalse((state / 'projects/lab.json').read_text().find('workspace') >= 0)
            self.assertFalse((original / 'runs' / run).exists())
            report = state / 'report.md'; report.write_text('Готово')
            self.cli(*prefix, 'finish', '--run', run, '--report', str(report))
            env = {**self.env, 'HERDR_PANE_ID': r['pane']}
            result = self.cli(*prefix, 'complete', 'task2', env=env)
            self.assertEqual(result['status'], 'completion_requested')
            self.wait_run(run, 'done')
            request = state / 'runs' / run / 'completion-request.json'
            import time
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline and json.loads(request.read_text())['status'] != 'completed':
                time.sleep(.03)
            self.assertEqual(json.loads(request.read_text())['status'], 'completed')
        finally:
            self.root = original

    def test_invalid_adapter_or_profile_does_not_create_state(self):
        for settings in ({'adapter': 'claude'}, {'profiles': {'implementation': {'normal': {'model': ''}}}}):
            config = self.root / 'invalid.json'; config.write_text(json.dumps(settings))
            state = self.root / 'unused'
            self.cli('--config', str(config), '--state-root', str(state), 'open', 'task', ok=False)
            self.assertFalse(state.exists())

    def test_read_only_installation_launches_using_external_state(self):
        install = self.root / 'installation'
        self.cli('--state-root', str(self.root / 'package-state'), '--bindings-root', str(self.root / 'package-bindings'), 'package', '--prefix', str(install))
        for file in install.rglob('*'):
            file.chmod(0o555 if file.is_dir() else (0o555 if file.name == 'control-workflow' else 0o444))
        install.chmod(0o555)
        self.addCleanup(lambda: [p.chmod(0o755) for p in [install, *install.rglob('*')] if p.is_dir()])
        state = self.root / 'readonly-state'
        copied_binary = install / 'bin/control-workflow'
        def call(*args):
            result = subprocess.run([str(copied_binary), '--state-root', str(state), *args],
                                    cwd='/tmp', env=self.env, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return json.loads(result.stdout)
        call('project-create', 'lab', '--name', 'Read only')
        call('task-create', 'task', '--project', 'lab', '--title', 'Проверка', '--goal', 'Проверка',
             '--acceptance', 'Проверка', '--no-code')
        run = call('open', 'task')[0]['run']
        original = self.root; self.root = state
        try:
            r = self.wait_run(run)
            self.assertEqual(r['program_root'], str(install))
            assignment = (state / 'runs' / run / 'assignment.md').read_text()
            self.assertIn(str(install / 'docs/worker.md'), assignment)
            self.assertFalse((install / 'scripts').exists())
        finally:
            self.root = original

    def test_legacy_records_migrate_without_losing_local_fields(self):
        binding = self.root / 'bindings/projects/lab.json'
        project = self.cli('show', 'projects', 'lab')
        project['repositories'] = ['/known/repo']
        binding.unlink()
        file = self.root / 'projects/lab.json'; file.write_text(json.dumps(project))
        self.cli('project-set', 'lab', '--description', 'Описание')
        loaded = self.cli('show', 'projects', 'lab')
        self.assertEqual(loaded['repositories'], ['/known/repo'])
        self.assertEqual(loaded['description'], 'Описание')
        self.assertNotIn('repositories', json.loads(file.read_text()))
        self.assertEqual(json.loads(binding.read_text())['repositories'], ['/known/repo'])

    def test_block_writes_checkpoint_and_plan_reads_unstarted_task(self):
        self.create()
        plan = self.root / 'tasks/task1/plan.md'; plan.parent.mkdir(); plan.write_text('Сохранённый план')
        view = self.cli('plan', 'task1', '--pane', 'w7:p2')
        self.assertEqual(view['file'], str(plan))
        r = self.start()
        self.cli('block', '--run', r['id'], '--question', 'Продолжать?', '--checkpoint', 'Сделано и следующий шаг')
        directory = self.root / 'runs' / r['id']
        q = json.loads((directory / 'questions.json').read_text())
        self.assertEqual(Path(q['checkpoint_path']).read_text().strip(), q['checkpoint'])
        self.cli('answer', 'task1', '--text', 'Да')
        self.assertTrue((directory / 'checkpoint.md').is_file())

    def test_report_links_work_after_finish_and_memory_copy(self):
        self.create(); run = self.start()
        source = self.root / 'external/report.md'; source.parent.mkdir()
        artifact = source.parent / 'artifact with spaces.txt'; artifact.write_text('Полезный артефакт')
        source.write_text('[Артефакт](<artifact with spaces.txt> "title")\n![image](artifact%20with%20spaces.txt#part)\n'
                          '[ref]: <artifact with spaces.txt>\n[site](https://example.com/path)\n[anchor](#part)\n'
                          '`[code](relative.txt)`\n```md\n[code](relative.txt)\n```\n')
        self.cli('finish', '--run', run['id'], '--report', str(source))
        rows = self.cli('memory-list', '--project', 'lab')
        for file in (self.root / 'runs' / run['id'] / 'result.md', self.root / rows[0]['file']):
            text = file.read_text()
            targets = re.findall(r'\]\((<[^>]+>|[^\s)]+)', text)
            for target in targets[:2]:
                path = unquote(urlsplit(target.strip('<>')).path)
                self.assertEqual((file.parent / path).resolve(), artifact)
            ref = re.search(r'^\[ref\]: <([^>]+)>', text, re.M)[1]
            self.assertEqual((file.parent / unquote(ref)).resolve(), artifact)
            self.assertIn('[site](https://example.com/path)', text)
            self.assertIn('[anchor](#part)', text)
            self.assertIn('`[code](relative.txt)`', text)
            self.assertIn('```md\n[code](relative.txt)\n```', text)

    def test_materialize_bindings_supports_legacy_reader_and_preserves_binding(self):
        self.create()
        binding = self.root / 'bindings/projects/lab.json'
        before = binding.read_bytes()
        self.cli('materialize-bindings')
        project = json.loads((self.root / 'projects/lab.json').read_text())
        task = json.loads((self.root / 'tasks/task1.json').read_text())
        self.assertIn('workspace', project)
        self.assertIn('repositories', task)
        self.assertEqual(binding.read_bytes(), before)
        self.assertEqual(self.cli('show', 'projects', 'lab'), project)

    def test_ordinary_diagnostics_fallback_and_no_code_option(self):
        task = self.cli('task-create', 'diagnostics', '--project', 'lab', '--title', 'Диагностика доступа',
                        '--goal', 'Исправить доступ', '--acceptance', 'Вход работает', '--no-code')
        self.assertEqual(task['task_kind'], 'implementation')
        self.assertFalse(task['code'])
