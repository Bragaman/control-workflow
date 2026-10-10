#!/usr/bin/env python3
"""Файловая контрольная панель. Только стандартная библиотека Python."""
import argparse
import contextlib
import datetime as dt
import fcntl
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import uuid
sys.dont_write_bytecode = True
from completion_git import complete_code, main_branch
import runtime_config
import herdr_codex
import agent_adapters
import platform_support
from artifact_links import relocate_links

PROGRAM_ROOT = runtime_config.PROGRAM_ROOT
ROOT = Path(os.environ.get("CONTROL_PLANE_STATE", os.environ.get("CONTROL_PLANE_ROOT", PROGRAM_ROOT))).resolve()
BINDINGS_ROOT = ROOT / "bindings"
SETTINGS = json.loads((PROGRAM_ROOT / "config/defaults.json").read_text())
MACHINE = {}
CONFIG_PATH = None
PROJECT_BINDING_FIELDS = ("repositories", "workspace", "workspace_namespace", "layout_pending", "cwd")
STATES = ("backlog", "ready", "running", "blocked", "review", "done", "cancelled")
HERDR_SESSION = None
AGENT_START_TIMEOUT_MS = 180000
TASK_KINDS = ("research", "reasoning", "planning", "implementation")


def adapter(run=None):
    return agent_adapters.select((run or {}).get('adapter', SETTINGS['adapter']))


def execution_profile(t):
    """Explicit purpose wins; legacy tasks use their title, independently of worktrees."""
    kind = t.get("task_kind")
    if kind is None:
        title = t["title"].casefold().split("по результатам")[0]
        if any(word in title for word in ("исслед", "эксперимент", "research", "experiment")):
            kind = "research"
        elif any(word in title for word in ("план", "архитектур", "planning", "design")):
            kind = "planning"
        elif any(word in title for word in ("рассуж", "анализ", "оценить", "reasoning", "analysis")):
            kind = "reasoning"
        else:
            kind = "implementation"
    complexity = t.get("complexity", "normal")
    if kind not in TASK_KINDS or complexity not in ("normal", "complex"):
        raise ValueError("Неизвестный тип или сложность задачи")
    return dict(task_kind=kind, complexity=complexity, **{key: SETTINGS["profiles"][kind][complexity][key] for key in ("model", "reasoning_effort")})


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")


def ident(value):
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", value):
        raise ValueError("ID: 1–64 символа a-z, 0-9, _ или -")
    return value


def path(kind, key):
    return ROOT / kind / (ident(key) + ".json")


def read(file):
    obj = json.loads(file.read_text())
    if file.parent in (ROOT / "projects", ROOT / "tasks"):
        kind = file.parent.name
        binding = BINDINGS_ROOT / kind / file.name
        if binding.exists():
            obj.update(json.loads(binding.read_text()))
        obj.setdefault("repositories", [] if kind == "projects" else None)
        if kind == "projects":
            obj.setdefault("workspace", None)
    return obj


def save(file, obj, *, split_bindings=True):
    if split_bindings and file.parent in (ROOT / "projects", ROOT / "tasks"):
        fields = PROJECT_BINDING_FIELDS if file.parent.name == "projects" else ("repositories",)
        binding = {key: obj[key] for key in fields if key in obj}
        save(BINDINGS_ROOT / file.parent.name / file.name, binding)
        obj = {key: value for key, value in obj.items() if key not in fields}
    file.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=file.parent, prefix=".tmp-")
    try:
        with os.fdopen(fd, "w") as out:
            json.dump(obj, out, ensure_ascii=False, indent=2)
            out.write("\n")
            out.flush()
            os.fsync(out.fileno())
        os.replace(tmp, file)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


@contextlib.contextmanager
def lock():
    ROOT.mkdir(parents=True, exist_ok=True)
    with (ROOT / ".control.lock").open("a") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def emit(obj):
    print(json.dumps(obj, ensure_ascii=False))


def command(argv, timeout=60, **kwargs):
    result = subprocess.run(argv, text=True, capture_output=True, timeout=timeout, **kwargs)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip() or str(argv))
    return result.stdout


def herdr_command(*args, session=None, timeout=60):
    selected = session or HERDR_SESSION
    if not selected and os.environ.get("HERDR_ENV") != "1":
        raise ValueError("Укажите --herdr-session NAME или запустите команду внутри herdr")
    return herdr_codex.transport(command, args, selected, timeout, MACHINE.get("herdr_executable", "herdr"))


class HerdrError(RuntimeError):
    def __init__(self, message):
        super().__init__(message)
        try:
            self.code = json.loads(message)["error"]["code"]
        except (ValueError, KeyError, TypeError):
            self.code = None


def herdr(*args, session=None, timeout=60):
    try:
        payload = json.loads(herdr_command(*args, session=session, timeout=timeout))
    except RuntimeError as exc:
        raise HerdrError(str(exc)) from exc
    if "error" in payload:
        raise HerdrError(json.dumps(payload))
    result = payload["result"]
    if not isinstance(result, dict):
        raise ValueError("Некорректный ответ herdr")
    return result


def namespace(session=None):
    selected = session or HERDR_SESSION
    return herdr_codex.namespace(command, selected, os.environ.get("HERDR_SOCKET_PATH"),
                                 platform_support.machine_id(), os.getuid(),
                                 MACHINE.get("herdr_executable", "herdr"))


def check_run_namespace(r):
    saved = r.get("herdr_namespace")
    if saved and namespace(saved["session"]) != saved:
        raise ValueError("Namespace сохранённого запуска изменился; нужна явная сверка")


def workspace_binding(p, ns, *, repair_missing_legacy=False):
    if p.get("layout_pending"):
        raise ValueError("Неизвестен результат создания layout; сначала сверьте запуск " + p["layout_pending"])
    if not p.get("workspace"):
        workspaces = herdr("workspace", "list", session=ns["session"])["workspaces"]
        if not isinstance(workspaces, list):
            raise ValueError("Некорректный список workspace")
        return None
    saved = p.get("workspace_namespace")
    if saved is None:
        # A numeric ID or a unique label alone never establishes its old server.
        evidence = []
        for file in (ROOT / "runs").glob("*/state.json"):
            r = read(file)
            if r.get("project") == p["id"] and r.get("workspace") == p["workspace"]:
                evidence.append(r.get("herdr_namespace") or r.get("herdr_session"))
        if not evidence or any(item != ns and item != ns["session"] for item in evidence):
            if repair_missing_legacy and not evidence:
                try:
                    herdr("workspace", "get", p["workspace"], session=ns["session"])
                except HerdrError as exc:
                    if exc.code != "workspace_not_found":
                        raise
                    p["workspace"] = None
                    p["workspace_namespace"] = ns
                    return None
            raise ValueError("Старая привязка workspace без подтверждённой сессии; используйте project-set --workspace")
    elif saved != ns:
        raise ValueError("Workspace принадлежит другому namespace; нужна явная перепривязка")
    try:
        workspace = herdr("workspace", "get", p["workspace"], session=ns["session"])["workspace"]
        if workspace["workspace_id"] != p["workspace"]:
            raise ValueError("Herdr вернул другой workspace")
    except HerdrError as exc:
        if exc.code != "workspace_not_found":
            raise
        workspace = None
    p["workspace_namespace"] = ns
    return workspace


