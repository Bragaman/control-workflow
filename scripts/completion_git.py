"""Сохранение кода при complete; checkpoint после каждого внешнего действия."""
import json
from pathlib import Path
import subprocess


def git(repo, *args, optional=False):
    result = subprocess.run(["git", "-C", str(repo), *args], text=True,
                            capture_output=True, timeout=60)
    if result.returncode and not optional:
        # Remote diagnostics may contain credentials embedded in a URL.
        raise RuntimeError("Git: операция " + args[0] + " не выполнена (код " + str(result.returncode) + ")")
    return result.stdout.strip() if result.returncode == 0 else ""


def main_branch(repo, saved=None):
    if saved:
        git(repo, "rev-parse", "--verify", "refs/heads/" + saved)
        return saved
    remote = git(repo, "symbolic-ref", "--quiet", "refs/remotes/origin/HEAD", optional=True)
    if remote.startswith("refs/remotes/origin/"):
        branch = remote.removeprefix("refs/remotes/origin/")
        if git(repo, "rev-parse", "--verify", "refs/heads/" + branch, optional=True):
            return branch
    for branch in ("main", "master"):
        if git(repo, "rev-parse", "--verify", "refs/heads/" + branch, optional=True):
            return branch
    branch = git(repo, "symbolic-ref", "--quiet", "--short", "HEAD", optional=True)
    if not branch:
        raise ValueError("Не удалось определить основную ветку: " + str(repo))
    return branch


def snapshot(root, run, persist):
    persist()
    (root / "git-completion.json").write_text(json.dumps(run["git_completion"], ensure_ascii=False, indent=2) + "\n")
    lines = ["# Сохранение кода при завершении", ""]
    for item in run["git_completion"].get("repositories", []):
        lines += ["- Репозиторий: " + item["repository"],
                  "  Ветка: " + item["branch"] + "; основная: " + item["main_branch"],
                  "  Действие: " + item["mode"] + "; commit: " + item.get("commit", "ещё не сохранён"),
                  "  Код сохранён: " + str(item.get("published", False)),
                  "  Worktree удалён: " + str(item.get("removed", False))]
    lines += ["", "Статус: " + run["git_completion"]["status"]]
    if run["git_completion"].get("error"):
        lines += [run["git_completion"]["error"]]
    (root / "git-completion.md").write_text("\n".join(lines) + "\n")


def validate_git_operation(wt):
    wt = Path(wt)
    for ref in ("MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"):
        if git(wt, "rev-parse", "--verify", ref, optional=True):
            raise ValueError("Сначала завершите Git-операцию в worktree: " + str(wt))
    for marker in ("rebase-merge", "rebase-apply"):
        marker_path = git(wt, "rev-parse", "--git-path", marker)
        if (wt / marker_path).exists():
            raise ValueError("Сначала завершите rebase в worktree: " + str(wt))


def validate_worktree(item):
    repo, wt = Path(item["repository"]), Path(item["path"])
    registered = git(repo, "worktree", "list", "--porcelain").split("\n\n")
    expected = "worktree " + str(wt.resolve())
    block = next((b.splitlines() for b in registered if expected in b.splitlines()), None)
    if not block or "branch refs/heads/" + item["branch"] not in block:
        raise ValueError("Worktree не соответствует сохранённой ветке: " + str(wt))
    if not wt.is_dir() or wt.resolve() == repo.resolve():
        raise ValueError("Не найден отдельный worktree задачи: " + str(wt))
    if git(wt, "symbolic-ref", "--quiet", "--short", "HEAD") != item["branch"]:
        raise ValueError("Ветка worktree изменена: " + str(wt))
    validate_git_operation(wt)


