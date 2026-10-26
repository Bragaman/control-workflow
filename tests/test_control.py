import json
import hashlib
import shutil
import os
import shlex
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "control.py"
FAKE_HERDR = '''#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
a = sys.argv[1:]
with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(json.dumps(a) + "\\n")
if a[:1] == ["--session"]:
    if a[1] == "unavailable":
        print("server unavailable", file=sys.stderr)
        sys.exit(1)
    a = a[2:]
if a[:3] == ["session", "list", "--json"]:
    print(json.dumps({"sessions": [{"name": "default", "socket_path": os.environ["FAKE_LOG"] + ".sock" + os.environ.get("FAKE_SOCKET_SUFFIX", "")}, {"name": "other", "socket_path": os.environ["FAKE_LOG"] + ".other.sock"}, {"name": "unavailable", "socket_path": os.environ["FAKE_LOG"] + ".unavailable.sock"}]}))
    sys.exit(0)
if os.environ.get("FAKE_UNAVAILABLE"):
    print("server unavailable", file=sys.stderr)
    sys.exit(1)
if a[:2] == ["workspace", "create"]:
    if os.environ.get("FAKE_CREATE_FAIL"):
        Path(os.environ["FAKE_LOG"] + ".created").touch()
        print(json.dumps({"error": {"code": "timeout"}}), file=sys.stderr)
        sys.exit(1)
    r = {"workspace": {"workspace_id": "w7"}, "tab": {"tab_id": "w7:t1"}, "root_pane": {"pane_id": "w7:p1"}}
elif a[:2] == ["workspace", "list"]:
    r = {"workspaces": []}
elif a[:2] == ["workspace", "get"]:
    if os.environ.get("FAKE_GET_ERROR"):
        print(json.dumps({"error": {"code": os.environ["FAKE_GET_ERROR"]}}), file=sys.stderr)
        sys.exit(1)
    if os.environ.get("FAKE_BAD_PROTOCOL"):
        print("{bad json")
        sys.exit(0)
    if os.environ.get("FAKE_DISAPPEAR"):
        counter = Path(os.environ["FAKE_LOG"] + ".get-count")
        count = int(counter.read_text()) + 1 if counter.exists() else 1
        counter.write_text(str(count))
    else:
        count = 0
    if a[2] == "stale" or count > 1:
        print(json.dumps({"error": {"code": "workspace_not_found"}}), file=sys.stderr)
        sys.exit(1)
    r = {"workspace": {"workspace_id": a[2]}}
elif a[:2] == ["tab", "create"]:
    if os.environ.get("FAKE_TAB_FAIL"):
        Path(os.environ["FAKE_LOG"] + ".created").touch()
        print(json.dumps({"error": {"code": "timeout"}}), file=sys.stderr)
        sys.exit(1)
    r = {"tab": {"tab_id": "w7:t2"}, "root_pane": {"pane_id": "w7:p2"}}
elif a[:2] == ["tab", "get"]:
    if Path(os.environ["FAKE_LOG"] + ".closed").exists():
        print(json.dumps({"error": {"code": "tab_not_found"}}), file=sys.stderr)
        sys.exit(1)
    r = {"tab": {"tab_id": a[2], "workspace_id": "w7"}}
elif a[:2] == ["pane", "list"]:
    r = {"panes": [{"pane_id": "w7:p1", "tab_id": "w7:t1", "agent": os.environ.get("FAKE_KIND", "codex")}]}
    if os.environ.get("FAKE_OTHER_AGENT"):
        r["panes"].append({"pane_id": "w7:p3", "tab_id": "w7:t1", "agent": os.environ.get("FAKE_KIND", "codex")})
elif a[:2] == ["tab", "close"]:
    time.sleep(float(os.environ.get("FAKE_CLOSE_DELAY", "0")))
    if os.environ.get("FAKE_CLOSE_FAIL"):
        print("close failed", file=sys.stderr)
        sys.exit(1)
    Path(os.environ["FAKE_LOG"] + ".closed").touch()
    r = {}
elif a[:2] == ["pane", "get"]:
    if a[2] == "missing":
        print("pane not found", file=sys.stderr)
        sys.exit(1)
    r = {"pane": {"pane_id": a[2]}}
elif a[:2] == ["pane", "split"]:
    r = {"pane": {"pane_id": "w7:p3"}}
elif a[:2] == ["agent", "start"]:
    Path(os.environ["FAKE_LOG"] + ".assignment").write_text(a[-1].split("Прочитай задание ")[1].split(" и выполни")[0])
    if os.environ.get("FAKE_START_GATE"):
        gate = Path(os.environ["FAKE_START_GATE"])
        gate.with_suffix(".entered").touch()
        deadline = time.monotonic() + 6
        while not gate.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
    time.sleep(float(os.environ.get("FAKE_DELAY", "0")))
    if os.environ.get("FAKE_MIN_TIMEOUT") and ("--timeout" not in a or int(a[a.index("--timeout") + 1]) < int(os.environ["FAKE_MIN_TIMEOUT"])):
        print('{"error":{"code":"timeout","message":"timed out waiting for agent startup"}}', file=sys.stderr)
        sys.exit(1)
    if os.environ.get("FAKE_FAIL"):
        print("agent_not_ready: процесс мог запуститься", file=sys.stderr)
        sys.exit(1)
    if os.environ.get("FAKE_START_BAD_JSON"):
        print("{bad json")
        sys.exit(0)
    r = {}
elif a[:2] == ["pane", "current"]:
    if os.environ.get("FAKE_PROBE_ERROR"):
        print('{"error":{"code":"timeout"}}', file=sys.stderr)
        sys.exit(1)
    if os.environ.get("FAKE_PANE_MISSING"):
        print(json.dumps({"error": {"code": "pane_not_found"}}), file=sys.stderr)
        sys.exit(1)
    r = {"pane": {"agent": os.environ.get("FAKE_KIND", "codex") if os.environ.get("FAKE_LIVE") else None}}
elif a[:2] == ["pane", "read"]:
    if os.environ.get("FAKE_LATE_ASSIGNMENT"):
        counter = Path(os.environ["FAKE_LOG"] + ".read-count")
        count = int(counter.read_text()) + 1 if counter.exists() else 1
        counter.write_text(str(count))
        if count == 1:
            print("agent UI is still starting")
            sys.exit(0)
    print(Path(os.environ["FAKE_LOG"] + ".assignment").read_text() if os.environ.get("FAKE_AUTO_ASSIGNMENT") else os.environ.get("FAKE_ASSIGNMENT", "unrelated terminal"))
    sys.exit(0)
else:
    r = {}
print(json.dumps({"result": r}))
'''


class ControlTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        fake = bin_dir / "herdr"
        fake.write_text(FAKE_HERDR)
        fake.chmod(0o755)
        self.log = self.root / "herdr-calls.jsonl"
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('CONTROL_PLANE_', 'HERDR_'))}
        test_config = self.root / 'test-config.json'
        test_profiles = {kind: {complexity: {'model': 'fixture-implementation' if kind == 'implementation' else 'fixture-analysis',
                        'reasoning_effort': 'high' if complexity == 'complex' and kind != 'implementation' else 'medium'}
                        for complexity in ('normal', 'complex')} for kind in ('implementation', 'research', 'reasoning', 'planning')}
        test_config.write_text(json.dumps({'schema_version': 1, 'profiles': test_profiles}))
        self.env = {**self.env, 'CONTROL_PLANE_CONFIG': str(test_config), "CONTROL_PLANE_ROOT": str(self.root), "HERDR_ENV": "1", "HERDR_SOCKET_PATH": str(self.log) + ".sock",
                    "FAKE_LOG": str(self.log), "PATH": str(bin_dir) + os.pathsep + os.environ["PATH"]}
        self.cli("project-create", "lab", "--name", "Лаборатория")

    def cli(self, *args, ok=True, env=None):
        result = subprocess.run([sys.executable, str(SCRIPT), *args], env=env or self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0 if ok else 1, result.stdout + result.stderr)
        return json.loads(result.stdout)

    def create(self, key="task1", *extra):
        return self.cli("task-create", key, "--project", "lab", "--title", "Проверка",
                        "--goal", "Проверить", "--acceptance", "Проверки проходят", *extra)

    def wait_run(self, key, state="running"):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            r = json.loads((self.root / "runs" / key / "state.json").read_text())
            if r["status"] == state:
                return r
            time.sleep(0.03)
        self.fail("Не дождались состояния: " + str(r))

    def start(self, key="task1"):
        result = self.cli("start", key)
        return self.wait_run(result["run"])

    def calls(self, raw=False):
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return calls if raw else [c[2:] if c[:1] == ["--session"] else c for c in calls]

    def bind_stale(self):
        self.cli("project-set", "lab", "--workspace", "w7")
        file = self.root / "bindings/projects/lab.json"
        project = json.loads(file.read_text())
        project["workspace"] = "stale"
        file.write_text(json.dumps(project))

    def agent_review_changes(self, status="changes_requested"):
        r = self.review()
        a = self.cli("review-request", "--run", r["id"])
        self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "tester")
        verdict = self.root / "verdict.json"
        verdict.write_text(json.dumps(dict(version=a["version"], status=status, conclusion="Нужны исправления",
            findings=["Исправить ошибку"], suggestions=[], evidence=["Воспроизведено"], limitations=[])))
        self.cli("review-conclude", "--run", r["id"], "--id", a["id"], "--actor", "tester", "--result", str(verdict))
        return r, a

    def test_compact_output_preserves_full_records(self):
        short = self.create('task1', '--compact')
        full = self.cli('show', 'tasks', 'task1')
        self.assertNotIn('acceptance', short)
        self.assertIn('acceptance', full)
        self.assertEqual(short['details'], 'show tasks task1')
        changed = self.cli('task-set', 'task1', '--goal', 'Новая цель', '--compact')
        self.assertNotIn('goal', changed)
        self.assertEqual(self.cli('show', 'tasks', 'task1')['goal'], 'Новая цель')
        r = self.start()
        self.cli('progress', '--run', r['id'], '--text', 'Проверяется')
        short = self.cli('run', r['id'], '--compact')
        self.assertEqual(short['status'], 'running')
        self.assertIn('progress.md', short['files'])
        self.assertNotIn('launch_argv', short)
        self.assertIn('launch_argv', self.cli('run', r['id']))

    def test_memory_pages_include_shared_and_keep_legacy_list(self):
        for project, title in [('lab', 'Тема один'), ('shared', 'Тема два'), ('lab', 'Другое')]:
            self.cli('memory-add', '--project', project, '--title', title, '--text', 'Факт')
        all_rows = self.cli('memory-list', '--project', 'lab', '--query', 'Тема')
        self.assertIsInstance(all_rows, list)
        first = self.cli('memory-list', '--project', 'lab', '--query', 'Тема', '--limit', '1')
        second = self.cli('memory-list', '--project', 'lab', '--query', 'Тема', '--limit', '1', '--offset', str(first['next_offset']))
        self.assertEqual(first['total'], 2)
        self.assertIsNone(second['next_offset'])
        self.assertEqual({m['id'] for m in all_rows}, {m['id'] for m in first['items'] + second['items']})
        self.cli('memory-list', '--project', 'lab', '--offset', '1', ok=False)

    def test_review_selective_reads_are_frozen_and_verify_hashes(self):
        r = self.review()
        a = self.cli('review-request', '--run', r['id'])
        args = ('--run', r['id'], '--id', a['id'])
        frozen = json.loads(Path(a['snapshot']).read_text())
        entry = self.cli('review-read', *args)
        self.assertEqual(entry['task'], frozen['manifest']['task'])
        self.assertEqual(self.cli('review-read', *args, '--section', 'manifest'), frozen)
        files = self.cli('review-read', *args, '--section', 'files')
        source = str(self.root / 'runs' / r['id'] / 'result.md')
        expected = (Path(a['snapshot']).parent / files[source]['blob']).read_text()
        Path(source).write_text('Изменённый живой отчёт')
        blob = self.cli('review-read', *args, '--section', 'blob', '--path', source)
        self.assertEqual(blob['content'], expected)
        self.cli('review-read', *args, '--section', 'blob', '--path', '/outside', ok=False)
        self.cli('review-read', '--run', r['id'], '--id', 'missing', ok=False)
        b = self.cli('review-request', '--run', r['id'])
        latest = self.cli('review-show', '--run', r['id'], '--latest')['attempts']
        self.assertEqual([x['id'] for x in latest], [b['id']])
        self.assertEqual(len(self.cli('review-show', '--run', r['id'])['attempts']), 2)
        (Path(a['snapshot']).parent / files[source]['blob']).write_text('Повреждение')
        self.cli('review-read', *args, ok=False)

    def test_review_selective_code_diff_binary_and_history_freshness_once(self):
        repo, wt, r = self.code_review()
        binary = wt / 'binary'; binary.write_bytes(b'\xff\x00')
        a = self.cli('review-request', '--run', r['id'])
        args = ('--run', r['id'], '--id', a['id'])
        frozen = json.loads(Path(a['snapshot']).read_text())
        self.assertEqual(self.cli('review-read', *args, '--section', 'diff'), frozen['manifest']['repositories'][0])
        self.assertIsNone(self.cli('review-read', *args, '--section', 'blob', '--path', str(binary))['content'])
        self.cli('review-read', *args, '--section', 'diff', '--repository', '-1', ok=False)
        # Exercise the handler with a fake context: all attempts share one observation.
        import importlib.util
        from types import SimpleNamespace
        from unittest.mock import MagicMock, patch
        spec = importlib.util.spec_from_file_location('review_module', SCRIPT.parent / 'agent_review.py')
        module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        c = MagicMock()
        c.read.return_value = {'id': r['id'], 'task': 'task1'}
        history = {'attempts': [dict(id='a', version='v'), dict(id='b', version='old')]}
        with patch.object(module, 'load', return_value=history), patch.object(module, 'snapshot', return_value={'version': 'v'}) as snap:
            module.handle(c, SimpleNamespace(run=r['id'], command='review-show', latest=False))
        snap.assert_called_once()
        self.assertEqual([x['stale'] for x in history['attempts']], [False, True])

    def test_agent_review_fast_finish_before_delivery_ack(self):
        r, a = self.agent_review_changes()
        self.cli("task-set", "task1", "--agent-review-policy", "auto")
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"])
        report = self.root / "fixed.md"; report.write_text("Исправлено")
        out = self.cli("finish", "--run", r["id"], "--report", str(report))
        self.assertEqual(out["agent_review"]["status"], "pending")
        self.assertNotEqual(out["agent_review"]["id"], a["id"])
        for _ in range(2):
            self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--delivered")
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")
        self.assertEqual(self.cli("review-show", "--run", r["id"])["attempts"][-1]["status"], "pending")

    def test_agent_review_identical_resubmission_is_new_version(self):
        r, a = self.agent_review_changes()
        report = self.root / "identical.md"
        report.write_bytes((self.root / "runs" / r["id"] / "result.md").read_bytes())
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"])
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--delivered")
        self.cli("finish", "--run", r["id"], "--report", str(report))
        # A duplicate preparation after finish must not reopen the same cycle.
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"])
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")
        b = self.cli("review-request", "--run", r["id"])
        self.assertNotEqual(a["id"], b["id"])
        self.assertNotEqual(a["version"], b["version"])
        self.assertEqual(b["id"], self.cli("review-request", "--run", r["id"])["id"])
        self.cli("review-claim", "--run", r["id"], "--id", b["id"], "--actor", "tester")

    def test_agent_review_failed_requires_user_request(self):
        r, a = self.agent_review_changes(status="failed")
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], ok=False)
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--user-requested")
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "running")
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--delivered")
        report = self.root / "fixed.md"; report.write_text("Исправлено по поручению")
        self.cli("finish", "--run", r["id"], "--report", str(report))
        # Duplicate preparation/ACK cannot reopen a completed submission.
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--user-requested")
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--delivered")
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")

    def test_agent_review_reservations_limit_without_delivery_ack(self):
        r, a = self.agent_review_changes()
        for cycle in range(2):
            self.cli("review-feedback", "--run", r["id"], "--id", a["id"])
            self.cli("review-feedback", "--run", r["id"], "--id", a["id"])
            report = self.root / "same.md"
            report.write_bytes((self.root / "runs" / r["id"] / "result.md").read_bytes())
            self.cli("finish", "--run", r["id"], "--report", str(report))
            a = self.cli("review-request", "--run", r["id"])
            self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "tester")
            verdict = self.root / "verdict.json"
            verdict.write_text(json.dumps(dict(version=a["version"], status="changes_requested",
                conclusion="Ошибка осталась", findings=["Исправить ошибку"], suggestions=[], evidence=["Повторено"], limitations=[])))
            self.cli("review-conclude", "--run", r["id"], "--id", a["id"], "--actor", "tester", "--result", str(verdict))
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], ok=False)
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--user-requested")
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "running")

    def test_agent_review_legacy_prepared_delivery(self):
        r, a = self.agent_review_changes()
        directory = self.root / "runs" / r["id"]
        state = json.loads((directory / "state.json").read_text())
        state.pop("result_revision")
        (directory / "state.json").write_text(json.dumps(state))
        history = json.loads((directory / "agent-review.json").read_text())
        old = history["attempts"][-1]
        frozen = json.loads(Path(a["snapshot"]).read_text())
        frozen["manifest"].pop("submission")
        old["version"] = frozen["version"] = hashlib.sha256(json.dumps(frozen["manifest"], sort_keys=True, ensure_ascii=False).encode()).hexdigest()
        old["result"]["version"] = old["version"]
        old.pop("submission")
        old["feedback_message"] = "task1: Старые подготовленные замечания"
        Path(a["snapshot"]).write_text(json.dumps(frozen))
        (directory / "agent-review.json").write_text(json.dumps(history))
        reply = self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--delivered")
        self.assertTrue(reply["feedback_migrated"])
        self.assertEqual(reply["feedback_message"], old["feedback_message"])
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "running")

    def test_agent_review_repairs_partial_run_task_write(self):
        r, a = self.agent_review_changes()
        scripts = self.root / "fault_scripts"
        shutil.copytree(SCRIPT.parent, scripts)
        (self.root / "config").mkdir(exist_ok=True)
        shutil.copy2(SCRIPT.parents[1] / "config/defaults.json", self.root / "config/defaults.json")
        injected = scripts / "control.py"
        text = injected.read_text()
        wrapper = ("PROGRAM_ROOT = Path(" + repr(str(SCRIPT.parents[1])) + ")\n_original_save = save\n"
            "def save(file, obj, **kwargs):\n"
            "    if file.parent.name == 'tasks' and obj.get('status') == 'running':\n"
            "        raise OSError('injected task-save failure')\n"
            "    return _original_save(file, obj, **kwargs)\n\n")
        injected.write_text(text.replace('if __name__ == "__main__":', wrapper + 'if __name__ == "__main__":'))
        failed = subprocess.run([sys.executable, str(injected), "review-feedback", "--run", r["id"], "--id", a["id"]],
            env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(failed.returncode, 1, failed.stdout + failed.stderr)
        self.assertIn("injected task-save failure", failed.stdout)
        self.assertEqual(self.cli("run", r["id"])["status"], "running")
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")
        self.cli("review-feedback", "--run", r["id"], "--id", a["id"])
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "running")

    def test_agent_review_no_code_lifecycle_and_feedback(self):
        r = self.review()
        before = len(self.calls())
        a = self.cli("review-request", "--run", r["id"])
        self.assertEqual(a["status"], "pending")
        self.assertEqual(self.cli("review-request", "--run", r["id"])["id"], a["id"])
        result = self.root / "verdict.json"
        def conclude(status, findings):
            result.write_text(json.dumps(dict(version=a["version"], status=status,
                conclusion="Проверено", findings=findings, suggestions=[], evidence=["Самостоятельная проверка"], limitations=[])))
            return self.cli("review-conclude", "--run", r["id"], "--id", a["id"], "--actor", "agent1", "--result", str(result))
        for cycle in range(3):
            self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "agent1")
            self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "other", ok=False)
            conclude("changes_requested", ["Исправить критерий"])
            if cycle == 2:
                self.cli("review-feedback", "--run", r["id"], "--id", a["id"], ok=False)
                break
            msg = self.cli("review-feedback", "--run", r["id"], "--id", a["id"])
            self.assertTrue(msg["feedback_message"].startswith("task1: "))
            (self.root / "runs" / r["id"] / "result.md").write_text("Изменено исполнителем до подтверждения")
            self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--delivered")
            self.cli("review-feedback", "--run", r["id"], "--id", a["id"], "--delivered")
            report = self.root / "new.md"
            report.write_text("Исправление " + str(cycle))
            self.cli("finish", "--run", r["id"], "--report", str(report))
            a = self.cli("review-request", "--run", r["id"])
        # Review itself never creates herdr tabs or changes the acceptance reviewer.
        self.assertFalse(any(x[:2] == ["tab", "create"] for x in self.calls()[before:]))
        self.assertEqual(self.cli("show", "tasks", "task1")["reviewer"], "user")

    def test_agent_review_expiry_staleness_and_policy(self):
        r = self.review()
        self.cli("task-set", "task1", "--agent-review-policy", "off")
        self.cli("review-request", "--run", r["id"], ok=False)
        self.cli("task-set", "task1", "--agent-review-policy", "manual")
        a = self.cli("review-request", "--run", r["id"])
        self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "lost")
        f = self.root / "runs" / r["id"] / "agent-review.json"
        state = json.loads(f.read_text()); state["attempts"][-1]["deadline"] = "2000-01-01T00:00:00+00:00"
        f.write_text(json.dumps(state))
        self.assertEqual(self.cli("review-show", "--run", r["id"])["attempts"][-1]["status"], "failed")
        b = self.cli("review-request", "--run", r["id"])
        self.assertNotEqual(a["id"], b["id"])
        self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "lost", ok=False)
        self.cli("review-claim", "--run", r["id"], "--id", b["id"], "--actor", "new")
        result = self.root / "verdict.json"
        result.write_text(json.dumps(dict(version=b["version"], status="passed", conclusion="OK",
            findings=[], suggestions=[], evidence=["Проверено"], limitations=[])))
        (self.root / "runs" / r["id"] / "result.md").write_text("changed")
        verdict = self.cli("review-conclude", "--run", r["id"], "--id", b["id"], "--actor", "new", "--result", str(result))
        self.assertEqual(verdict["status"], "failed")
        self.assertTrue(self.cli("review-show", "--run", r["id"])["attempts"][-1]["stale"])
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")

    def test_agent_review_code_dirty_untracked_and_passed(self):
        repo, wt, r = self.code_review()
        a = self.cli("review-request", "--run", r["id"])
        self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "agent")
        result = self.root / "verdict.json"
        result.write_text(json.dumps(dict(version=a["version"], status="passed", conclusion="OK",
            findings=[], suggestions=[], evidence=["Тесты"], limitations=[])))
        self.assertEqual(self.cli("review-conclude", "--run", r["id"], "--id", a["id"], "--actor", "agent", "--result", str(result))["status"], "passed")
        self.assertFalse(self.cli("review-show", "--run", r["id"])["attempts"][-1]["stale"])
        (wt / "new-file").write_text("untracked")
        self.assertTrue(self.cli("review-show", "--run", r["id"])["attempts"][-1]["stale"])
        b = self.cli("review-request", "--run", r["id"])
        self.assertNotEqual(a["version"], b["version"])
        (wt / "new-file").write_text("dirty")
        self.cli("review-claim", "--run", r["id"], "--id", b["id"], "--actor", "agent", ok=False)

    def test_agent_review_stale_pending_and_symlink(self):
        self.create()
        r = self.start()
        target = self.root / "target.txt"; target.write_text("outside")
        link = self.root / "artifact-link"; link.symlink_to(target)
        report = self.root / "report.md"; report.write_text("Отчёт")
        self.cli("finish", "--run", r["id"], "--report", str(report), "--artifact", str(link))
        a = self.cli("review-request", "--run", r["id"])
        snapshot = json.loads(Path(a["snapshot"]).read_text())
        self.assertEqual(snapshot["manifest"]["files"][str(link)]["kind"], "symlink")
        self.assertNotIn(str(target), snapshot["manifest"]["files"])
        link.unlink(); link.symlink_to(self.root / "different")
        b = self.cli("review-request", "--run", r["id"])
        self.assertNotEqual(a["id"], b["id"])
        self.assertNotEqual(a["version"], b["version"])
        attempts = self.cli("review-show", "--run", r["id"])["attempts"]
        self.assertEqual(attempts[0]["status"], "failed")
        self.assertTrue(attempts[0]["stale"])

    def test_agent_review_corruption_and_late_artifacts(self):
        r = self.review()
        artifact = self.root / "late.txt"; artifact.write_text("result")
        a = self.cli("review-request", "--run", r["id"], "--artifact", str(artifact))
        frozen = json.loads(Path(a["snapshot"]).read_text())
        item = frozen["manifest"]["files"][str(artifact)]
        (Path(a["snapshot"]).parent / item["blob"]).write_text("tampered")
        self.cli("review-claim", "--run", r["id"], "--id", a["id"], "--actor", "agent", ok=False)
        self.assertEqual(self.cli("review-show", "--run", r["id"])["attempts"][-1]["status"], "failed")
        b = self.cli("review-request", "--run", r["id"])
        self.assertNotEqual(a["id"], b["id"])

    def test_agent_review_auto_finish_artifacts(self):
        self.create("task1", "--agent-review-policy", "auto")
        r = self.start()
        artifact = self.root / "artifact.txt"; artifact.write_text("actual result")
        report = self.root / "report.md"; report.write_text("Отчёт")
        out = self.cli("finish", "--run", r["id"], "--report", str(report), "--artifact", str(artifact))
        self.assertEqual(out["agent_review"]["status"], "pending")
        snap = json.loads(Path(out["agent_review"]["snapshot"]).read_text())
        self.assertIn(str(artifact), snap["manifest"]["files"])
        artifact.write_text("changed")
        self.assertTrue(self.cli("review-show", "--run", r["id"])["attempts"][-1]["stale"])

    def test_execution_profiles_reach_codex_and_run(self):
        cases = (("research", "normal", "fixture-analysis", "medium"),
                 ("research", "complex", "fixture-analysis", "high"),
                 ("planning", "complex", "fixture-analysis", "high"),
                 ("reasoning", "normal", "fixture-analysis", "medium"),
                 ("implementation", "complex", "fixture-implementation", "medium"))
        for index, (kind, complexity, model, effort) in enumerate(cases):
            with self.subTest(kind=kind, complexity=complexity):
                key = "profile" + str(index)
                task = self.create(key, "--task-kind", kind, "--complexity", complexity)
                self.assertEqual((task["model"], task["reasoning_effort"]), (model, effort))
                run = self.start(key)
                self.assertEqual((run["model"], run["reasoning_effort"]), (model, effort))
                agent = [c for c in self.calls() if c[:2] == ["agent", "start"]][-1]
                codex_args = agent[agent.index("--") + 1:]
                self.assertEqual(codex_args[codex_args.index("--model") + 1], model)
                self.assertIn("model_reasoning_effort=" + effort, codex_args)

    def test_profile_change_and_legacy_research_with_code(self):
        self.create()
        updated = self.cli("task-set", "task1", "--task-kind", "research", "--complexity", "complex")
        self.assertEqual((updated["model"], updated["reasoning_effort"]), ("fixture-analysis", "high"))
        updated = self.cli("task-set", "task1", "--task-kind", "implementation")
        self.assertEqual((updated["model"], updated["reasoning_effort"]), ("fixture-implementation", "medium"))
        file = self.root / "tasks/task1.json"
        legacy = json.loads(file.read_text())
        for field in ("task_kind", "complexity", "model", "reasoning_effort"):
            legacy.pop(field)
        legacy.update(title="Исследовать модели и обвязки", code=True)
        file.write_text(json.dumps(legacy))
        # Editing a legacy record resolves its purpose even when it requires code.
        updated = self.cli("task-set", "task1", "--complexity", "complex")
        self.assertEqual((updated["model"], updated["reasoning_effort"]), ("fixture-analysis", "high"))

    def test_legacy_profile_is_selected_at_launch(self):
        self.create()
        file = self.root / "tasks/task1.json"
        legacy = json.loads(file.read_text())
        for field in ("task_kind", "complexity", "model", "reasoning_effort"):
            legacy.pop(field)
        legacy["title"] = "Исследовать распределение ресурсов"
        file.write_text(json.dumps(legacy))
        run = self.start()
        self.assertEqual((run["model"], run["reasoning_effort"]), ("fixture-analysis", "medium"))
        task = self.cli("show", "tasks", "task1")
        self.assertEqual(task["model"], "fixture-analysis")

    def test_open_multiple_tasks_returns_summary_and_copies_plan(self):
        self.create("task1")
        self.create("task2")
        plan = self.root / "tasks/task1/plan.md"
        plan.parent.mkdir(parents=True)
        plan.write_text("Сохранённый план")
        results = self.cli("open", "task1", "task2", "task1")
        self.assertEqual([r["task"] for r in results], ["task1", "task2"])
        for result in results:
            self.assertEqual(result["action"], "started")
            self.assertEqual(result["run_status"], "launching")
            self.assertEqual(result["goal"], "Проверить")
            self.wait_run(result["run"])
        self.assertEqual(Path(results[0]["plan"]).read_text(), "Сохранённый план")
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 2)

    def test_open_live_executor_is_idempotent(self):
        self.create()
        run = self.start()
        self.env.update(FAKE_LIVE="1", FAKE_AUTO_ASSIGNMENT="1")
        for _ in range(2):
            result = self.cli("open", "task1")[0]
            self.assertEqual(result["action"], "existing")
            self.assertEqual(result["run"], run["id"])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)
        self.assertFalse(any(c[:2] == ["agent", "prompt"] for c in self.calls()))

    def test_open_reuses_active_preparer(self):
        self.create()
        self.env["FAKE_DELAY"] = "0.4"
        first = self.cli("open", "task1")[0]
        second = self.cli("open", "task1")[0]
        self.assertEqual(second["action"], "preparing")
        self.assertEqual(first["run"], second["run"])
        self.wait_run(first["run"])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_open_recovers_failed_live_executor(self):
        self.create()
        self.env["FAKE_FAIL"] = "1"
        first = self.cli("open", "task1")[0]
        self.wait_run(first["run"], "launch_failed")
        self.env.update(FAKE_LIVE="1", FAKE_AUTO_ASSIGNMENT="1")
        result = self.cli("open", "task1")[0]
        self.assertEqual(result["action"], "recovered")
        self.assertEqual(result["run"], first["run"])
        self.assertEqual(result["status"], "running")
        self.assertIn("agent_not_ready", self.cli("run", first["run"])["startup_error"])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_open_missing_executor_preserves_previous_context(self):
        self.create()
        self.env["FAKE_FAIL"] = "1"
        first = self.cli("open", "task1")[0]
        self.wait_run(first["run"], "launch_failed")
        directory = self.root / "runs" / first["run"]
        (directory / "plan.md").write_text("Восстановительный план")
        (directory / "checkpoint.md").write_text("Уже выполнено")
        self.env.pop("FAKE_FAIL")
        self.env["FAKE_PANE_MISSING"] = "1"
        result = self.cli("open", "task1")[0]
        run = self.wait_run(result["run"])
        self.assertEqual(result["action"], "restarted")
        self.assertEqual(run["previous_run"], first["run"])
        self.assertEqual(result["previous_checkpoint"], str(directory / "checkpoint.md"))
        self.assertEqual(Path(result["plan"]).read_text(), "Восстановительный план")
        self.assertEqual((directory / "checkpoint.md").read_text(), "Уже выполнено")
        self.assertEqual(self.cli("run", first["run"])["status"], "interrupted")
        self.assertIn("checkpoint.md", (self.root / "runs" / result["run"] / "assignment.md").read_text())

    def test_open_uncertain_start_does_not_restart_or_change_state(self):
        self.create()
        self.env["FAKE_FAIL"] = "1"
        first = self.cli("open", "task1")[0]
        run = self.wait_run(first["run"], "launch_failed")
        result = self.cli("open", "task1", ok=False)[0]
        self.assertEqual(result["action"], "error")
        self.assertEqual(result["run"], first["run"])
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(result["state"], str(self.root / "runs" / first["run"] / "state.json"))
        self.assertEqual(self.cli("run", first["run"])["status"], run["status"])
        self.assertEqual(self.cli("show", "tasks", "task1")["active_run"], first["run"])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_open_blocked_and_review_do_not_launch(self):
        self.create()
        run = self.start()
        self.cli("block", "--run", run["id"], "--question", "Как продолжить?", "--checkpoint", "Сохранено")
        result = self.cli("open", "task1")[0]
        self.assertEqual(result["action"], "awaiting_answer")
        self.assertEqual(result["question"], "Как продолжить?")
        self.assertEqual(result["checkpoint"], "Сохранено")
        self.cli("answer", "task1", "--text", "Продолжай")
        report = self.root / "report.md"
        report.write_text("Готово")
        self.cli("finish", "--run", run["id"], "--report", str(report))
        result = self.cli("open", "task1")[0]
        self.assertEqual(result["action"], "result")
        self.assertEqual(result["status"], "review")
        self.assertEqual(Path(result["result"]).read_text(), "Готово")
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_open_legacy_absent_workspace_recovers_but_existing_refuses(self):
        self.cli("project-set", "lab", "--workspace", "w7")
        project_file = self.root / "projects/lab.json"
        project = self.cli("show", "projects", "lab")
        (self.root / "bindings/projects/lab.json").unlink()
        project.pop("workspace_namespace")
        project_file.write_text(json.dumps(project))
        self.create()
        self.cli("open", "task1", ok=False)
        self.assertFalse((self.root / "runs").exists())
        project["workspace"] = "stale"
        project_file.write_text(json.dumps(project))
        result = self.cli("open", "task1")[0]
        self.wait_run(result["run"])
        self.assertEqual(self.cli("show", "projects", "lab")["workspace"], "w7")

    def test_open_error_does_not_prevent_other_selected_tasks(self):
        self.create("good")
        result = self.cli("open", "missing", "good", ok=False)
        self.assertEqual(result[0]["action"], "error")
        self.assertEqual(result[1]["action"], "started")
        self.wait_run(result[1]["run"])

    def test_open_rejects_wrong_namespace_and_unrelated_executor(self):
        self.create()
        run = self.start()
        self.cli("--herdr-session", "other", "open", "task1", ok=False)
        self.env["FAKE_SOCKET_SUFFIX"] = ".changed"
        self.cli("open", "task1", ok=False)
        self.env.pop("FAKE_SOCKET_SUFFIX")
        self.env["FAKE_LIVE"] = "1"
        self.cli("open", "task1", ok=False)
        self.assertEqual(self.cli("show", "tasks", "task1")["active_run"], run["id"])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_open_concurrent_calls_start_only_one_executor(self):
        self.create()
        self.env["FAKE_DELAY"] = "0.6"
        argv = [sys.executable, str(SCRIPT), "open", "task1"]
        processes = [subprocess.Popen(argv, env=self.env, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True) for _ in range(2)]
        results = []
        for process in processes:
            stdout, stderr = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 0, stdout + stderr)
            results.append(json.loads(stdout)[0])
        self.assertEqual({r["action"] for r in results}, {"started", "preparing"})
        self.assertEqual(results[0]["run"], results[1]["run"])
        self.wait_run(results[0]["run"])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_stale_workspace_recovers_both_contexts(self):
        for explicit in (False, True):
            with self.subTest(explicit=explicit):
                self.bind_stale()
                key = "explicit" if explicit else "inherited"
                self.create(key)
                args = ["--herdr-session", "default"] if explicit else []
                result = self.cli(*args, "start", key)
                run = self.wait_run(result["run"])
                self.assertEqual(run["workspace"], "w7")
                project = self.cli("show", "projects", "lab")
                self.assertEqual(project["workspace_namespace"]["session"], "default")
                self.assertEqual(project["workspace"], "w7")

    def test_live_previous_executor_blocks_restart(self):
        self.create()
        run = self.start()
        self.cli("interrupt", "task1", "--reason", "save progress")
        self.env["FAKE_LIVE"] = "1"
        self.cli("start", "task1", ok=False)
        self.assertEqual(self.cli("show", "tasks", "task1")["active_run"], run["id"])

    def test_inherited_unavailable_does_not_claim_or_clear(self):
        self.bind_stale()
        self.create()
        self.env["FAKE_UNAVAILABLE"] = "1"
        self.cli("start", "task1", ok=False)
        self.assertEqual(self.cli("show", "projects", "lab")["workspace"], "stale")
        self.assertFalse((self.root / "runs").exists())

    def test_wrong_namespace_refuses_before_workspace_get(self):
        self.cli("project-set", "lab", "--workspace", "w7")
        self.create()
        before = len(self.log.read_text().splitlines())
        self.cli("--herdr-session", "other", "start", "task1", ok=False)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()[before:]]
        self.assertFalse(any("workspace" in c for c in calls))

    def test_workspace_disappears_after_preflight(self):
        self.cli("project-set", "lab", "--workspace", "w7")
        self.create()
        self.env["FAKE_DISAPPEAR"] = "1"
        run = self.start()
        self.assertEqual(run["workspace_recovery"]["reason"], "workspace_not_found")
        self.assertEqual(sum(c[:2] == ["workspace", "create"] for c in self.calls()), 1)

    def test_get_timeout_and_protocol_error_preserve_binding(self):
        self.bind_stale()
        self.create()
        for extra in ({"FAKE_GET_ERROR": "timeout"}, {"FAKE_GET_ERROR": "permission_denied"},
                      {"FAKE_BAD_PROTOCOL": "1"}):
            with self.subTest(extra=extra):
                self.cli("start", "task1", env={**self.env, **extra}, ok=False)
                self.assertEqual(self.cli("show", "projects", "lab")["workspace"], "stale")
                self.assertFalse((self.root / "runs").exists())
        self.assertFalse(any(c[:2] == ["workspace", "create"] for c in self.calls()))

    def test_legacy_binding_requires_explicit_evidence(self):
        self.cli("project-set", "lab", "--workspace", "w7")
        project_file = self.root / "projects/lab.json"
        project = self.cli("show", "projects", "lab")
        (self.root / "bindings/projects/lab.json").unlink()
        project.pop("workspace_namespace")
        project_file.write_text(json.dumps(project))
        self.create()
        self.cli("start", "task1", ok=False)
        self.assertFalse((self.root / "runs").exists())
        # A previous explicitly selected server is valid migration evidence.
        previous = self.root / "runs/old/state.json"
        previous.parent.mkdir(parents=True)
        previous.write_text(json.dumps({"id": "old", "project": "lab", "workspace": "w7", "herdr_session": "default"}))
        run = self.start()
        self.assertEqual(run["herdr_namespace"]["session"], "default")

    def test_partial_create_blocks_same_and_other_task(self):
        for kind in ("workspace", "tab"):
            with self.subTest(kind=kind):
                # Recreate the isolated store for each layout operation.
                if kind == "tab":
                    self.temp.cleanup()
                    self.setUp()
                    self.cli("project-set", "lab", "--workspace", "w7")
                self.create()
                self.create("task2")
                self.env["FAKE_CREATE_FAIL" if kind == "workspace" else "FAKE_TAB_FAIL"] = "1"
                result = self.cli("start", "task1")
                run = self.wait_run(result["run"], "launch_failed")
                self.assertTrue(Path(str(self.log) + ".created").exists())
                self.cli("interrupt", "task1", "--reason", "inspect failed request")
                self.cli("start", "task1", ok=False)
                self.cli("start", "task2", ok=False)
                self.assertEqual(self.cli("show", "projects", "lab")["layout_pending"], run["id"])
                self.assertEqual(sum(c[:2] == [kind, "create"] for c in self.calls()), 1)

    def test_two_tasks_recover_one_workspace(self):
        self.bind_stale()
        self.create("task1")
        self.create("task2")
        processes = [subprocess.Popen([sys.executable, str(SCRIPT), "start", key],
                     env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                     for key in ("task1", "task2")]
        for process in processes:
            out, err = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 0, out + err)
            self.wait_run(json.loads(out)["run"])
        calls = self.calls()
        self.assertEqual(sum(c[:2] == ["workspace", "create"] for c in calls), 1)
        self.assertEqual(sum(c[:2] == ["tab", "create"] for c in calls), 1)
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in calls), 2)

    def test_recover_interrupted_live_executor_without_resubmission(self):
        self.create()
        run = self.start()
        self.cli("interrupt", "task1", "--reason", "save context")
        self.env.update(FAKE_LIVE="1", FAKE_AUTO_ASSIGNMENT="1")
        self.cli("start", "task1", ok=False)
        recovered = self.cli("recover-start", "--run", run["id"])
        self.assertEqual(recovered["status"], "running")
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_changed_session_socket_refuses_binding(self):
        self.cli("project-set", "lab", "--workspace", "w7")
        before = self.cli("show", "projects", "lab")
        self.create()
        self.env["FAKE_SOCKET_SUFFIX"] = ".replacement"
        self.cli("--herdr-session", "default", "start", "task1", ok=False)
        self.assertEqual(self.cli("show", "projects", "lab"), before)
        self.assertFalse((self.root / "runs").exists())

    def test_old_boot_pid_does_not_block_but_same_identity_does(self):
        self.create()
        run = self.start()
        self.cli("interrupt", "task1", "--reason", "save context")
        file = self.root / "runs" / run["id"] / "state.json"
        run = json.loads(file.read_text())
        run["worker_pid"] = os.getpid()
        sys.path.insert(0, str(SCRIPT.parent))
        import platform_support
        run["worker_identity"] = platform_support.process_identity(os.getpid())
        file.write_text(json.dumps(run))
        self.cli("start", "task1", ok=False)
        run["worker_identity"]["boot_id"] = "previous-boot"
        file.write_text(json.dumps(run))
        self.start()

    def test_second_launcher_cannot_change_owned_launch(self):
        self.create()
        self.env["FAKE_DELAY"] = "1"
        result = self.cli("start", "task1")
        other = subprocess.run([sys.executable, str(SCRIPT), "_launch", result["run"]],
                               env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(other.returncode, 1)
        self.wait_run(result["run"])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_minimal_project_and_duplicate(self):
        p = self.cli("show", "projects", "lab")
        self.assertEqual(p["repositories"], [])
        self.assertIsNone(p["workspace"])
        self.cli("project-create", "other", "--name", "лаборатория", ok=False)
        self.cli("project-create", "../bad", "--name", "Bad", ok=False)

    def test_no_herdr_does_not_change_task(self):
        self.create()
        self.cli("start", "task1", env={**self.env, "HERDR_ENV": ""}, ok=False)
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "backlog")
        self.assertFalse((self.root / "runs").exists())

    def test_explicit_session_launch_outside_herdr(self):
        self.create()
        env = {**self.env, "HERDR_ENV": ""}
        result = self.cli("--herdr-session", "default", "start", "task1", env=env)
        r = self.wait_run(result["run"])
        self.assertEqual(r["herdr_session"], "default")
        calls = self.calls(raw=True)
        self.assertTrue(calls)
        self.assertTrue(all(c[:2] == ["--session", "default"] for c in calls))
        self.assertTrue(any(c[2:4] == ["agent", "start"] for c in calls))
        # A later worker command must retain the run's session for notifications.
        self.cli("block", "--run", r["id"], "--question", "План согласован?",
                 "--checkpoint", "План сохранён", env=env)
        calls = self.calls(raw=True)
        self.assertEqual(calls[-1][:4], ["--session", "default", "notification", "show"])

    def test_unavailable_explicit_session_does_not_claim_task(self):
        self.create()
        result = self.cli("--herdr-session", "unavailable", "start", "task1",
                          env={**self.env, "HERDR_ENV": ""}, ok=False)
        self.assertIn("server unavailable", result["error"])
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "backlog")
        self.assertFalse((self.root / "runs").exists())

    def test_recover_start_requires_live_agent_and_matching_assignment(self):
        self.create()
        self.env["FAKE_FAIL"] = "1"
        result = self.cli("start", "task1")
        r = self.wait_run(result["run"], "launch_failed")
        deadline = time.monotonic() + 3
        while self.cli("run", r["id"])["launcher_alive"]:
            self.assertLess(time.monotonic(), deadline)
            time.sleep(0.03)
        self.cli("recover-start", "--run", r["id"], ok=False)
        self.env["FAKE_LIVE"] = "1"
        self.cli("recover-start", "--run", r["id"], ok=False)
        self.assertEqual(self.cli("run", r["id"])["status"], "launch_failed")
        self.env["FAKE_ASSIGNMENT"] = str(self.root / "runs" / r["id"] / "assignment.md")
        recovered = self.cli("recover-start", "--run", r["id"])
        self.assertEqual(recovered["status"], "running")
        self.assertIn("agent_not_ready", self.cli("run", r["id"])["startup_error"])
        self.assertEqual(len(self.cli("show", "tasks", "task1")["runs"]), 1)

    def test_background_launch_and_duplicate_claim(self):
        self.create()
        self.env["FAKE_DELAY"] = "1.3"
        before = time.monotonic()
        result = self.cli("start", "task1")
        self.assertLess(time.monotonic() - before, 1.0)
        self.cli("start", "task1", ok=False)
        self.cli("interrupt", "task1", "--reason", "still launching", ok=False)
        r = self.wait_run(result["run"])
        self.assertEqual(r["workspace"], "w7")
        calls = self.calls()
        self.assertTrue(any("--no-focus" in c for c in calls))
        agent = next(c for c in calls if c[:2] == ["agent", "start"])
        self.assertIn("assignment.md", agent[-1])
        if self.env.get("FAKE_KIND") == "claude":
            self.assertEqual(agent[agent.index("--permission-mode") + 1], "default")
        else:
            self.assertEqual(agent[agent.index("--sandbox") + 1], "workspace-write")
        self.assertNotIn("--add-dir", agent)
        self.assertFalse(any(c[:2] == ["agent", "prompt"] for c in calls))

    def test_slow_start_has_longer_readiness_timeout(self):
        self.create()
        self.env["FAKE_MIN_TIMEOUT"] = "120000"
        result = self.cli("start", "task1")
        r = self.wait_run(result["run"])
        self.assertEqual(r["status"], "running")

    def test_start_error_with_matching_live_agent_does_not_notify_failure(self):
        self.create()
        self.env.update(FAKE_FAIL="1", FAKE_LIVE="1", FAKE_AUTO_ASSIGNMENT="1")
        result = self.cli("start", "task1")
        r = self.wait_run(result["run"])
        self.assertIn("agent_not_ready", r["startup_error"])
        self.assertEqual(self.cli("notifications"), [])
        calls = self.calls()
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in calls), 1)

    def test_start_error_before_assignment_becomes_visible(self):
        self.create()
        self.env.update(FAKE_FAIL="1", FAKE_LIVE="1", FAKE_AUTO_ASSIGNMENT="1",
                        FAKE_LATE_ASSIGNMENT="1")
        result = self.cli("open", "task1")[0]
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline:
            run = self.cli("run", result["run"])
            if not run["launcher_alive"]:
                break
            time.sleep(0.03)
        self.assertEqual(run["status"], "running")
        self.assertIn("agent_not_ready", run["startup_error"])
        for _ in range(2):
            reopened = self.cli("open", "task1")[0]
            self.assertEqual(reopened["run"], result["run"])
            self.assertEqual(reopened["action"], "existing")
        self.assertEqual(self.cli("notifications"), [])
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)
        self.assertFalse(any(c[:2] == ["agent", "prompt"] for c in self.calls()))

    def wait_launcher(self, key):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            run = self.cli("run", key)
            if not run["launcher_alive"]:
                return run
            time.sleep(0.03)
        self.fail("Подготовитель не завершился: " + str(run))

    def test_uncertain_start_is_reconciled_without_duplicate_or_failure(self):
        for reason in ("unrelated-output", "transport-error", "malformed-start-response"):
            with self.subTest(reason=reason):
                key = reason
                self.create(key)
                self.env.update(FAKE_FAIL="1", FAKE_LIVE="1")
                if reason == "transport-error":
                    self.env["FAKE_PROBE_ERROR"] = "1"
                elif reason == "malformed-start-response":
                    self.env.pop("FAKE_FAIL")
                    self.env["FAKE_START_BAD_JSON"] = "1"
                opened = self.cli("open", key)[0]
                run = self.wait_launcher(opened["run"])
                self.assertEqual(run["status"], "launching")
                self.assertTrue(run["startup_confirmation_pending"])
                self.assertTrue(run["startup_error"])
                self.assertEqual(self.cli("notifications"), [])
                self.cli("open", key, ok=False)
                self.env.pop("FAKE_PROBE_ERROR", None)
                self.env["FAKE_AUTO_ASSIGNMENT"] = "1"
                recovered = self.cli("open", key)[0]
                self.assertEqual(recovered["run"], run["id"])
                self.assertEqual(recovered["action"], "recovered")
                fresh = self.cli("run", run["id"])
                self.assertTrue(fresh["agent_start_completed"])
                self.assertNotIn("startup_confirmation_pending", fresh)
                self.env.pop("FAKE_AUTO_ASSIGNMENT")
                self.env.pop("FAKE_START_BAD_JSON", None)
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 3)
        self.assertFalse(any(c[:2] == ["agent", "prompt"] for c in self.calls()))

    def test_launcher_preserves_worker_transitions_on_success_and_failure(self):
        for failed in (False, True):
            for action, expected in (("progress", "running"), ("block", "blocked"), ("finish", "review")):
                with self.subTest(failed=failed, action=action):
                    key = action + str(int(failed))
                    self.create(key)
                    gate = self.root / (key + ".release")
                    self.env["FAKE_START_GATE"] = str(gate)
                    if failed:
                        self.env["FAKE_FAIL"] = "1"
                    opened = self.cli("open", key)[0]
                    deadline = time.monotonic() + 4
                    while not gate.with_suffix(".entered").exists():
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(0.01)
                    if action == "progress":
                        self.cli(action, "--run", opened["run"], "--text", "Работа начата")
                    elif action == "block":
                        self.cli(action, "--run", opened["run"], "--question", "Как продолжать?",
                                 "--checkpoint", "Контекст сохранён")
                    else:
                        report = self.root / "report.md"
                        report.write_text("Проверенный результат")
                        self.cli(action, "--run", opened["run"], "--report", str(report))
                    gate.touch()
                    run = self.wait_launcher(opened["run"])
                    self.assertEqual(run["status"], expected)
                    self.assertEqual(self.cli("show", "tasks", key)["status"], expected)
                    self.assertTrue(run["agent_start_completed"])
                    if action == "progress":
                        self.assertEqual(run["progress"], "Работа начата")
                    elif action == "block":
                        question = json.loads((self.root / "runs" / run["id"] / "questions.json").read_text())
                        self.assertEqual(question["question"], "Как продолжать?")
                    else:
                        self.assertEqual((self.root / "runs" / run["id"] / "result.md").read_text(), "Проверенный результат")
        self.assertFalse(any("Не удалось запустить" in n["title"] for n in self.cli("notifications")))

    def test_plan_opens_beside_explicit_pane_without_starting_agent(self):
        self.create()
        file = self.root / "plan with spaces $(touch unwanted).md"
        file.write_text("# План\n\nПроверить настройки")
        view = self.cli("--herdr-session", "default", "plan", "task1", "--pane", "w7:p2",
                        "--file", str(file), env={**self.env, "HERDR_ENV": "", "HERDR_PANE_ID": ""})
        self.assertEqual(view["pane"], "w7:p3")
        calls = [json.loads(line)[2:] for line in self.log.read_text().splitlines()]
        split = next(c for c in calls if c[:2] == ["pane", "split"])
        self.assertEqual(split[split.index("--pane") + 1], "w7:p2")
        self.assertEqual(split[split.index("--direction") + 1], "right")
        self.assertIn("--no-focus", split)
        run = next(c for c in calls if c[:2] == ["pane", "run"])
        self.assertEqual(shlex.split(run[3])[-1], str(file))
        self.assertFalse(any(c[:2] in (["tab", "create"], ["agent", "start"]) for c in calls))
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "backlog")

    def test_plan_uses_caller_and_labels_task_fallback(self):
        self.create()
        view = self.cli("plan", "task1", env={**self.env, "HERDR_PANE_ID": "w7:p2"})
        self.assertEqual(view["parent_pane"], "w7:p2")
        self.assertIn("Отдельный plan.md пока не сохранён", Path(view["file"]).read_text())

    def test_plan_requires_target_and_existing_file_before_split(self):
        self.create()
        env = {**self.env, "HERDR_PANE_ID": ""}
        self.cli("plan", "task1", env=env, ok=False)
        self.cli("plan", "task1", "--pane", "missing", env=env, ok=False)
        self.cli("plan", "task1", "--pane", "w7:p2", "--file", str(self.root / "absent.md"), ok=False)
        calls = self.calls()
        self.assertFalse(any(c[:2] == ["pane", "split"] for c in calls))

    def test_simultaneous_start_only_one_wins(self):
        self.create()
        argv = [sys.executable, str(SCRIPT), "start", "task1"]
        processes = [subprocess.Popen(argv, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) for _ in range(2)]
        outputs = [p.communicate(timeout=10) for p in processes]
        self.assertEqual(sorted(p.returncode for p in processes), [0, 1])
        winner = next(json.loads(out) for p, (out, _) in zip(processes, outputs) if p.returncode == 0)
        self.wait_run(winner["run"])
        self.assertEqual(len(self.cli("show", "tasks", "task1")["runs"]), 1)

    def test_question_saved_without_notification_and_resume(self):
        self.create()
        r = self.start()
        output = self.cli("block", "--run", r["id"], "--question", "Какой сервер?",
                          "--checkpoint", "Проверки завершены", env={**self.env, "HERDR_ENV": ""})
        self.assertTrue(output["notification"]["delivered"])
        self.assertEqual(self.cli("tasks", "--status", "blocked")[0]["id"], "task1")
        calls_before = self.log.read_text()
        output = self.cli("answer", "task1", "--text", "Существующий VPS")
        self.assertEqual(output["run"], r["id"])
        self.assertEqual(output["status"], "running")
        self.assertEqual(output["pane"], r["pane"])
        self.assertEqual(self.log.read_text(), calls_before)
        q = json.loads((self.root / "runs" / r["id"] / "questions.json").read_text())
        self.assertEqual(q["answer"], "Существующий VPS")
        self.assertEqual(len(self.cli("show", "tasks", "task1")["runs"]), 1)
        t = self.cli("show", "tasks", "task1")
        self.assertEqual(t["active_run"], r["id"])
        self.assertEqual(t["status"], "running")
        resumed = self.cli("run", r["id"])
        self.assertEqual(resumed["status"], "running")
        self.assertEqual(resumed["tab"], r["tab"])
        self.cli("start", "task1", ok=False)
        self.cli("progress", "--run", r["id"], "--text", "Продолжение")
        self.cli("block", "--run", r["id"], "--question", "Ещё вопрос?", "--checkpoint", "Продолжили")
        self.cli("answer", "task1", "--text", "Да")
        report = self.root / "report.md"
        report.write_text("Работа после ответа завершена и проверена.")
        self.cli("finish", "--run", r["id"], "--report", str(report))
        self.assertEqual(self.cli("run", r["id"])["status"], "review")
        self.assertEqual(self.cli("show", "tasks", "task1")["runs"], [r["id"]])

    def test_git_worktree_isolated_and_preserved_on_resume(self):
        repo = self.root / "repository"
        repo.mkdir()
        def git(*args):
            subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True)
        git("init")
        (repo / "example.txt").write_text("original")
        git("add", ".")
        git("-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-m", "initial")
        self.cli("project-set", "lab", "--repo", str(repo))
        self.create("task1", "--code")
        r = self.start()
        wt = Path(r["worktrees"][0]["path"])
        calls = self.calls()
        agent = next(c for c in calls if c[:2] == ["agent", "start"])
        if self.env.get("FAKE_KIND") == "claude":
            self.assertEqual(agent[agent.index("--permission-mode") + 1], "default")
        else:
            self.assertEqual(agent[agent.index("--sandbox") + 1], "workspace-write")
        if self.env.get("FAKE_KIND") != "claude":
            self.assertEqual(agent[agent.index("--cd") + 1], str(wt))
        layout = next(c for c in calls if c[:2] in (["workspace", "create"], ["tab", "create"]))
        self.assertEqual(layout[layout.index("--cwd") + 1], str(wt))
        self.assertEqual(r["cwd"], str(wt))
        self.assertEqual(agent[agent.index("--add-dir") + 1], str(self.root))
        (wt / "example.txt").write_text("changed")
        self.assertEqual((repo / "example.txt").read_text(), "original")
        self.cli("block", "--run", r["id"], "--question", "Продолжать?", "--checkpoint", "Изменён example.txt")
        self.cli("answer", "task1", "--text", "Да")
        resumed = self.cli("run", r["id"])
        self.assertEqual(resumed["id"], r["id"])
        self.assertEqual(resumed["worktrees"], r["worktrees"])
        self.assertEqual((wt / "example.txt").read_text(), "changed")

    def test_finish_after_launch_failed_saves_result_for_review(self):
        self.create()
        self.env["FAKE_FAIL"] = "1"
        started = self.cli("start", "task1")
        run = self.wait_run(started["run"], "launch_failed")
        report = self.root / "report.md"
        report.write_text("Исполнитель выполнил работу после ошибки готовности")
        self.cli("finish", "--run", run["id"], "--report", str(report))
        finished = self.cli("run", run["id"])
        self.assertEqual(finished["status"], "review")
        self.assertEqual(finished["startup_error"], run["error"])
        self.assertNotIn("error", finished)
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")
        self.assertEqual((self.root / "runs" / run["id"] / "result.md").read_text(), report.read_text())
        self.assertEqual(len(self.cli("memory-list", "--project", "lab")), 1)
        self.assertTrue(any(n["title"].startswith("Готово к приёмке:") for n in self.cli("notifications")))
        self.assertEqual(sum(c[:2] == ["agent", "start"] for c in self.calls()), 1)

    def test_finish_rejects_inactive_and_closed_runs(self):
        self.create()
        first = self.start()
        report = self.root / "report.md"
        report.write_text("Результат")
        self.cli("interrupt", "task1", "--reason", "Остановлен")
        self.cli("finish", "--run", first["id"], "--report", str(report), ok=False)
        second = self.start()
        self.cli("finish", "--run", first["id"], "--report", str(report), ok=False)
        self.cli("interrupt", "task1", "--reason", "Отмена")
        self.cli("task-set", "task1", "--status", "cancelled")
        self.cli("finish", "--run", second["id"], "--report", str(report), ok=False)
        for run in (first, second):
            self.assertFalse((self.root / "runs" / run["id"] / "result.md").exists())
        self.assertEqual(self.cli("memory-list", "--project", "lab"), [])
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "cancelled")

    def test_finish_memory_and_delegated_acceptance(self):
        self.create()
        r = self.start()
        report = self.root / "report.md"
        report.write_text("Изменения проверены. Ограничения описаны.")
        decisions = self.root / "decisions.md"
        decisions.write_text("Решение и причина выбора.")
        self.cli("finish", "--run", r["id"], "--report", str(report), "--decisions", str(decisions))
        self.assertEqual(len(self.cli("memory-list", "--project", "lab")), 2)
        self.cli("accept", "task1", "--by", "someone", ok=False)
        self.cli("task-set", "task1", "--reviewer", "review-agent")
        self.cli("accept", "task1", "--by", "review-agent")
        self.assertEqual(self.cli("tasks"), [])
        self.assertEqual(self.cli("tasks", "--all")[0]["status"], "done")
        notice = self.cli("notifications")[0]
        self.cli("notice-ack", notice["id"])
        self.assertEqual(self.cli("notifications"), [])

    def review(self):
        self.create()
        r = self.start()
        report = self.root / "report.md"
        report.write_text("Проверенный результат")
        self.cli("finish", "--run", r["id"], "--report", str(report))
        return r

    def test_complete_accepts_and_closes_saved_tab_idempotently(self):
        r = self.review()
        result = self.cli("complete", "task1")
        self.assertEqual(result["status"], "done")
        self.assertEqual(result["cleanup"]["status"], "closed")
        self.assertEqual(self.cli("run", r["id"])["status"], "done")
        self.assertTrue((self.root / "runs" / r["id"] / "result.md").exists())
        self.cli("complete", "task1")
        calls = self.calls()
        self.assertEqual([c for c in calls if c[:2] == ["tab", "close"]],
                         [["tab", "close", r["tab"]]])

    def git(self, repo, *args):
        result = subprocess.run(["git", "-C", str(repo), *args], check=True,
                                capture_output=True, text=True)
        return result.stdout.strip()

    def code_review(self, remote=False, git_mode=None):
        repo = self.root / "repository"
        repo.mkdir()
        self.git(repo, "init", "-b", "main")
        self.git(repo, "config", "user.name", "Test")
        self.git(repo, "config", "user.email", "test@example.com")
        (repo / "example.txt").write_text("original")
        self.git(repo, "add", ".")
        self.git(repo, "commit", "-m", "initial")
        if remote:
            origin = self.root / "origin.git"
            origin.mkdir()
            self.git(origin, "init", "--bare", "-b", "main")
            self.git(repo, "remote", "add", "origin", str(origin))
            self.git(repo, "push", "-u", "origin", "main")
        self.create("task1", "--code", "--repo", str(repo), "--git-mode", git_mode or ("branch" if remote else "main"))
        r = self.start()
        wt = Path(r["worktrees"][0]["path"])
        (wt / "example.txt").write_text("task change")
        (wt / "added.txt").write_text("task artifact")
        report = self.root / "report.md"
        report.write_text("Код проверен и готов к приёмке")
        self.cli("finish", "--run", r["id"], "--report", str(report))
        return repo, wt, r

    def test_complete_requires_message_before_staging_and_can_retry(self):
        repo, wt, r = self.code_review()
        before = self.git(wt, "rev-parse", "HEAD")
        status = self.git(wt, "status", "--porcelain")
        for args in ((), ("--commit-message", "   ")):
            result = self.cli("complete", "task1", *args, ok=False)
            self.assertEqual(result["status"], "review")
            self.assertIn("--commit-message", result["git_completion"]["error"])
            self.assertEqual(self.git(wt, "rev-parse", "HEAD"), before)
            self.assertEqual(self.git(repo, "rev-parse", "HEAD"), before)
            self.assertEqual(self.git(wt, "status", "--porcelain"), status)
            self.assertFalse(any(c[:2] == ["tab", "close"] for c in self.calls()))
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertEqual(self.git(repo, "log", "-1", "--format=%s"),
                         "Update example content and add task artifact")

    def test_complete_keeps_existing_commit_without_message(self):
        repo, wt, r = self.code_review()
        self.git(wt, "add", "--all")
        self.git(wt, "commit", "-m", "Update example content and add task artifact")
        before = self.git(wt, "rev-parse", "HEAD")
        self.cli("complete", "task1")
        self.assertEqual(self.git(repo, "rev-parse", "HEAD"), before)
        self.assertEqual(self.git(wt, "rev-parse", "HEAD"), before)

    def test_complete_without_origin_commits_and_merges_main(self):
        repo, wt, r = self.code_review()
        result = self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertEqual(result["status"], "done")
        self.assertEqual((repo / "example.txt").read_text(), "task change")
        self.assertEqual((repo / "added.txt").read_text(), "task artifact")
        self.assertEqual(self.git(repo, "status", "--porcelain"), "")
        self.assertTrue(wt.exists())
        head = self.git(repo, "rev-parse", "HEAD")
        self.assertEqual(self.git(repo, "log", "-1", "--format=%s"),
                         "Update example content and add task artifact")
        self.cli("complete", "task1")
        self.assertEqual(self.git(repo, "rev-parse", "HEAD"), head)
        self.assertTrue((self.root / "runs" / r["id"] / "git-completion.md").exists())

    def test_complete_origin_pushes_feature_without_local_merge_removes_worktree(self):
        repo, wt, r = self.code_review(remote=True)
        before = self.git(repo, "rev-parse", "HEAD")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        branch = r["worktrees"][0]["branch"]
        origin = self.root / "origin.git"
        feature = self.git(origin, "rev-parse", "refs/heads/" + branch)
        self.assertEqual(feature, self.git(repo, "rev-parse", branch))
        self.assertEqual(self.git(repo, "rev-parse", "HEAD"), before)
        self.assertEqual(self.git(origin, "rev-parse", "main"), before)
        self.assertEqual(self.git(origin, "show", branch + ":example.txt"), "task change")
        self.assertFalse(wt.exists())
        self.assertTrue((self.root / "runs" / r["id"] / "result.md").exists())
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")

    def test_complete_push_rejection_retains_worktree_and_review_then_retries(self):
        repo, wt, r = self.code_review(remote=True)
        hook = self.root / "origin.git" / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        result = self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        self.assertEqual(result["status"], "review")
        self.assertEqual(result["git_completion"]["status"], "failed")
        self.assertTrue(wt.exists())
        self.assertFalse(Path(str(self.log) + ".closed").exists())
        commit = self.git(wt, "rev-parse", "HEAD")
        hook.unlink()
        self.cli("complete", "task1")
        self.assertEqual(self.git(repo, "rev-parse", r["worktrees"][0]["branch"]), commit)
        self.assertFalse(wt.exists())

    def test_complete_dirty_main_preserves_both_trees(self):
        repo, wt, r = self.code_review()
        (repo / "example.txt").write_text("user edit")
        before = self.git(wt, "rev-parse", "HEAD")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        self.assertEqual(self.git(wt, "rev-parse", "HEAD"), before)
        self.assertEqual((repo / "example.txt").read_text(), "user edit")
        self.assertEqual((wt / "example.txt").read_text(), "task change")
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")

    def test_complete_diverged_local_main_merges_and_conflict_aborts(self):
        repo, wt, r = self.code_review()
        (repo / "other.txt").write_text("parallel change")
        self.git(repo, "add", ".")
        self.git(repo, "commit", "-m", "parallel")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertEqual(self.git(repo, "log", "-1", "--format=%s"),
                         "Merge: Update example content and add task artifact")
        self.assertEqual((repo / "other.txt").read_text(), "parallel change")
        self.assertEqual((repo / "example.txt").read_text(), "task change")

    def test_complete_merge_conflict_leaves_review_and_clean_main(self):
        repo, wt, r = self.code_review()
        (repo / "example.txt").write_text("conflicting change")
        self.git(repo, "add", ".")
        self.git(repo, "commit", "-m", "parallel conflicting")
        before = self.git(repo, "rev-parse", "HEAD")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        self.assertEqual(self.git(repo, "rev-parse", "HEAD"), before)
        self.assertEqual(self.git(repo, "status", "--porcelain"), "")
        self.assertTrue(wt.exists())
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")

    def test_complete_worktree_on_main_pushes_main_and_keeps_tree(self):
        repo, wt, r = self.code_review(remote=True)
        # Git permits main in a linked tree when the primary tree is detached.
        self.git(repo, "checkout", "--detach")
        self.git(repo, "branch", "-d", "main")
        self.git(wt, "branch", "-m", "main")
        state_file = self.root / "runs" / r["id"] / "state.json"
        state = json.loads(state_file.read_text())
        state["worktrees"][0]["branch"] = "main"
        state_file.write_text(json.dumps(state))
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertEqual(self.git(self.root / "origin.git", "show", "main:example.txt"), "task change")
        self.assertTrue(wt.exists())
        self.assertEqual(self.cli("run", r["id"])["git_completion"]["status"], "completed")

    def test_complete_temporary_branch_merges_main_and_pushes_origin(self):
        repo, wt, r = self.code_review(remote=True, git_mode="main")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertEqual((repo / "example.txt").read_text(), "task change")
        self.assertEqual(self.git(self.root / "origin.git", "rev-parse", "main"), self.git(repo, "rev-parse", "HEAD"))
        refs = self.git(self.root / "origin.git", "for-each-ref", "--format=%(refname)")
        self.assertNotIn(r["worktrees"][0]["branch"], refs)
        self.assertTrue(wt.exists())

    def test_git_mode_can_be_selected_at_review(self):
        repo, wt, r = self.code_review(remote=True)
        self.cli("task-set", "task1", "--git-mode", "main")
        self.assertEqual(self.cli("show", "tasks", "task1")["git_mode"], "main")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertEqual((repo / "example.txt").read_text(), "task change")

    def test_two_main_tasks_from_same_base_merge_and_push_both(self):
        repo, wt, r = self.code_review(remote=True, git_mode="main")
        self.create("task2", "--code", "--repo", str(repo), "--git-mode", "main")
        second = self.start("task2")
        second_tree = Path(second["worktrees"][0]["path"])
        (second_tree / "second.txt").write_text("second task")
        self.cli("finish", "--run", second["id"], "--report", str(self.root / "report.md"))
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.cli("complete", "task2", "--commit-message", "Add second task file")
        self.assertEqual((repo / "example.txt").read_text(), "task change")
        self.assertEqual((repo / "second.txt").read_text(), "second task")
        self.assertEqual(self.git(self.root / "origin.git", "rev-parse", "main"), self.git(repo, "rev-parse", "HEAD"))

    def test_complete_cancel_keeps_unpublished_code_and_tree(self):
        repo, wt, r = self.code_review(remote=True)
        before = self.git(repo, "rev-parse", "HEAD")
        self.cli("complete", "task1", "--status", "cancelled")
        self.assertTrue(wt.exists())
        self.assertEqual(self.git(wt, "rev-parse", "HEAD"), before)
        self.assertEqual((wt / "example.txt").read_text(), "task change")
        self.assertNotIn("git_completion", self.cli("run", r["id"]))

    def test_complete_recovers_removed_tree_before_checkpoint(self):
        repo, wt, r = self.code_review(remote=True)
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        state_file = self.root / "runs" / r["id"] / "state.json"
        state = json.loads(state_file.read_text())
        state["git_completion"]["status"] = "pending"
        state["git_completion"]["repositories"][0]["removed"] = False
        state_file.write_text(json.dumps(state))
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertTrue(self.cli("run", r["id"])["git_completion"]["repositories"][0]["removed"])

    def test_complete_rejects_edited_tree_after_failed_push(self):
        repo, wt, r = self.code_review(remote=True)
        hook = self.root / "origin.git" / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        hook.unlink()
        (wt / "example.txt").write_text("edit after checkpoint")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        self.assertTrue(wt.exists())
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")

    def test_complete_freezes_delivery_mode_after_first_attempt(self):
        repo, wt, r = self.code_review(remote=True)
        hook = self.root / "origin.git" / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        self.cli("task-set", "task1", "--git-mode", "main")
        hook.unlink()
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        self.assertTrue(wt.exists())
        self.assertEqual((repo / "example.txt").read_text(), "original")
        self.cli("task-set", "task1", "--git-mode", "branch")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.assertFalse(wt.exists())

    def test_complete_preserves_users_unfinished_merge_in_main(self):
        repo, wt, r = self.code_review()
        self.git(repo, "checkout", "-b", "user-branch")
        (repo / "other.txt").write_text("user merge")
        self.git(repo, "add", ".")
        self.git(repo, "commit", "-m", "user branch")
        self.git(repo, "checkout", "main")
        self.git(repo, "merge", "--no-ff", "--no-commit", "user-branch")
        # Empty index still has MERGE_HEAD; complete must not abort this merge.
        self.git(repo, "reset", "HEAD", "--", "other.txt")
        (repo / "other.txt").unlink()
        merge_head = self.git(repo, "rev-parse", "MERGE_HEAD")
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact", ok=False)
        self.assertEqual(self.git(repo, "rev-parse", "MERGE_HEAD"), merge_head)

    def test_complete_failure_retains_status_and_can_retry(self):
        r = self.review()
        self.env["FAKE_CLOSE_FAIL"] = "1"
        result = self.cli("complete", "task1", ok=False)
        self.assertEqual(result["status"], "done")
        self.assertEqual(result["cleanup"]["status"], "failed")
        self.assertEqual(self.cli("run", r["id"])["cleanup"]["status"], "failed")
        del self.env["FAKE_CLOSE_FAIL"]
        self.assertEqual(self.cli("complete", "task1")["cleanup"]["status"], "closed")

    def test_complete_rejects_wrong_reviewer_and_foreign_agents(self):
        self.review()
        self.cli("complete", "task1", "--by", "someone", ok=False)
        self.env["FAKE_OTHER_AGENT"] = "1"
        self.cli("complete", "task1", ok=False)
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")
        calls = self.calls()
        self.assertFalse(any(c[:2] == ["tab", "close"] for c in calls))

    def wait_completion(self, r, status="completed"):
        file = self.root / "runs" / r["id"] / "completion-request.json"
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            request = json.loads(file.read_text())
            if request["status"] == status:
                return request
            time.sleep(0.03)
        self.fail("Не дождались завершения: " + str(request))

    def test_complete_own_tab_detaches_and_closes_without_controller(self):
        r = self.review()
        self.env["HERDR_PANE_ID"] = "w7:p1"
        result = self.cli("complete", "task1", "--note", "Поручение пользователя")
        self.assertEqual(result["status"], "completion_requested")
        request = self.wait_completion(r)
        self.assertEqual(request["note"], "Поручение пользователя")
        self.assertEqual(self.cli("run", r["id"])["cleanup"]["status"], "closed")
        self.cli("complete", "task1")
        self.assertEqual(len([c for c in self.calls() if c[:2] == ["tab", "close"]]), 1)

    def test_complete_own_tab_parallel_requests_do_not_duplicate(self):
        r = self.review()
        self.env.update(HERDR_PANE_ID="w7:p1", FAKE_CLOSE_DELAY="0.4")
        argv = [sys.executable, str(SCRIPT), "complete", "task1"]
        children = [subprocess.Popen(argv, env=self.env, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True) for _ in range(4)]
        for child in children:
            stdout, stderr = child.communicate(timeout=10)
            self.assertEqual(child.returncode, 0, stdout + stderr)
        self.wait_completion(r)
        self.assertEqual(len([c for c in self.calls() if c[:2] == ["tab", "close"]]), 1)

    def test_complete_own_tab_push_failure_retains_code_and_can_retry(self):
        repo, wt, r = self.code_review(remote=True)
        hook = self.root / "origin.git/hooks/pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        self.env["HERDR_PANE_ID"] = "w7:p1"
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.wait_completion(r, "failed")
        self.assertTrue(wt.exists())
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")
        self.assertFalse(any(c[:2] == ["tab", "close"] for c in self.calls()))
        hook.unlink()
        self.cli("complete", "task1")
        self.wait_completion(r)
        self.assertFalse(wt.exists())

    def test_complete_own_tab_main_merges_and_pushes(self):
        repo, wt, r = self.code_review(remote=True, git_mode="main")
        self.env["HERDR_PANE_ID"] = "w7:p1"
        self.cli("complete", "task1", "--commit-message", "Update example content and add task artifact")
        self.wait_completion(r)
        self.assertEqual((repo / "example.txt").read_text(), "task change")
        self.assertEqual(self.git(self.root / "origin.git", "rev-parse", "main"),
                         self.git(repo, "rev-parse", "HEAD"))
        self.assertEqual(self.cli("run", r["id"])["completion_request"]["status"], "completed")
        self.assertEqual(self.git(repo, "log", "-1", "--format=%s"),
                         "Update example content and add task artifact")

    def test_complete_own_tab_changed_namespace_refused(self):
        r = self.review()
        self.env.update(HERDR_PANE_ID="w7:p1", FAKE_SOCKET_SUFFIX=".changed")
        self.cli("complete", "task1", ok=False)
        self.wait_completion(r, "failed")
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")
        self.assertFalse(any(c[:2] == ["tab", "close"] for c in self.calls()))

    def test_complete_own_tab_herdr_failure_keeps_request_and_retries(self):
        r = self.review()
        self.env.update(HERDR_PANE_ID="w7:p1", FAKE_CLOSE_FAIL="1")
        self.cli("complete", "task1")
        self.wait_completion(r, "failed")
        self.assertEqual(self.cli("run", r["id"])["cleanup"]["status"], "failed")
        del self.env["FAKE_CLOSE_FAIL"]
        self.cli("complete", "task1")
        self.wait_completion(r)

    def test_complete_own_tab_cancel_keeps_worktree(self):
        repo, wt, r = self.code_review()
        self.env["HERDR_PANE_ID"] = "w7:p1"
        self.cli("complete", "task1", "--status", "cancelled")
        self.wait_completion(r)
        self.assertTrue(wt.exists())
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "cancelled")

    def test_complete_foreign_pane_in_own_tab_still_refused(self):
        r = self.review()
        self.env.update(HERDR_PANE_ID="w7:p3", FAKE_OTHER_AGENT="1")
        self.cli("complete", "task1", ok=False)
        self.assertFalse((self.root / "runs" / r["id"] / "completion-request.json").exists())
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "review")

    def test_complete_own_tab_unavailable_herdr_preserves_request(self):
        r = self.review()
        self.env.update(HERDR_PANE_ID="w7:p1", FAKE_UNAVAILABLE="1")
        self.cli("complete", "task1", ok=False)
        request = self.wait_completion(r, "failed")
        self.assertIn("unavailable", request["error"])
        del self.env["FAKE_UNAVAILABLE"]
        self.cli("complete", "task1")
        self.wait_completion(r)

    def test_complete_recovers_dead_request(self):
        r = self.review()
        file = self.root / "runs" / r["id"] / "completion-request.json"
        file.write_text(json.dumps(dict(status="running", pid=99999999, identity={},
                                        target_status="done", by="user")))
        self.env["HERDR_PANE_ID"] = "w7:p1"
        self.cli("complete", "task1")
        self.wait_completion(r)

    def test_complete_request_refuses_stale_run(self):
        r = self.review()
        file = self.root / "runs" / r["id"] / "completion-request.json"
        file.write_text(json.dumps(dict(token="test", status="pending", target_status="done",
                                        by="user", note="")))
        task_file = self.root / "tasks/task1.json"
        task = json.loads(task_file.read_text())
        task["active_run"] = "run-other"
        task_file.write_text(json.dumps(task))
        self.cli("_complete-request", r["id"], "test", ok=False)
        self.assertEqual(json.loads(file.read_text())["status"], "failed")
        self.assertFalse(any(c[:2] == ["tab", "close"] for c in self.calls()))

    def test_complete_cancels_running_task_and_backlog(self):
        self.create()
        self.start()
        self.cli("complete", "task1", ok=False)
        result = self.cli("complete", "task1", "--status", "cancelled", "--note", "Отменено пользователем")
        self.assertEqual(result["status"], "cancelled")
        self.assertEqual(result["cleanup"]["status"], "closed")
        self.create("task2")
        self.assertEqual(self.cli("complete", "task2", "--status", "cancelled")["cleanup"], "no_run")

    def test_launch_failure_preserves_layout_and_requires_recovery(self):
        self.create()
        self.env["FAKE_FAIL"] = "1"
        result = self.cli("start", "task1")
        r = self.wait_run(result["run"], "launch_failed")
        self.assertEqual(r["pane"], "w7:p1")
        self.assertIn("agent_not_ready", r["error"])
        self.cli("start", "task1", ok=False)
        self.cli("answer", "task1", "--text", "retry", ok=False)
        self.cli("interrupt", "task1", "--reason", "Проверено: старый процесс остановлен")
        self.env.pop("FAKE_FAIL")
        self.cli("start", "task1", ok=False)
        # Only a confirmed vanished pane makes the uncertain old start harmless.
        self.env["FAKE_PANE_MISSING"] = "1"
        self.start()

    def test_dependencies_prevent_start(self):
        self.create("base")
        self.create("task1", "--depends", "base")
        self.cli("start", "task1", ok=False)
        self.assertEqual(self.cli("show", "tasks", "task1")["status"], "backlog")

    def test_shared_memory_is_searchable_from_project(self):
        self.cli("memory-add", "--project", "shared", "--kind", "decision", "--title", "Обмен через файлы",
                 "--text", "Агенты сохраняют checkpoint сразу")
        rows = self.cli("memory-list", "--project", "lab", "--query", "checkpoint")
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["project"], "shared")
        self.cli("project-create", "shared", "--name", "Shared", ok=False)


if __name__ == "__main__":
    unittest.main()