def launcher_alive(r):
    pid = r.get("worker_pid")
    if not pid_alive(pid):
        return False
    saved = r.get("worker_identity")
    return saved is None or process_identity(pid) == saved


def require_executor_absent(r):
    if launcher_alive(r):
        raise ValueError("Подготовитель ещё работает: " + r["id"])
    check_run_namespace(r)
    if r.get("pane") and not r.get("herdr_namespace") and not r.get("herdr_session"):
        raise ValueError("Не подтверждена сессия прежнего исполнителя: " + r["id"])
    if r.get("layout_intent") and not r.get("pane"):
        raise ValueError("Неизвестен результат создания layout: " + r["id"])
    if not r.get("pane"):
        return
    try:
        pane = herdr("pane", "current", "--pane", r["pane"], session=r.get("herdr_session"))["pane"]
    except HerdrError as exc:
        if exc.code == "pane_not_found":
            return
        raise
    if "agent" not in pane:
        raise ValueError("Herdr не подтвердил состояние прежнего исполнителя")
    if pane["agent"]:
        raise ValueError("Прежний исполнитель ещё работает; продолжите " + r["id"] + " в " + r["pane"])
    if r.get("agent_start_intent") and not r.get("agent_start_completed"):
        raise ValueError("Результат agent start неизвестен; сверьте " + r["id"] + " через recover-start")


def require_old_workspace_idle(p, ns, current_run):
    for file in (ROOT / "runs").glob("*/state.json"):
        old = read(file)
        if old["id"] != current_run and old.get("project") == p["id"] and old.get("workspace") == p["workspace"]:
            if old.get("herdr_namespace") not in (None, ns) or old.get("herdr_session") != ns["session"]:
                raise ValueError("Namespace прежнего запуска не подтверждён: " + old["id"])
            require_executor_absent(old)


def confirms_started_agent(r):
    check_run_namespace(r)
    pane = herdr("pane", "current", "--pane", r["pane"],
                 session=r.get("herdr_session"))["pane"]
    transcript = herdr_command("pane", "read", r["pane"], "--source", "recent-unwrapped",
                              "--lines", "120", session=r.get("herdr_session"))
    assignment = str(run_file(r["id"]).parent / "assignment.md")
    return pane.get("agent") == adapter(r).AGENT_KIND and assignment in transcript


def task(key):
    return read(path("tasks", key))


def run_file(key):
    return ROOT / "runs" / ident(key) / "state.json"


def current(t, key):
    if t.get("active_run") != key:
        raise ValueError("Этот запуск уже не является текущим для задачи")
    return read(run_file(key))


def update_run(r, t):
    r["updated_at"] = t["updated_at"] = now()
    # Run first: interrupted paired writes can be diagnosed from the run file.
    save(run_file(r["id"]), r)
    save(path("tasks", t["id"]), t)


def notification(title, body, r=None):
    event = {"title": title, "body": body, "created_at": now(), "delivered": False}
    try:
        if r:
            check_run_namespace(r)
        herdr("notification", "show", title, "--body", body, "--sound", "request",
              session=r.get("herdr_session") if r else None)
        event["delivered"] = True
    except (ValueError, RuntimeError, subprocess.TimeoutExpired, OSError, KeyError) as exc:
        event["error"] = str(exc)
    key = "notice-" + uuid.uuid4().hex[:12]
    event["run"] = r["id"] if r else None
    save(path("notifications", key), event)
    return event


def project_create(a):
    with lock():
        if a.id == "shared":
            raise ValueError("ID shared зарезервирован для общей памяти")
        file = path("projects", a.id)
        if file.exists():
            raise ValueError("Проект с таким ID уже существует")
        projects = [read(p) for p in (ROOT / "projects").glob("*.json")]
        if any(p["name"].casefold() == a.name.casefold() for p in projects):
            raise ValueError("Проект с таким названием уже существует")
        p = dict(id=a.id, name=a.name, description=a.description, repositories=[],
                 workspace=None, created_at=now())
        save(file, p)
        emit(p)


def project_set(a):
    with lock():
        file = path("projects", a.id)
        p = read(file)
        if a.name is not None:
            p["name"] = a.name
        if a.description is not None:
            p["description"] = a.description
        if a.repo is not None:
            repos = []
            for item in a.repo:
                repo = Path(item).expanduser().resolve()
                top = Path(command(["git", "-C", str(repo), "rev-parse", "--show-toplevel"]).strip()).resolve()
                if top != repo:
                    raise ValueError("Укажите корень репозитория: " + str(top))
                if str(repo) not in repos:
                    repos.append(str(repo))
            p["repositories"] = repos
        if a.workspace is not None:
            # Binding is explicit, never infer the user's focused workspace.
            ns = namespace()
            live = herdr("workspace", "get", a.workspace, session=ns["session"])["workspace"]
            p["workspace"] = live["workspace_id"]
            p["workspace_namespace"] = ns
        if a.cwd is not None:
            cwd = runtime_config.location(a.cwd)
            if not cwd.is_dir():
                raise ValueError("Не найден рабочий каталог: " + str(cwd))
            p["cwd"] = str(cwd)
        if a.clear_workspace:
            p["workspace"] = None
            p["workspace_namespace"] = None
        save(file, p)
        emit(p)


def task_create(a):
    with lock():
        read(path("projects", a.project))
        file = path("tasks", a.id)
        if file.exists():
            raise ValueError("Такая задача уже существует")
        for dep in a.depends:
            task(dep)
            if dep == a.id:
                raise ValueError("Задача не может зависеть от себя")
        t = dict(id=a.id, project=a.project, title=a.title, goal=a.goal,
                 acceptance=a.acceptance, status="backlog", code=a.code,
                 repositories=a.repo, depends=a.depends, reviewer="user", git_mode=a.git_mode or SETTINGS["git_mode"],
                 active_run=None, runs=[], created_at=now(), updated_at=now())
        t.update(execution_profile({**t, "task_kind": a.task_kind, "complexity": a.complexity}))
        save(file, t)
        emit(t)


def task_set(a):
    with lock():
        t = task(a.id)
        if t["status"] in ("running", "blocked"):
            raise ValueError("Для текущего запуска используйте progress/block/answer/interrupt")
        for attr in ("title", "goal", "acceptance", "reviewer", "git_mode"):
            value = getattr(a, attr)
            if value is not None:
                t[attr] = value
        if a.code is not None:
            t["code"] = a.code
        if a.repo is not None:
            t["repositories"] = [str(Path(repo).expanduser().resolve()) for repo in a.repo]
        if a.task_kind is not None:
            t["task_kind"] = a.task_kind
        if a.complexity is not None:
            t["complexity"] = a.complexity
        t.update(execution_profile(t))
        if a.status is not None:
            if a.status == "done":
                raise ValueError("Для приёмки используйте accept")
            t["status"] = a.status
        t["updated_at"] = now()
        save(path("tasks", a.id), t)
        emit(t)