def complete_code(task, run, root, persist, commit_message=None):
    """Повторяемая транзакция: commit → merge/push → cleanup, без force."""
    if run.get("git_completion", {}).get("status") == "completed":
        return
    state = run.setdefault("git_completion", {"status": "pending", "repositories": [],
                                             "git_mode": task.get("git_mode", "main")})
    checkpoint = lambda: snapshot(root, run, persist)
    try:
        if state.get("git_mode", task.get("git_mode", "main")) != task.get("git_mode", "main"):
            raise ValueError("Git-режим изменён после начала завершения; сначала разберите сохранённый git_completion")
        if not state["repositories"]:
            for wt in run.get("worktrees", []):
                repo = wt["repository"]
                base = main_branch(repo, wt.get("main_branch"))
                origin = "origin" in git(repo, "remote").splitlines()
                mode = ("push_branch" if task.get("git_mode", "main") == "branch"
                        and origin and wt["branch"] != base else "merge_main")
                state["repositories"].append({**wt, "main_branch": base, "origin": origin, "mode": mode})
            checkpoint()
        message = (commit_message or "").strip()
        # Validate all remaining trees before committing any changes.
        for item in state["repositories"]:
            if item.get("published") and item["mode"] == "push_branch" and not Path(item["path"]).exists():
                # Recover a crash after git worktree remove, before the checkpoint.
                if git(item["repository"], "rev-parse", "refs/heads/" + item["branch"]) != item["commit"]:
                    raise ValueError("Сохранённая ветка изменилась после удаления worktree")
                item["removed"] = True
                checkpoint()
            if not item.get("removed"):
                validate_worktree(item)
                if not item.get("commit") and git(item["path"], "status", "--porcelain") and not message:
                    raise ValueError("Для новых изменений укажите --commit-message с английским описанием сути изменения")
                if item["mode"] == "merge_main" and item["branch"] != item["main_branch"]:
                    validate_git_operation(item["repository"])
                    if git(item["repository"], "status", "--porcelain"):
                        raise ValueError("Основной репозиторий содержит чужие незакоммиченные изменения: " + item["repository"])
                    if git(item["repository"], "symbolic-ref", "--quiet", "--short", "HEAD") != item["main_branch"]:
                        raise ValueError("В основном репозитории должна быть открыта ветка " + item["main_branch"])
        for item in state["repositories"]:
            if item.get("removed"):
                continue
            wt, repo = item["path"], item["repository"]
            if item.get("commit"):
                if git(wt, "status", "--porcelain") or git(wt, "rev-parse", "HEAD") != item["commit"]:
                    raise ValueError("Worktree изменился после начала завершения: " + wt)
            else:
                if git(wt, "status", "--porcelain"):
                    git(wt, "add", "--all")
                    git(wt, "commit", "-m", message)
                item["commit"] = git(wt, "rev-parse", "HEAD")
                checkpoint()
            if not item.get("published"):
                if item["mode"] == "merge_main" and item["branch"] != item["main_branch"]:
                    try:
                        subject = git(wt, "log", "-1", "--format=%s", item["commit"])
                        git(repo, "merge", "--no-edit", "-m", "Merge: " + subject, item["commit"])
                    except RuntimeError:
                        if git(repo, "rev-parse", "--verify", "MERGE_HEAD", optional=True):
                            git(repo, "merge", "--abort")
                        raise
                if item["mode"] == "merge_main":
                    item["main_commit"] = git(repo, "rev-parse", "refs/heads/" + item["main_branch"])
                    git(repo, "merge-base", "--is-ancestor", item["commit"], item["main_commit"])
                if item["origin"]:
                    branch = item["branch"] if item["mode"] == "push_branch" else item["main_branch"]
                    head = item["commit"] if item["mode"] == "push_branch" else item["main_commit"]
                    git(repo, "push", "origin", head + ":refs/heads/" + branch)
                    remote = git(repo, "ls-remote", "--exit-code", "origin", "refs/heads/" + branch)
                    if remote.split()[0] != head:
                        raise ValueError("Не подтверждён commit удалённой ветки " + branch)
                    item["remote_branch"] = branch
                    item["remote_commit"] = head
                item["published"] = True
                checkpoint()
            # Only the remote feature-branch workflow requests worktree deletion.
            # Git itself checks untracked files and locks; never force removal.
            if item["mode"] == "push_branch":
                if Path(wt).exists():
                    git(repo, "worktree", "remove", wt)
                item["removed"] = True
            checkpoint()
        state["status"] = "completed"
        state.pop("error", None)
        checkpoint()
    except (ValueError, RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
        state["status"] = "failed"
        state["error"] = str(exc)
        checkpoint()
        raise
