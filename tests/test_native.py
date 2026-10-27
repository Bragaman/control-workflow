"""Run the established black-box contract suite against the native executable.

The Python shim is only a test launcher. Production CLI/MCP never invokes it.
Set CONTROL_WORKFLOW_BIN to a compiled binary before running this module.
"""
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
import test_control as fixture

ORIGINAL = fixture.SCRIPT
NATIVE = os.environ.get('CONTROL_WORKFLOW_BIN')
TEMP = None


def setUpModule():
    global TEMP
    if not NATIVE:
        raise unittest.SkipTest('Set CONTROL_WORKFLOW_BIN to test native parity')
    TEMP = tempfile.TemporaryDirectory(prefix='workflow-native-suite-')
    root = Path(TEMP.name)
    shutil.copytree(ORIGINAL.parent, root / 'scripts')
    shutil.copytree(ORIGINAL.parents[1] / 'config', root / 'config')
    shim = root / 'scripts/control.py'
    shim.write_text('import os, sys\nos.execv(' + repr(str(Path(NATIVE).resolve())) + ', ['
                    + repr(str(Path(NATIVE).resolve())) + ', *sys.argv[1:]])\n')
    fixture.SCRIPT = shim


def tearDownModule():
    fixture.SCRIPT = ORIGINAL
    if TEMP:
        TEMP.cleanup()


class NativeControlTests(fixture.ControlTests):
    @unittest.skip('Python source monkeypatch is not a native failure injector; paired-write recovery has a separate fixture')
    def test_agent_review_repairs_partial_run_task_write(self):
        pass

    def test_claude_native_adapter_and_pinned_run(self):
        config = Path(self.env['CONTROL_PLANE_CONFIG'])
        settings = json.loads(config.read_text())
        settings['adapter'] = 'herdr-claude'
        config.write_text(json.dumps(settings))
        self.env['FAKE_KIND'] = 'claude'
        self.create()
        run = self.start()
        args = next(c for c in self.calls() if c[:2] == ['agent', 'start'])
        self.assertEqual(args[args.index('--kind') + 1], 'claude')
        self.assertEqual(args[args.index('--permission-mode') + 1], 'default')
        self.assertEqual(args[args.index('--session-id') + 1], run['requested_agent_session_id'])
        self.assertNotIn('--dangerously-skip-permissions', args)
        settings['adapter'] = 'herdr-codex'
        config.write_text(json.dumps(settings))
        self.env.update(FAKE_LIVE='1', FAKE_AUTO_ASSIGNMENT='1')
        self.assertEqual(self.cli('open', 'task1')[0]['run'], run['id'])
        self.assertEqual(len([c for c in self.calls() if c[:2] == ['agent', 'start']]), 1)