def listing(a):
    rows = [read(p) for p in sorted((ROOT / a.kind).glob("*.json"))]
    if a.kind == "tasks":
        rows = [t for t in rows if (not a.project or t["project"] == a.project)
                and (not a.status or t["status"] == a.status)
                and (a.all or t["status"] not in ("done", "cancelled"))]
        rows = [{k: t[k] for k in ("id", "project", "title", "status", "active_run")} for t in rows]
    else:
        rows = [{k: p[k] for k in ("id", "name", "description", "workspace")} for p in rows]
    emit(rows)


def start(a):
    require_herdr_context()
    with lock():
        emit(start_locked(a.id))


def require_herdr_context():
    if not HERDR_SESSION and os.environ.get("HERDR_ENV") != "1":
        raise ValueError("Укажите --herdr-session NAME или запустите команду внутри herdr. Состояние задачи не меняется")


def start_locked(key, *, reopen=False, repair_missing_legacy=False):
    t = task(key)
    if t["status"] not in ("backlog", "ready") and not reopen:
        raise ValueError("Запуск разрешён для backlog/ready. Для blocked сохраните answer и продолжите текущий запуск; для прерванной — interrupt")
    for dep in t["depends"]:
        if task(dep)["status"] != "done":
            raise ValueError("Не завершена зависимость: " + dep)
    p = read(path("projects", t["project"]))
    ns = namespace()
    previous = read(run_file(t["active_run"])) if t.get("active_run") else None
    if previous:
        require_executor_absent(previous)
    workspace = workspace_binding(p, ns, repair_missing_legacy=repair_missing_legacy)
    if p.get("workspace") and workspace is None:
        require_old_workspace_idle(p, ns, None)
    save(path("projects", p["id"]), p)
    repos = t["repositories"] if t["repositories"] is not None else p["repositories"]
    if t["code"] and not repos:
        raise ValueError("Для изменения кода сначала укажите репозиторий проекта или задачи")
    repos = [str(Path(repo).expanduser().resolve()) for repo in repos] if t["code"] else []
    for repo in repos:
        top = Path(command(["git", "-C", repo, "rev-parse", "--show-toplevel"]).strip()).resolve()
        if str(top) != repo:
            raise ValueError("Укажите корень репозитория: " + str(top))
        command(["git", "-C", repo, "rev-parse", "--verify", "HEAD"])
    key = "run-" + uuid.uuid4().hex[:12]
    previous = read(run_file(t["active_run"])) if t.get("active_run") else None
    r = dict(id=key, task=t["id"], project=p["id"], status="launching", created_at=now(),
             updated_at=now(), repositories=repos, worktrees=[], previous_run=t.get("active_run"),
             workspace=None, tab=None, pane=None, worker_pid=None, herdr_session=ns["session"], herdr_namespace=ns)
    t.update(execution_profile(t))
    r.update({k: t[k] for k in ("task_kind", "complexity", "model", "reasoning_effort")})
    r.update(adapter=adapter().ADAPTER, capabilities=adapter().CAPABILITIES,
             access_policy=adapter().ACCESS_POLICY, program_root=str(PROGRAM_ROOT),
             state_root=str(ROOT), bindings_root=str(BINDINGS_ROOT),
             git_mode=t.get("git_mode", "main"))
    # Continue in the same worktrees; retain uncommitted changes and branch history.
    if previous and previous.get("worktrees"):
        if [w["repository"] for w in previous["worktrees"]] != repos[:len(previous["worktrees"])]:
            raise ValueError("Набор репозиториев изменился. Сначала явно разберите прежние worktree")
        r["worktrees"] = previous["worktrees"]
    directory = run_file(key).parent
    directory.mkdir(parents=True)
    plan = run_file(previous["id"]).parent / "plan.md" if previous else ROOT / "tasks" / t["id"] / "plan.md"
    if not plan.exists():
        plan = ROOT / "tasks" / t["id"] / "plan.md"
    if plan.exists():
        shutil.copyfile(plan, directory / "plan.md")
    if reopen and previous:
        previous["status"] = "interrupted"
        previous["interruption"] = "open: подтверждено отсутствие исполнителя, контекст сохранён для нового запуска"
        previous["updated_at"] = now()
        save(run_file(previous["id"]), previous)
    t["active_run"] = key
    t["runs"].append(key)
    t["status"] = "running"
    update_run(r, t)
    with (directory / "launcher.log").open("a") as log:
        try:
            launcher = [sys.executable, str(Path(__file__).resolve())]
            launcher.extend(["--herdr-session", ns["session"]])
            worker = subprocess.Popen([*launcher, "_launch", key],
                                      stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                      start_new_session=True, env=runtime_config.child_environment(ROOT, BINDINGS_ROOT, CONFIG_PATH))
        except OSError as exc:
            r["status"] = "launch_failed"
            r["error"] = str(exc)
            t["status"] = "blocked"
            update_run(r, t)
            raise
    r["worker_pid"] = worker.pid
    r["worker_identity"] = process_identity(worker.pid)
    update_run(r, t)
    return {"run": key, "status": "launching", "state": str(run_file(key))}


def opening_summary(t, r, action):
    directory = run_file(r["id"]).parent if r else ROOT / "tasks" / t["id"]
    result = dict(task=t["id"], title=t["title"], status=t["status"], goal=t["goal"], action=action)
    if r:
        result.update(run=r["id"], run_status=r["status"], state=str(run_file(r["id"])),
                      workspace=r.get("workspace"), tab=r.get("tab"), pane=r.get("pane"))
        for field in ("model", "reasoning_effort"):
            if field in r:
                result[field] = r[field]
        if r.get("previous_run"):
            result["previous_run"] = r["previous_run"]
            checkpoint = run_file(r["previous_run"]).parent / "checkpoint.md"
            if checkpoint.exists():
                result["previous_checkpoint"] = str(checkpoint)
        if r.get("error"):
            result["error"] = r["error"]
        if r.get("progress"):
            result["progress"] = r["progress"]
    for name in ("plan.md", "checkpoint.md", "result.md"):
        if (directory / name).exists():
            result[name.removesuffix(".md")] = str(directory / name)
    if r and (directory / "questions.json").exists():
        question = read(directory / "questions.json")
        if not question.get("answer"):
            result["question"] = question.get("question")
            result["checkpoint"] = question.get("checkpoint")
    return result


def open_task_locked(key):
    t = task(key)
    r = read(run_file(t["active_run"])) if t.get("active_run") else None
    if t["status"] in ("done", "cancelled", "review"):
        return opening_summary(t, r, "result")
    if r and r["status"] == "blocked":
        return opening_summary(t, r, "awaiting_answer")
    if r:
        selected = namespace()
        saved = r.get("herdr_namespace")
        if ((saved and saved != selected)
                or (not saved and r.get("herdr_session") != selected["session"])):
            raise ValueError("Запуск принадлежит другому или неподтверждённому namespace")
    # Even an unfinished layout belongs to the active preparer; never start another.
    if r and launcher_alive(r):
        check_run_namespace(r)
        return opening_summary(t, r, "preparing")
    if r and r.get("pane"):
        check_run_namespace(r)
        pane = None
        try:
            pane = herdr("pane", "current", "--pane", r["pane"], session=r.get("herdr_session"))["pane"]
        except HerdrError as exc:
            if exc.code != "pane_not_found":
                raise
        if pane is not None:
            if "agent" not in pane:
                raise ValueError("Herdr не подтвердил состояние прежнего исполнителя")
            if pane["agent"]:
                if pane["agent"] != adapter(r).AGENT_KIND or not confirms_started_agent(r):
                    raise ValueError("Терминал не подтверждает исполнителя с заданием этого запуска")
                action = "existing"
                if r["status"] in ("launch_failed", "interrupted", "launching"):
                    mark_start_recovered(r, t)
                    action = "recovered"
                return opening_summary(t, r, action)
    if t["status"] not in ("backlog", "ready", "running", "blocked"):
        raise ValueError("Открытие недоступно для статуса " + t["status"])
    # start_locked rechecks absence under this same lock before changing state.
    start_locked(key, reopen=bool(r), repair_missing_legacy=True)
    t = task(key)
    r = read(run_file(t["active_run"]))
    return opening_summary(t, r, "restarted" if r.get("previous_run") else "started")


def open_tasks(a):
    require_herdr_context()
    results = []
    for key in dict.fromkeys(a.ids):
        try:
            with lock():
                results.append(open_task_locked(key))
        except (ValueError, RuntimeError, OSError, KeyError, subprocess.TimeoutExpired) as exc:
            failed = {"task": key, "action": "error", "error": str(exc),
                      "error_code": getattr(exc, "code", None) or "panel_error",
                      "next_action": "Прочитайте файлы указанного запуска и docs/recovery.md программы"}
            try:
                with lock():
                    t = task(key)
                    failed.update(title=t["title"], status=t["status"], goal=t["goal"])
                    if t.get("active_run"):
                        r = read(run_file(t["active_run"]))
                        failed.update(run=r["id"], run_status=r["status"], state=str(run_file(r["id"])),
                                      tab=r.get("tab"), pane=r.get("pane"))
            except (ValueError, OSError, KeyError):
                pass
            results.append(failed)
    emit(results)
    return int(any(item["action"] == "error" for item in results))


def launch(a):
    key = a.run
    try:
        with lock():
            r = read(run_file(key))
            t = task(r["task"])
            current(t, key)
            if (r["status"] != "launching" or r.get("worker_pid") != os.getpid()
                    or r.get("worker_identity") != process_identity(os.getpid())):
                raise ValueError("Запуск уже подготовлен или принадлежит другому подготовителю")
            check_run_namespace(r)
        if len(r["worktrees"]) < len(r["repositories"]):
            for index in range(len(r["worktrees"]), len(r["repositories"])):
                repo = r["repositories"][index]
                wt = runtime_config.location(MACHINE.get("worktrees_root", ROOT / "worktrees")) / t["id"] / (str(index) + "-" + key)
                branch = "control/" + t["id"] + "/" + key
                wt.parent.mkdir(parents=True, exist_ok=True)
                command(["git", "-C", repo, "worktree", "add", "-b", branch, str(wt), "HEAD"])
                r["worktrees"].append(dict(repository=repo, path=str(wt), branch=branch,
                                          main_branch=main_branch(repo)))
                with lock():
                    fresh = current(task(r["task"]), key)
                    if fresh["status"] != "launching":
                        raise ValueError("Подготовка остановлена: " + fresh["status"])
                    fresh["worktrees"] = r["worktrees"]
                    r = fresh
                    t = task(r["task"])
                    update_run(r, t)
        for wt in r["worktrees"]:
            if not Path(wt["path"]).is_dir():
                raise ValueError("Не найден сохранённый worktree: " + wt["path"])
        project = read(path("projects", t["project"]))
        cwd = r["worktrees"][0]["path"] if r["worktrees"] else str(runtime_config.location(project.get("cwd", MACHINE.get("cwd", ROOT))))
        if not Path(cwd).is_dir():
            raise ValueError("Не найден рабочий каталог: " + cwd)
        with lock():
            r = current(task(r["task"]), key)
            t = task(r["task"])
            if r["status"] != "launching":
                raise ValueError("Подготовка остановлена: " + r["status"])
            p = read(path("projects", t["project"]))
            ns = r["herdr_namespace"]
            workspace = workspace_binding(p, ns)
            if p.get("workspace") and workspace is None:
                require_old_workspace_idle(p, ns, key)
            r["layout_intent"] = dict(kind="tab" if workspace else "workspace", old_workspace=p.get("workspace"))
            p["layout_pending"] = key
            save(path("projects", p["id"]), p)
            update_run(r, t)
            if workspace:
                r["workspace"] = workspace["workspace_id"]
                update_run(r, t)
                layout = herdr("tab", "create", "--workspace", r["workspace"], "--cwd", cwd,
                               "--label", t["id"], "--no-focus")
            else:
                layout = herdr("workspace", "create", "--cwd", cwd, "--label", p["name"], "--no-focus")
                r["workspace"] = layout["workspace"]["workspace_id"]
            r["tab"] = layout["tab"]["tab_id"]
            r["pane"] = layout["root_pane"]["pane_id"]
            if not all(isinstance(r[item], str) and r[item] for item in ("workspace", "tab", "pane")):
                raise ValueError("Herdr не подтвердил созданный layout")
            if not workspace and p.get("workspace"):
                r["workspace_recovery"] = dict(old=p["workspace"], new=r["workspace"], reason="workspace_not_found")
            p["workspace"] = r["workspace"]
            p["workspace_namespace"] = ns
            update_run(r, t)
            p.pop("layout_pending", None)
            save(path("projects", p["id"]), p)
        if "workspace" in layout:
            herdr("tab", "rename", r["tab"], t["id"])
        # Freeze the selected profile in this run, including recovery of legacy runs.
        if "model" not in r or "reasoning_effort" not in r:
            r.update(execution_profile(t))
        assignment = assignment_text(r, t)
        file = run_file(key).parent / "assignment.md"
        file.write_text(assignment)
        args = adapter(r).start_args(r, t, cwd, file, ROOT, PROGRAM_ROOT / "docs/worker.md", AGENT_START_TIMEOUT_MS)
        with lock():
            r = current(task(r["task"]), key)
            if r["status"] != "launching":
                raise ValueError("Подготовка остановлена: " + r["status"])
            r["agent_start_intent"] = True
            r["cwd"] = cwd
            r["launch_argv"] = args
            r["adapter"] = adapter(r).ADAPTER
            r["capabilities"] = adapter(r).CAPABILITIES
            if "requested_agent_session_id" not in r and adapter(r).AGENT_KIND == "claude":
                r["requested_agent_session_id"] = args[args.index("--session-id") + 1]
            update_run(r, task(r["task"]))
        try:
            herdr(*args, timeout=AGENT_START_TIMEOUT_MS / 1000 + 30)
        except (RuntimeError, subprocess.TimeoutExpired) as exc:
            # A readiness error can arrive after the initial assignment started.
            try:
                confirmed = confirms_started_agent(r)
            except (ValueError, RuntimeError, subprocess.TimeoutExpired, OSError, KeyError):
                confirmed = False
            if not confirmed:
                raise exc
            with lock():
                fresh = current(task(r["task"]), key)
                fresh["startup_error"] = str(exc)
                update_run(fresh, task(r["task"]))
        with lock():
            fresh = read(run_file(key))
            t = task(r["task"])
            current(t, key)
            fresh["agent_start_completed"] = True
            # The agent can block/finish before startup detection returns.
            if fresh["status"] == "launching":
                fresh["status"] = "running"
            update_run(fresh, t)
    except Exception as exc:
        with lock():
            r = read(run_file(key))
            t = task(r["task"])
            if t.get("active_run") == key and r["status"] == "launching" and r.get("worker_pid") == os.getpid():
                r["status"] = "launch_failed"
                r["error"] = str(exc)
                t["status"] = "blocked"
                checkpoint = run_file(key).parent / "startup-checkpoint.md"
                checkpoint.write_text("# Ошибка подготовки\n\n" + str(exc) + "\n\n"
                                      + "Состояние и намерения сохранены в state.json. Не повторяйте create/start вслепую. "
                                      + "Сверьте layout и исполнителя в сохранённом namespace. "
                                      + "При живом агенте с assignment этого запуска используйте recover-start.\n")
                r["checkpoint"] = str(checkpoint)
                update_run(r, t)
                notification("Не удалось запустить " + t["id"], str(exc), r)
        print(str(exc), file=sys.stderr)
        return 1
    return 0


def assignment_text(r, t):
    cli = shlex.join([sys.executable, str(Path(__file__).resolve()), "--state-root", str(ROOT),
                      "--bindings-root", str(BINDINGS_ROOT),
                      *(["--config", str(CONFIG_PATH)] if CONFIG_PATH else [])])
    directory = run_file(r["id"]).parent
    previous = run_file(r["previous_run"]).parent if r.get("previous_run") else None
    return f"""# Задача {t['id']}: {t['title']}

Цель: {t['goal']}

Критерии готовности: {t['acceptance']}

Состояние панели: {ROOT}
Ресурсы программы: {PROGRAM_ROOT}
Запуск: {r['id']}
Адаптер: {r.get('adapter', herdr_codex.ADAPTER)}
Модель: {r.get('model', 'не сохранена')}; reasoning effort: {r.get('reasoning_effort', 'не сохранён')}.
Проект: {path('projects', t['project'])}
Рабочие деревья: {json.dumps(r['worktrees'], ensure_ascii=False)}
Завершение Git: {t.get('git_mode', 'main')}; контракт: {PROGRAM_ROOT / 'docs/git-policy.md'}
Приёмка: {t['reviewer']}.

До работы прочитай контракт исполнителя: {PROGRAM_ROOT / 'docs/worker.md'}.
Правила планирования: {PROGRAM_ROOT / 'docs/planning.md'}.
Прочитай {directory / 'plan.md'}, если он есть; иначе {ROOT / 'tasks' / t['id'] / 'plan.md'}.
Предыдущий запуск: {previous or 'нет'}.
При восстановлении сначала прочитай checkpoint.md предыдущего запуска,
затем его state.json, questions.json, progress.md и result.md, если они есть.
Артефакты и checkpoint текущего запуска сохраняй в {directory}.

Команда панели (из любого cwd): {cli}
В progress/block/finish/memory-add указывай --run {r['id']}.
Для памяти: memory-list --project {t['project']} --query 'тема'; читай только нужные записи.
Для завершения по явному поручению пользователя: {PROGRAM_ROOT / 'skills/control-complete/SKILL.md'}.
Самостоятельный finish оставляет review.
"""


def recover_start(a):
    with lock():
        r = read(run_file(a.run))
        t = task(r["task"])
        current(t, a.run)
        if r["status"] not in ("launch_failed", "interrupted"):
            raise ValueError("Восстановление старта доступно только для launch_failed/interrupted")
        if launcher_alive(r):
            raise ValueError("Подготовитель ещё работает")
        if not confirms_started_agent(r):
            raise ValueError("Терминал не подтверждает исполнителя с заданием этого запуска")
        mark_start_recovered(r, t)
        emit({"run": r["id"], "status": "running", "pane": r["pane"]})


def mark_start_recovered(r, t):
    r["agent_start_completed"] = True
    if "error" in r:
        r["startup_error"] = r.pop("error")
    r["status"] = t["status"] = "running"
    r["progress"] = "Старт восстановлен: агент и путь задания подтверждены в созданном терминале"
    update_run(r, t)


def plan_view(a):
    t = task(a.id)
    target = a.pane or os.environ.get("HERDR_PANE_ID")
    if not target:
        raise ValueError("Для просмотра вне herdr укажите --herdr-session NAME и --pane ID нужного таба")
    # Validate target before creating either a file or a pane; never use UI focus.
    herdr("pane", "get", target)
    directory = run_file(t["active_run"]).parent if t.get("active_run") else ROOT / "tasks" / t["id"]
    if a.file:
        file = Path(a.file).expanduser().resolve()
        if not file.is_file():
            raise ValueError("Не найден файл плана: " + str(file))
    elif (directory / "plan.md").is_file():
        file = directory / "plan.md"
    else:
        # The task and checkpoint are a starting point, not an approved plan.
        content = f"# {t['id']}: {t['title']}\n\nСтатус: {t['status']}\n\nОтдельный plan.md пока не сохранён. Ниже цель, критерии и checkpoint задачи.\n\n## Цель\n\n{t['goal']}\n\n## Критерии готовности\n\n{t['acceptance']}\n"
        questions = directory / "questions.json"
        if questions.is_file():
            q = read(questions)
            content += f"\n## Checkpoint\n\n{q['checkpoint']}\n\n## Вопросы\n\n{q['question']}\n"
            if q.get("answer"):
                content += f"\n## Ответ\n\n{q['answer']}\n"
        directory.mkdir(parents=True, exist_ok=True)
        file = directory / "plan-view.md"
        file.write_text(content)
    pager = shutil.which("less") or shutil.which("more")
    if not pager:
        raise ValueError("Для просмотра установите less или more")
    layout = herdr("pane", "split", "--pane", target, "--direction", "right",
                   "--cwd", str(ROOT), "--no-focus")
    pane = layout["pane"]["pane_id"]
    # Persist the created pane before submitting anything; failure must not cause a blind retry.
    view = dict(task=t["id"], pane=pane, parent_pane=target, file=str(file),
                herdr_session=HERDR_SESSION, created_at=now())
    save(ROOT / "views" / t["id"] / "state.json", view)
    herdr("pane", "rename", pane, "План: " + t["id"])
    herdr("pane", "run", pane, shlex.join([pager, "--", str(file)]))
    emit(view)


def progress(a):
    with lock():
        r = read(run_file(a.run))
        t = task(r["task"])
        current(t, a.run)
        if r["status"] not in ("launching", "running"):
            raise ValueError("Запуск не выполняется")
        file = run_file(a.run).parent / "progress.md"
        with file.open("a") as out:
            out.write("\n- " + now() + ": " + a.text + "\n")
        r["progress"] = a.text
        update_run(r, t)
        emit({"run": a.run, "progress": a.text})


def block(a):
    with lock():
        r = read(run_file(a.run))
        t = task(r["task"])
        current(t, a.run)
        if r["status"] not in ("launching", "running"):
            raise ValueError("Запуск не выполняется")
        checkpoint = run_file(a.run).parent / "checkpoint.md"
        checkpoint.write_text(a.checkpoint + "\n")
        save(run_file(a.run).parent / "questions.json",
             {"question": a.question, "checkpoint": a.checkpoint, "checkpoint_path": str(checkpoint),
              "answer": None, "created_at": now()})
        r["checkpoint"] = str(checkpoint)
        r["status"] = t["status"] = "blocked"
        update_run(r, t)
        event = notification("Нужен ответ: " + t["title"], a.question, r)
        emit({"run": a.run, "status": "blocked", "notification": event})


def answer(a):
    with lock():
        t = task(a.id)
        if t["status"] != "blocked":
            raise ValueError("Задача не заблокирована")
        r = current(t, t["active_run"])
        if r["status"] == "launch_failed":
            raise ValueError("После ошибки запуска используйте interrupt с объяснением; проверьте созданную панель перед повтором")
        file = run_file(r["id"]).parent / "questions.json"
        q = read(file)
        q["answer"] = a.text
        q["answered_at"] = now()
        save(file, q)
        r["status"] = t["status"] = "running"
        update_run(r, t)
        emit({"task": t["id"], "run": r["id"], "status": "running",
              "pane": r.get("pane"), "questions": str(file),
              "next": "Продолжите текущий ход исполнителя в сохранённой панели, прочитав questions.json и checkpoint"})


def memory_write(project, title, content, kind, run=None, source=None):
    key = "mem-" + uuid.uuid4().hex[:12]
    file = ROOT / "memory" / ident(project) / (key + ".md")
    file.parent.mkdir(parents=True, exist_ok=True)
    if source:
        content = relocate_links(content, Path(source).resolve(), file)
    file.write_text("# " + title + "\n\n" + content + "\n")
    save(ROOT / "memory" / project / (key + ".json"),
         dict(id=key, project=project, title=title, kind=kind, run=run,
              created_at=now(), file=str(file.relative_to(ROOT))))
    return str(file)


def memory_add(a):
    with lock():
        if a.project != "shared":
            read(path("projects", a.project))
        if a.run:
            r = read(run_file(a.run))
            current(task(r["task"]), a.run)
            if a.project != "shared" and r["project"] != a.project:
                raise ValueError("Проект не совпадает с запуском")
        content = Path(a.file).read_text() if a.file else a.text
        emit({"file": memory_write(a.project, a.title, content, a.kind, a.run, a.file)})


def memory_list(a):
    rows = []
    projects = list(dict.fromkeys(["shared", ident(a.project)]))
    for project in projects:
        for file in sorted((ROOT / "memory" / project).glob("*.json")):
            m = read(file)
            if not a.query or a.query.casefold() in (m["title"] + " " + (ROOT / m["file"]).read_text()).casefold():
                rows.append(m)
    emit(rows)


def finish(a):
    report = Path(a.report).read_text()
    decisions = Path(a.decisions).read_text() if a.decisions else ""
    if not report.strip():
        raise ValueError("Отчёт не может быть пустым")
    with lock():
        r = read(run_file(a.run))
        t = task(r["task"])
        current(t, a.run)
        # A readiness failure can be recorded while the executor keeps working.
        # Its report for the current run is sufficient to submit the result.
        if r["status"] not in ("running", "launching", "launch_failed"):
            raise ValueError("Запуск не выполняется")
        result_file = run_file(a.run).parent / "result.md"
        report = relocate_links(report, Path(a.report).resolve(), result_file)
        result_file.write_text(report)
        memory_write(t["project"], t["title"] + ": выполненная работа", report, "work", a.run, result_file)
        if decisions.strip():
            memory_write(t["project"], t["title"] + ": решения", decisions, "decision", a.run, a.decisions)
        if r["status"] == "launch_failed" and "error" in r:
            r["startup_error"] = r.pop("error")
        r["status"] = t["status"] = "review"
        update_run(r, t)
        notification("Готово к приёмке: " + t["title"], str(run_file(a.run).parent / "result.md"), r)
        emit({"task": t["id"], "status": "review"})


def accept_task(t, r, by, note):
    if t["status"] != "review":
        raise ValueError("Приёмка доступна только для review")
    if by != t["reviewer"]:
        raise ValueError("Принимает назначенный reviewer: " + t["reviewer"])
    t["status"] = r["status"] = "done"
    t["accepted"] = dict(by=by, note=note, at=now())
    update_run(r, t)


def accept(a):
    with lock():
        t = task(a.id)
        if t["status"] != "review":
            raise ValueError("Приёмка доступна только для review")
        r = current(t, t["active_run"])
        accept_task(t, r, a.by, a.note)
        emit(t)


def saved_tab(r):
    check_run_namespace(r)
    try:
        return herdr("tab", "get", r["tab"], session=r.get("herdr_session"))["tab"]
    except RuntimeError as exc:
        try:
            code = json.loads(str(exc))["error"]["code"]
        except (ValueError, KeyError, TypeError):
            raise exc
        if code != "tab_not_found":
            raise
        return None


def completion_request_file(r):
    return run_file(r["id"]).parent / "completion-request.json"


def request_alive(request):
    pid = request.get("pid")
    if not pid or not request.get("identity") or not pid_alive(pid):
        return False
    return platform_support.process_running(pid) and request["identity"] == process_identity(pid)


def queue_completion(a, r):
    """Called under the panel lock after verifying the caller and tab ownership."""
    file = completion_request_file(r)
    request = dict(token=uuid.uuid4().hex, run=r["id"], task=a.id,
                   status="pending", requested_at=now(), by=a.by, note=a.note,
                   target_status=a.status)
    save(file, request)
    env = runtime_config.child_environment(ROOT, BINDINGS_ROOT, CONFIG_PATH)
    # The child is independent of the terminal being closed and uses explicit IDs.
    for key in ("HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"):
        env.pop(key, None)
    argv = [sys.executable, str(Path(__file__).resolve()), "_complete-request",
            r["id"], request["token"]]
    try:
        with file.with_suffix(".log").open("a") as output:
            child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=output,
                                     stderr=subprocess.STDOUT, env=env, cwd=ROOT,
                                     start_new_session=True, close_fds=True)
        request.update(pid=child.pid, identity=process_identity(child.pid))
        save(file, request)
    except OSError as exc:
        request.update(status="failed", error=str(exc), finished_at=now())
        save(file, request)
        raise
    emit({"task": a.id, "run": r["id"], "status": "completion_requested",
          "request": str(file), "next": "Итог сохранится в completion-request.json и state.json; при сбое повторите complete"})
    sys.stdout.flush()


def complete_request(a):
    # Serialize with ordinary complete; refuse stale requests after a new run starts.
    with lock():
        r = read(run_file(a.run))
        file = completion_request_file(r)
        request = read(file)
        if request["token"] != a.token or request["status"] != "pending":
            raise ValueError("Запрос завершения заменён или уже обработан")
        request.update(status="running", started_at=now())
        save(file, request)
        try:
            current(task(r["task"]), a.run)
            args = argparse.Namespace(id=r["task"], status=request["target_status"],
                                      by=request["by"], note=request["note"])
            result = complete_locked(args)
            request["status"] = "failed" if result else "completed"
            if result:
                request["error"] = "Завершение не выполнено; проверьте git_completion и cleanup в state.json"
        except (ValueError, RuntimeError, OSError, KeyError, subprocess.TimeoutExpired) as exc:
            request.update(status="failed", error=str(exc))
            result = 1
        request["finished_at"] = now()
        save(file, request)
        emit(request)
        return result


def complete(a):
    with lock():
        t = task(a.id)
        if t.get("active_run"):
            r = current(t, t["active_run"])
            file = completion_request_file(r)
            if file.exists():
                request = read(file)
                if request["status"] in ("pending", "running"):
                    if request_alive(request):
                        if (a.status, a.by) != (request["target_status"], request["by"]):
                            raise ValueError("Уже выполняется другой запрос завершения")
                        emit({"task": a.id, "status": "completion_requested", "request": str(file)})
                        return
                    request.update(status="interrupted", finished_at=now(),
                                   error="Процесс завершения исчез; повторный complete восстанавливает операцию")
                    save(file, request)
        try:
            return complete_locked(a)
        except (ValueError, RuntimeError, OSError, KeyError, subprocess.TimeoutExpired) as exc:
            if t.get("active_run") and os.environ.get("HERDR_PANE_ID") == r.get("pane"):
                # Preserve even a failed preflight (e.g. an unavailable socket).
                # An existing request retains its identity and diagnostics.
                if not file.exists() or read(file)["status"] not in ("pending", "running"):
                    save(file, dict(token=uuid.uuid4().hex, run=r["id"], task=a.id,
                                    status="failed", target_status=a.status, by=a.by,
                                    note=a.note, requested_at=now(), finished_at=now(),
                                    error=str(exc)))
            raise


def complete_locked(a):
    # Keep the current run fixed throughout acceptance and terminal cleanup.
    t = task(a.id)
    r = current(t, t["active_run"]) if t.get("active_run") else None
    if t["status"] in ("done", "cancelled") and t["status"] != a.status:
        raise ValueError("Задача уже завершена с другим статусом")
    if a.status == "done":
        if t["status"] not in ("review", "done"):
            raise ValueError("Сначала нужен результат в review; для отмены используйте --status cancelled")
        if a.by != t["reviewer"]:
            raise ValueError("Принимает назначенный reviewer: " + t["reviewer"])
    if r and r["status"] == "launching" and launcher_alive(r):
        raise ValueError("Подготовитель ещё работает; завершение повторите после подготовки запуска")
    if r and r.get("herdr_session") and HERDR_SESSION and r["herdr_session"] != HERDR_SESSION:
        raise ValueError("Используйте сохранённую сессию запуска: " + r["herdr_session"])
    tab = None
    if r and r.get("tab"):
        tab = saved_tab(r)
        if tab:
            if tab["workspace_id"] != r["workspace"]:
                raise ValueError("Таб не принадлежит сохранённому workspace")
            panes = herdr("pane", "list", "--workspace", r["workspace"],
                          session=r.get("herdr_session"))["panes"]
            members = [p for p in panes if p["tab_id"] == r["tab"]]
            if not any(p["pane_id"] == r["pane"] for p in members):
                raise ValueError("Сохранённая панель исполнителя отсутствует в табе; проверьте его вручную")
            if any(p.get("agent") and p["pane_id"] != r["pane"] for p in members):
                raise ValueError("В табе есть другой агент; сначала отделите его от таба задачи")
            caller = os.environ.get("HERDR_PANE_ID")
            if any(p["pane_id"] == caller for p in members):
                if caller != r["pane"]:
                    raise ValueError("Текущий агент находится в чужой панели закрываемого таба")
                return queue_completion(a, r)

    if r and a.status == "done":
        try:
            complete_code(t, r, run_file(r["id"]).parent, lambda: update_run(r, t))
        except (ValueError, RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
            emit({"task": t["id"], "status": t["status"], "git_completion": r["git_completion"],
                  "next": "Код и таб сохранены; устраните причину и повторите complete"})
            return 1
    if t["status"] != a.status:
        if a.status == "done":
            accept_task(t, r, a.by, a.note)
        else:
            t["status"] = "cancelled"
            t["cancelled"] = dict(note=a.note, at=now())
            if r:
                r["status"] = "cancelled"
                update_run(r, t)
            else:
                t["updated_at"] = now()
                save(path("tasks", t["id"]), t)
    if not r:
        emit({"task": t["id"], "status": t["status"], "cleanup": "no_run"})
        return
    cleanup = dict(tab=r.get("tab"), pane=r.get("pane"), at=now(), status="pending")
    r["cleanup"] = cleanup
    update_run(r, t)
    try:
        if tab:
            herdr("tab", "close", r["tab"], session=r.get("herdr_session"))
            if saved_tab(r) is not None:
                raise RuntimeError("herdr не подтвердил закрытие таба")
        cleanup["status"] = "closed" if r.get("tab") else "no_tab"
    except (ValueError, RuntimeError, OSError, KeyError, subprocess.TimeoutExpired) as exc:
        cleanup["status"] = "failed"
        cleanup["error"] = str(exc)
        update_run(r, t)
        emit({"task": t["id"], "status": t["status"], "cleanup": cleanup,
              "next": "Статус сохранён; проверьте herdr и повторите complete для этого же статуса"})
        return 1
    update_run(r, t)
    emit({"task": t["id"], "run": r["id"], "status": t["status"], "cleanup": cleanup})


def interrupt(a):
    with lock():
        t = task(a.id)
        if t["status"] not in ("running", "blocked"):
            raise ValueError("Нет активной/заблокированной работы")
        r = current(t, t["active_run"])
        if r["status"] == "launching" and launcher_alive(r):
            raise ValueError("Подготовитель ещё работает. Сначала дождитесь или остановите его; PID: " + str(r["worker_pid"]))
        r["status"] = "interrupted"
        r["interruption"] = a.reason
        t["status"] = "ready"
        update_run(r, t)
        emit({"task": t["id"], "status": "ready", "warning": "Процесс herdr не остановлен. Перед повторным запуском убедитесь, что прежний исполнитель прекратил работу"})


def materialize_bindings(a):
    """Reversible export for the legacy reader before a program rollback."""
    with lock():
        count = 0
        for kind in ("projects", "tasks"):
            for file in sorted((ROOT / kind).glob("*.json")):
                save(file, read(file), split_bindings=False)
                count += 1
        emit({"records": count, "state_root": str(ROOT), "bindings_preserved": True})


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--state-root", help="Каталог состояния экземпляра")
    p.add_argument("--config", help="JSON пользовательских настроек")
    p.add_argument("--bindings-root", help="Каталог привязок этой машины")
    p.add_argument("--herdr-session", help="Явная именованная сессия herdr; позволяет запуск вне терминала herdr")
    sub = p.add_subparsers(dest="command", required=True)
    def cmd(name, func):
        c = sub.add_parser(name)
        c.set_defaults(func=func)
        return c
    cmd("environment", lambda a: emit(dict(program_root=str(PROGRAM_ROOT), state_root=str(ROOT),
        bindings_root=str(BINDINGS_ROOT), config=str(CONFIG_PATH) if CONFIG_PATH else None,
        adapter=SETTINGS["adapter"], git_mode=SETTINGS["git_mode"],
        documents={name: str(PROGRAM_ROOT / "docs" / name) for name in
                   ("worker.md", "planning.md", "git-policy.md", "recovery.md")})))
    cmd("materialize-bindings", materialize_bindings)
    c = cmd("project-create", project_create)
    c.add_argument("id"); c.add_argument("--name", required=True); c.add_argument("--description", default="")
    c = cmd("project-set", project_set)
    c.add_argument("id"); c.add_argument("--name"); c.add_argument("--description")
    c.add_argument("--cwd", help="Рабочий каталог проекта для задач без worktree")
    c.add_argument("--repo", action="append"); c.add_argument("--workspace"); c.add_argument("--clear-workspace", action="store_true")
    for name, kind in (("projects", "projects"), ("tasks", "tasks")):
        c = cmd(name, listing); c.set_defaults(kind=kind)
        c.add_argument("--project"); c.add_argument("--status", choices=STATES); c.add_argument("--all", action="store_true")
    c = cmd("task-create", task_create)
    c.add_argument("id"); c.add_argument("--project", required=True); c.add_argument("--title", required=True)
    c.add_argument("--goal", required=True); c.add_argument("--acceptance", required=True)
    c.add_argument("--code", action=argparse.BooleanOptionalAction, default=False); c.add_argument("--repo", action="append")
    c.add_argument("--git-mode", choices=("main", "branch"), default=None,
                   help="main: merge и push основной ветки; branch: push отдельной ветки без merge")
    c.add_argument("--depends", action="append", default=[])
    c.add_argument("--task-kind", choices=TASK_KINDS,
                   help="Назначение задачи; без флага определяется по названию")
    c.add_argument("--complexity", choices=("normal", "complex"), default="normal",
                   help="Сложность для выбора профиля из настроек")
    c = cmd("task-set", task_set)
    c.add_argument("id"); c.add_argument("--title"); c.add_argument("--goal"); c.add_argument("--acceptance")
    c.add_argument("--reviewer"); c.add_argument("--status", choices=("backlog", "ready", "cancelled"))
    c.add_argument("--code", action=argparse.BooleanOptionalAction, default=None)
    c.add_argument("--repo", action="append")
    c.add_argument("--git-mode", choices=("main", "branch"))
    c.add_argument("--task-kind", choices=TASK_KINDS)
    c.add_argument("--complexity", choices=("normal", "complex"))
    c = cmd("show", lambda a: emit(read(path(a.kind, a.id))))
    c.add_argument("kind", choices=("projects", "tasks")); c.add_argument("id")
    c = cmd("start", start); c.add_argument("id")
    c = cmd("open", open_tasks); c.add_argument("ids", nargs="+")
    c = cmd("run", run_show); c.add_argument("id")
    c = cmd("plan", plan_view); c.add_argument("id"); c.add_argument("--file"); c.add_argument("--pane")
    c = cmd("recover-start", recover_start); c.add_argument("--run", required=True)
    c = cmd("_launch", launch); c.add_argument("run")
    c = cmd("progress", progress); c.add_argument("--run", required=True); c.add_argument("--text", required=True)
    c = cmd("block", block); c.add_argument("--run", required=True)
    c.add_argument("--question", required=True); c.add_argument("--checkpoint", required=True)
    c = cmd("answer", answer); c.add_argument("id"); c.add_argument("--text", required=True)
    c = cmd("interrupt", interrupt); c.add_argument("id"); c.add_argument("--reason", required=True)
    c = cmd("finish", finish); c.add_argument("--run", required=True)
    c.add_argument("--report", required=True); c.add_argument("--decisions")
    c = cmd("accept", accept); c.add_argument("id"); c.add_argument("--by", default="user"); c.add_argument("--note", default="")
    c = cmd("complete", complete); c.add_argument("id")
    c.add_argument("--status", choices=("done", "cancelled"), default="done")
    c.add_argument("--by", default="user"); c.add_argument("--note", default="")
    c = cmd("_complete-request", complete_request); c.add_argument("run"); c.add_argument("token")
    c = cmd("memory-add", memory_add); c.add_argument("--project", required=True)
    c.add_argument("--title", required=True); c.add_argument("--kind", choices=("decision", "knowledge", "work"), default="knowledge")
    c.add_argument("--run"); group = c.add_mutually_exclusive_group(required=True)
    group.add_argument("--text"); group.add_argument("--file")
    c = cmd("memory-list", memory_list); c.add_argument("--project", required=True); c.add_argument("--query")
    c = cmd("notifications", lambda a: emit([{"id": p.stem, **read(p)} for p in sorted((ROOT / "notifications").glob("*.json")) if not read(p).get("acknowledged")]))
    c = cmd("notice-ack", notice_ack); c.add_argument("id")
    return p


def notice_ack(a):
    with lock():
        file = path("notifications", a.id)
        event = read(file)
        event["acknowledged"] = now()
        save(file, event)
        emit(event)


def process_identity(pid):
    return platform_support.process_identity(pid)


def pid_alive(pid):
    if not pid:
        return False
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def run_show(a):
    r = read(run_file(a.id))
    request_file = completion_request_file(r)
    if request_file.exists():
        request = read(request_file)
        r["completion_request"] = request
        r["completion_process_alive"] = request_alive(request)
    r["launcher_alive"] = launcher_alive(r)
    r["seconds_since_update"] = int((dt.datetime.now(dt.timezone.utc) - dt.datetime.fromisoformat(r["updated_at"])).total_seconds())
    r["note"] = "launcher_alive относится только к подготовителю; состояние исполнителя подтверждается его файлами, а не этим PID"
    emit(r)


def main():
    global HERDR_SESSION, ROOT, BINDINGS_ROOT, SETTINGS, MACHINE, CONFIG_PATH, AGENT_START_TIMEOUT_MS
    try:
        a = parser().parse_args()
        ROOT, BINDINGS_ROOT, SETTINGS, MACHINE, CONFIG_PATH = runtime_config.load(a.state_root, a.config, a.bindings_root)
        AGENT_START_TIMEOUT_MS = SETTINGS["agent_start_timeout_ms"]
        HERDR_SESSION = a.herdr_session or MACHINE.get("herdr_session")
        return a.func(a) or 0
    except (ValueError, RuntimeError, OSError, KeyError, subprocess.TimeoutExpired) as exc:
        emit({"error": str(exc), "error_code": getattr(exc, "code", None) or "panel_error"})
        return 1


if __name__ == "__main__":
    sys.exit(main())
