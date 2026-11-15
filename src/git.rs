use crate::{args::s, config::Context, external, store};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{path::Path, process::Command, time::Duration};
pub fn git(repo: &Path, args: &[&str], optional: bool) -> Result<String> {
    let (ok, out, _) = external::output(
        Command::new("git").arg("-C").arg(repo).args(args),
        Duration::from_secs(60),
    )?;
    if !ok {
        if optional {
            return Ok(String::new());
        }
        bail!(
            "Git: операция {} не выполнена",
            args.first().unwrap_or(&"unknown")
        );
    }
    Ok(out.trim().into())
}
pub fn add_worktree(
    repo: &Path,
    path: &Path,
    branch: &str,
    lease: &external::ProcessLease,
) -> Result<(bool, String)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .args(["worktree", "add", "-b", branch])
        .arg(path)
        .arg("HEAD");
    lease.inherit(&mut cmd);
    let (ok, _, stderr) = external::output(&mut cmd, Duration::from_secs(60))?;
    Ok((ok, stderr.trim().to_owned()))
}
pub fn main_branch(repo: &Path, saved: Option<&str>) -> Result<String> {
    if let Some(saved) = saved {
        git(
            repo,
            &["rev-parse", "--verify", &format!("refs/heads/{saved}")],
            false,
        )?;
        return Ok(saved.into());
    }
    let remote = git(
        repo,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        true,
    )?;
    if let Some(branch) = remote.strip_prefix("refs/remotes/origin/")
        && !git(
            repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
            true,
        )?
        .is_empty()
    {
        return Ok(branch.into());
    }
    for branch in ["main", "master"] {
        if !git(
            repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
            true,
        )?
        .is_empty()
        {
            return Ok(branch.into());
        }
    }
    let branch = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"], true)?;
    ensure!(
        !branch.is_empty(),
        "Не удалось определить основную ветку: {}",
        repo.display()
    );
    Ok(branch)
}
fn validate_operation(wt: &Path) -> Result<()> {
    for name in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"] {
        ensure!(
            git(wt, &["rev-parse", "--verify", name], true)?.is_empty(),
            "Сначала завершите Git-операцию в worktree: {}",
            wt.display()
        );
    }
    for marker in ["rebase-merge", "rebase-apply"] {
        ensure!(
            !wt.join(git(wt, &["rev-parse", "--git-path", marker], false)?)
                .exists(),
            "Сначала завершите rebase в worktree: {}",
            wt.display()
        );
    }
    Ok(())
}
pub(crate) fn validate_worktree(item: &Value) -> Result<()> {
    let repo = Path::new(s(item, "repository"));
    let wt = Path::new(s(item, "path"));
    ensure!(
        wt.is_dir() && std::fs::canonicalize(wt)? != std::fs::canonicalize(repo)?,
        "Не найден отдельный worktree задачи: {}",
        wt.display()
    );
    let registered = git(repo, &["worktree", "list", "--porcelain"], false)?;
    let expected = format!("worktree {}", std::fs::canonicalize(wt)?.display());
    let branch = format!("branch refs/heads/{}", s(item, "branch"));
    ensure!(
        registered
            .split("\n\n")
            .any(|b| b.lines().any(|l| l == expected) && b.lines().any(|l| l == branch)),
        "Worktree не соответствует сохранённой ветке: {}",
        wt.display()
    );
    ensure!(
        git(wt, &["symbolic-ref", "--quiet", "--short", "HEAD"], false)? == s(item, "branch"),
        "Ветка worktree изменена: {}",
        wt.display()
    );
    validate_operation(wt)
}
/// A clean status can hide tracked changes behind index flags.
pub(crate) fn validate_index_flags(wt: &Path) -> Result<()> {
    let entries = git(wt, &["ls-files", "-v", "-z"], false)?;
    ensure!(
        entries
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .all(|entry| entry.starts_with("H ")),
        "Индекс скрывает отслеживаемые изменения; снимите skip-worktree и assume-unchanged"
    );
    Ok(())
}
fn checkpoint(c: &Context, t: &mut Value, r: &mut Value) -> Result<()> {
    c.update(r, t)?;
    let dir = c.run_file(s(r, "id"))?.parent().unwrap().to_path_buf();
    store::save_raw(&dir.join("git-completion.json"), &r["git_completion"])?;
    if !r["worktree_cleanup"].is_null() {
        store::save_raw(&dir.join("worktree-cleanup.json"), &r["worktree_cleanup"])?;
    }
    let mut text = String::from("# Сохранение кода при завершении\n\n");
    for item in r["git_completion"]["repositories"].as_array().unwrap() {
        text.push_str(&format!("- Репозиторий: {}\n  Ветка: {}; основная: {}\n  Действие: {}; commit: {}\n  Код сохранён: {}\n  Worktree удалён: {}\n",s(item,"repository"),s(item,"branch"),s(item,"main_branch"),s(item,"mode"),s(item,"commit"),item["published"]==true,item["removed"]==true));
    }
    text.push_str(&format!(
        "\nСтатус: {}\n{}\n",
        s(&r["git_completion"], "status"),
        s(&r["git_completion"], "error")
    ));
    text.push_str(&format!("\nУдаление worktree: {}\n", r["worktree_cleanup"]));
    store::atomic(&dir.join("git-completion.md"), text.as_bytes())
}
// Keep current outcome separate from the immutable evidence of earlier attempts.
fn start_attempt(phase: &mut Value, mut details: Value) {
    if !phase["attempts"].is_array() {
        phase["attempts"] = json!([]);
        if phase["error"].is_string() {
            let legacy = json!({"status":"failed","error":phase["error"],"legacy":true});
            phase["attempts"].as_array_mut().unwrap().push(legacy);
        }
    }
    if phase["first_error"].is_null() && phase["error"].is_string() {
        phase["first_error"] = phase["error"].clone();
    }
    let attempts = phase["attempts"].as_array_mut().unwrap();
    if let Some(last) = attempts.last_mut()
        && last["status"] == "pending"
    {
        last["status"] = json!("interrupted");
    }
    details["status"] = json!("pending");
    details["started_at"] = json!(crate::store::now());
    attempts.push(details);
}

fn end_attempt(phase: &mut Value, result: &Result<()>) {
    if let Err(error) = result {
        phase["status"] = json!("failed");
        phase["error"] = json!(error.to_string());
        if phase["first_error"].is_null() {
            phase["first_error"] = phase["error"].clone();
        }
    }
    let outcome = phase["status"].clone();
    let attempt = phase["attempts"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    attempt["status"] = outcome;
    attempt["finished_at"] = json!(crate::store::now());
    if let Err(error) = result {
        attempt["error"] = json!(error.to_string());
    }
}

pub fn complete_code(
    c: &Context,
    t: &mut Value,
    r: &mut Value,
    message: Option<&str>,
) -> Result<()> {
    if r["worktree_cleanup"].is_null() {
        publish_code(c, t, r, message)?;
        crate::review::bind_publication(c, t, r)?;
        r["worktree_cleanup"] = json!({"status":"pending","submission":r["result_revision"],"review_policy":t["agent_review_policy"]});
    }
    if r["worktree_cleanup"]["status"] == "completed" {
        return remove_completed_worktrees(c, t, r);
    }
    // Old keep/process policies do not override the current deletion policy.
    r["worktree_cleanup"]["keep"] = json!(false);
    start_attempt(&mut r["worktree_cleanup"], json!({}));
    checkpoint(c, t, r)?;
    let result = remove_completed_worktrees(c, t, r);
    end_attempt(&mut r["worktree_cleanup"], &result);
    checkpoint(c, t, r)?;
    result
}

pub fn cancel_cleanup(c: &Context, t: &mut Value, r: &mut Value) -> Result<()> {
    if !r["worktree_cleanup"].is_null() && r["worktree_cleanup"]["status"] != "completed" {
        start_attempt(
            &mut r["worktree_cleanup"],
            json!({"action":"cancel","keep":true,"keep_reason":"cancelled: preserve unpublished work"}),
        );
        r["worktree_cleanup"]["status"] = json!("cancelled");
        r["worktree_cleanup"]["keep"] = json!(true);
        r["worktree_cleanup"]["keep_reason"] = json!("cancelled: preserve unpublished work");
        r["worktree_cleanup"]
            .as_object_mut()
            .unwrap()
            .remove("error");
        end_attempt(&mut r["worktree_cleanup"], &Ok(()));
        for item in r["git_completion"]["repositories"]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            if item["removed"] != true {
                item["kept"] = json!(true);
            }
        }
        checkpoint(c, t, r)?;
    }
    Ok(())
}

fn complete_inner(c: &Context, t: &mut Value, r: &mut Value, message: &str) -> Result<()> {
    ensure!(
        r["git_completion"]["git_mode"].as_str().unwrap_or("main")
            == t["git_mode"].as_str().unwrap_or("main"),
        "Git-режим изменён после начала завершения; сначала разберите сохранённый git_completion"
    );
    if r["git_completion"]["repositories"]
        .as_array()
        .unwrap()
        .is_empty()
    {
        let mut items = vec![];
        for wt in r["worktrees"].as_array().into_iter().flatten() {
            let mut item = wt.clone();
            let repo = Path::new(s(wt, "repository"));
            let base = main_branch(repo, wt["main_branch"].as_str())?;
            let origin = git(repo, &["remote"], false)?
                .lines()
                .any(|l| l == "origin");
            item["mode"] = json!(
                if t["git_mode"] == "branch" && origin && wt["branch"] != base {
                    "push_branch"
                } else {
                    "merge_main"
                }
            );
            item["origin"] = json!(origin);
            item["main_branch"] = json!(base);
            items.push(item);
        }
        r["git_completion"]["repositories"] = Value::Array(items);
        checkpoint(c, t, r)?;
    }
    let count = r["git_completion"]["repositories"]
        .as_array()
        .unwrap()
        .len();
    for i in 0..count {
        let mut item = r["git_completion"]["repositories"][i].clone();
        let wt_path = std::path::PathBuf::from(s(&item, "path"));
        let repo_path = std::path::PathBuf::from(s(&item, "repository"));
        let wt = wt_path.as_path();
        let repo = repo_path.as_path();
        if item["published"] == true && item["mode"] == "push_branch" && !wt.exists() {
            ensure!(
                git(
                    repo,
                    &["rev-parse", &format!("refs/heads/{}", s(&item, "branch"))],
                    false
                )? == s(&item, "commit"),
                "Сохранённая ветка изменилась после удаления worktree"
            );
            item["removed"] = json!(true);
            r["git_completion"]["repositories"][i] = item;
            checkpoint(c, t, r)?;
            continue;
        }
        if item["published"] == true && wt.exists() {
            validate_worktree(&item)?;
            validate_secret_files(wt)?;
            let changed = git(wt, &["rev-parse", "HEAD"], false)? != s(&item, "commit")
                || !git(wt, &["status", "--porcelain"], false)?.is_empty();
            if changed {
                for key in ["commit", "main_commit", "remote_commit", "published"] {
                    item.as_object_mut().unwrap().remove(key);
                }
            } else {
                let branch = if item["mode"] == "push_branch" {
                    s(&item, "branch")
                } else {
                    s(&item, "main_branch")
                };
                let expected = if item["mode"] == "push_branch" {
                    s(&item, "commit")
                } else {
                    s(&item, "main_commit")
                };
                let local = git(repo, &["rev-parse", &format!("refs/heads/{branch}")], false)?;
                let remote = if item["origin"] == true {
                    git(
                        repo,
                        &["ls-remote", "origin", &format!("refs/heads/{branch}")],
                        false,
                    )?
                } else {
                    String::new()
                };
                if local != expected
                    || (item["origin"] == true
                        && remote.split_whitespace().next() != Some(expected))
                {
                    item["published"] = json!(false);
                }
            }
            r["git_completion"]["repositories"][i] = item.clone();
            checkpoint(c, t, r)?;
        }
        if item["removed"] != true {
            validate_worktree(&item)?;
            validate_secret_files(wt)?;
            ensure!(
                !item["commit"].is_null()
                    || git(wt, &["status", "--porcelain"], false)?.is_empty()
                    || !message.is_empty(),
                "Для новых изменений укажите --commit-message с английским описанием сути изменения"
            );
            if item["mode"] == "merge_main" && item["branch"] != item["main_branch"] {
                validate_operation(repo)?;
                ensure!(
                    git(repo, &["status", "--porcelain"], false)?.is_empty(),
                    "Основной репозиторий содержит чужие незакоммиченные изменения: {}",
                    repo.display()
                );
                ensure!(
                    git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"], false)?
                        == s(&item, "main_branch"),
                    "В основном репозитории должна быть открыта ветка {}",
                    s(&item, "main_branch")
                );
            }
        }
    }
    for i in 0..count {
        let mut item = r["git_completion"]["repositories"][i].clone();
        if item["removed"] == true {
            continue;
        }
        let wt_buf = std::path::PathBuf::from(s(&item, "path"));
        let repo_buf = std::path::PathBuf::from(s(&item, "repository"));
        let wt = wt_buf.as_path();
        let repo = repo_buf.as_path();
        if !item["commit"].is_null() {
            ensure!(
                git(wt, &["status", "--porcelain"], false)?.is_empty()
                    && git(wt, &["rev-parse", "HEAD"], false)? == s(&item, "commit"),
                "Worktree изменился после начала завершения: {}",
                wt.display()
            );
        } else {
            if !git(wt, &["status", "--porcelain"], false)?.is_empty() {
                git(wt, &["add", "--all"], false)?;
                git(wt, &["commit", "-m", message], false)?;
            }
            item["commit"] = json!(git(wt, &["rev-parse", "HEAD"], false)?);
            r["git_completion"]["repositories"][i] = item.clone();
            checkpoint(c, t, r)?;
        }
        if item["published"] != true {
            if item["mode"] == "merge_main" && item["branch"] != item["main_branch"] {
                let subject = git(wt, &["log", "-1", "--format=%s", s(&item, "commit")], false)?;
                if let Err(e) = git(
                    repo,
                    &[
                        "merge",
                        "--no-edit",
                        "-m",
                        &format!("Merge: {subject}"),
                        s(&item, "commit"),
                    ],
                    false,
                ) {
                    if !git(repo, &["rev-parse", "--verify", "MERGE_HEAD"], true)?.is_empty() {
                        git(repo, &["merge", "--abort"], false)?;
                    }
                    return Err(e);
                }
            }
            if item["mode"] == "merge_main" {
                item["main_commit"] = json!(git(
                    repo,
                    &[
                        "rev-parse",
                        &format!("refs/heads/{}", s(&item, "main_branch"))
                    ],
                    false
                )?);
                git(
                    repo,
                    &[
                        "merge-base",
                        "--is-ancestor",
                        s(&item, "commit"),
                        s(&item, "main_commit"),
                    ],
                    false,
                )?;
            }
            if item["origin"] == true {
                let (branch, head) = if item["mode"] == "push_branch" {
                    (s(&item, "branch").to_owned(), s(&item, "commit").to_owned())
                } else {
                    (
                        s(&item, "main_branch").to_owned(),
                        s(&item, "main_commit").to_owned(),
                    )
                };
                git(
                    repo,
                    &["push", "origin", &format!("{head}:refs/heads/{branch}")],
                    false,
                )?;
                let remote = git(
                    repo,
                    &[
                        "ls-remote",
                        "--exit-code",
                        "origin",
                        &format!("refs/heads/{branch}"),
                    ],
                    false,
                )?;
                ensure!(
                    remote.split_whitespace().next() == Some(&head),
                    "Не подтверждён commit удалённой ветки {branch}"
                );
                item["remote_branch"] = json!(branch);
                item["remote_commit"] = json!(head);
            }
            item["published"] = json!(true);
            r["git_completion"]["repositories"][i] = item.clone();
            checkpoint(c, t, r)?;
        }
        r["git_completion"]["repositories"][i] = item;
        checkpoint(c, t, r)?;
    }
    r["git_completion"]["status"] = json!("completed");
    r["git_completion"].as_object_mut().unwrap().remove("error");
    checkpoint(c, t, r)
}

/// Same transaction as completion, retaining every checkout for continued work.
pub fn publish_code(
    c: &Context,
    t: &mut Value,
    r: &mut Value,
    message: Option<&str>,
) -> Result<()> {
    crate::review::prepare_publication(c, t, r)?;
    if r["git_completion"].is_null() {
        r["git_completion"] = json!({"status":"pending","repositories":[],"git_mode":t["git_mode"].as_str().unwrap_or("main")});
    }
    start_attempt(&mut r["git_completion"], json!({}));
    checkpoint(c, t, r)?;
    let result = complete_inner(c, t, r, message.unwrap_or("").trim());
    end_attempt(&mut r["git_completion"], &result);
    checkpoint(c, t, r)?;
    result
}

/// Observed freshness; never treats a persisted success as proof of current code.
pub fn publication_current(r: &Value) -> Result<bool> {
    if r["git_completion"]["status"] != "completed" {
        return Ok(false);
    }
    for item in r["git_completion"]["repositories"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let wt = Path::new(s(item, "path"));
        let repo = Path::new(s(item, "repository"));
        if wt.exists() {
            validate_worktree(item)?;
            if !git(wt, &["status", "--porcelain"], false)?.is_empty()
                || git(wt, &["rev-parse", "HEAD"], false)? != s(item, "commit")
            {
                return Ok(false);
            }
        } else if item["removed"] != true && item["removal_pending"] != true {
            return Ok(false);
        }
        if git(
            repo,
            &["rev-parse", &format!("refs/heads/{}", s(item, "branch"))],
            false,
        )? != s(item, "commit")
        {
            return Ok(false);
        }
        let branch = if item["mode"] == "push_branch" {
            s(item, "branch")
        } else {
            s(item, "main_branch")
        };
        let head = if item["mode"] == "push_branch" {
            s(item, "commit")
        } else {
            s(item, "main_commit")
        };
        if git(repo, &["rev-parse", &format!("refs/heads/{branch}")], false)? != head {
            return Ok(false);
        }
        if item["origin"] == true
            && git(
                repo,
                &["ls-remote", "origin", &format!("refs/heads/{branch}")],
                false,
            )?
            .split_whitespace()
            .next()
                != Some(head)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

// Only another unfinished task can retain a published worktree.
fn worktree_users(c: &Context, task: &Value, wt: &Path) -> Result<Vec<Value>> {
    let wt = std::fs::canonicalize(wt)?;
    let mut users = vec![];
    for other in c.rows("tasks")? {
        if other["id"] == task["id"] || ["done", "cancelled"].contains(&s(&other, "status")) {
            continue;
        }
        let Some(run_id) = other["active_run"].as_str() else {
            continue;
        };
        let run = c.current(&other, run_id)?;
        let cwd_uses = if let Some(cwd) = run["cwd"].as_str() {
            crate::config::location(Path::new(cwd))?.starts_with(&wt)
        } else {
            false
        };
        let mut uses = cwd_uses;
        for path in run["worktrees"].as_array().into_iter().flatten() {
            let path = crate::config::location(Path::new(s(path, "path")))?;
            uses |= path.starts_with(&wt) || wt.starts_with(path);
        }
        if uses {
            users.push(json!({"task":other["id"],"run":run_id}));
        }
    }
    Ok(users)
}

// Publication and deletion are separate durable phases: a removal retry never
// republishes code. Git owns recursive deletion, including local/ignored files.
fn remove_completed_worktrees(c: &Context, t: &mut Value, r: &mut Value) -> Result<()> {
    ensure!(
        r["worktree_cleanup"]["submission"] == r["result_revision"],
        "Отчёт изменился после начала завершения. Какую версию результата принимать? Worktree cleanup не продолжается"
    );
    if r["worktree_cleanup"]["status"] == "completed" {
        return Ok(());
    }
    ensure!(
        r["git_completion"]["git_mode"].as_str().unwrap_or("main")
            == t["git_mode"].as_str().unwrap_or("main"),
        "Git-режим изменён после начала завершения; восстановите прежний режим"
    );
    ensure!(
        publication_current(r)?,
        "Сохранённый Git-результат изменился. Где сохранить новые коммиты? Worktree и таб оставлены"
    );
    let count = r["git_completion"]["repositories"]
        .as_array()
        .unwrap()
        .len();
    for i in 0..count {
        let mut item = r["git_completion"]["repositories"][i].clone();
        let path = std::path::PathBuf::from(s(&item, "path"));
        let wt = path.as_path();
        if item["removed"] == true {
            ensure!(
                !wt.exists(),
                "Путь удалённого worktree снова занят: {}",
                wt.display()
            );
            continue;
        }
        if !wt.exists() && item["removal_pending"] == true {
            ensure!(
                !git(
                    Path::new(s(&item, "repository")),
                    &["worktree", "list", "--porcelain"],
                    false
                )?
                .lines()
                .any(|line| line == format!("worktree {}", wt.display())),
                "Worktree отсутствует, но зарегистрирован Git: {}",
                wt.display()
            );
        } else {
            validate_worktree(&item)?;
            let users = worktree_users(c, t, wt)?;
            if !users.is_empty() {
                item["kept"] = json!(true);
                item["used_by"] = json!(users);
                r["git_completion"]["repositories"][i] = item;
                checkpoint(c, t, r)?;
                continue;
            }
            item["removal_pending"] = json!(true);
            r["git_completion"]["repositories"][i] = item.clone();
            checkpoint(c, t, r)?;
            git(
                Path::new(s(&item, "repository")),
                &["worktree", "remove", "--force", "--force", s(&item, "path")],
                false,
            )?;
        }
        item["removed"] = json!(true);
        item.as_object_mut().unwrap().remove("kept");
        item.as_object_mut().unwrap().remove("used_by");
        r["git_completion"]["repositories"][i] = item;
        checkpoint(c, t, r)?;
    }
    r["worktree_cleanup"]["status"] = json!("completed");
    r["worktree_cleanup"]
        .as_object_mut()
        .unwrap()
        .remove("error");
    checkpoint(c, t, r)
}

fn validate_secret_files(wt: &Path) -> Result<()> {
    let files = format!(
        "{}\0{}",
        git(
            wt,
            &["ls-files", "-z", "--others", "--exclude-standard"],
            false
        )?,
        git(wt, &["diff", "--name-only", "-z", "HEAD"], false)?
    );
    for file in files.split('\0').filter(|s| !s.is_empty()) {
        let path = Path::new(file);
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let secret = name == ".env"
            || (name.starts_with(".env.")
                && ![".env.example", ".env.sample", ".env.template"].contains(&name))
            || [".aws", ".ssh"]
                .iter()
                .any(|part| path.components().any(|c| c.as_os_str() == *part))
            || [".codex/config.toml", ".claude/settings.local.json"].contains(&file);
        ensure!(
            !secret,
            "Обнаружены локальные секретные настройки {} в {}. Куда перенести их перед публикацией?",
            file,
            wt.display()
        );
    }
    Ok(())
}

/// Replace a failed candidate only when no publication of it can be observed.
pub fn recover_candidate(
    c: &Context,
    t: &mut Value,
    r: &mut Value,
    a: &Value,
    frozen: &Value,
) -> Result<()> {
    ensure!(
        r["worktree_cleanup"].is_null(),
        "Удаление worktree уже началось"
    );
    ensure!(
        r["git_completion"]["status"] == "failed",
        "Восстанавливается только неудачная публикация"
    );
    ensure!(
        r["git_completion"]["attempts"]
            .as_array()
            .and_then(|v| v.last())
            .is_some_and(|v| v["status"] == "failed"),
        "Исход последней попытки не подтверждён; нужна сверка процесса Git"
    );
    ensure!(
        r["git_completion"]["git_mode"].as_str().unwrap_or("main")
            == t["git_mode"].as_str().unwrap_or("main"),
        "Git-режим изменён"
    );
    let repo_name = s(a, "repository");
    let requested = s(a, "commit");
    let items = r["git_completion"]["repositories"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Нет журнала репозиториев"))?;
    let indexes: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, v)| s(v, "repository") == repo_name)
        .map(|(i, _)| i)
        .collect();
    ensure!(
        indexes.len() == 1,
        "Репозиторий должен однозначно соответствовать журналу"
    );
    let i = indexes[0];
    let item = items[i].clone();
    let saved = r["worktrees"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| s(v, "repository") == repo_name)
        .collect::<Vec<_>>();
    ensure!(
        saved.len() == 1
            && ["path", "branch", "main_branch"]
                .iter()
                .all(|k| saved[0][*k] == item[*k]),
        "Worktree не соответствует запуску"
    );
    ensure!(
        item["published"] != true
            && item["removed"] != true
            && item["main_commit"].is_null()
            && item["remote_commit"].is_null(),
        "Возможна частичная публикация; нужна отдельная сверка"
    );
    let current = s(&item, "commit");
    let prior = r["git_completion"]["recoveries"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|v| v["commit"] == requested && v["previous"]["repository"] == repo_name);
    let repeated = current == requested && prior.is_some_and(|v| v["review"] == a["review"]);
    let old = if current == requested {
        let previous =
            prior.ok_or_else(|| anyhow::anyhow!("Нет записи предыдущего восстановления"))?;
        previous["old_commit"]
            .as_str()
            .unwrap_or_else(|| s(&previous["previous"], "commit"))
    } else {
        current
    };
    ensure!(
        !old.is_empty() && old != requested,
        "Нужен новый commit вместо сохранённого"
    );
    let wt = Path::new(s(&item, "path"));
    let repo = Path::new(repo_name);
    validate_worktree(&item)?;
    validate_secret_files(wt)?;
    validate_index_flags(wt)?;
    ensure!(
        git(wt, &["status", "--porcelain"], false)?.is_empty(),
        "Worktree содержит изменения"
    );
    ensure!(
        git(wt, &["rev-parse", "HEAD"], false)? == requested,
        "Укажите полный текущий commit worktree"
    );
    let reviewed = frozen["manifest"]["repositories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| {
            v["repository"] == repo_name
                && v["path"] == item["path"]
                && v["head"] == requested
                && s(v, "status").trim().is_empty()
        })
        .count();
    ensure!(reviewed == 1, "Commit не соответствует снимку ревью");
    git(
        repo,
        &["cat-file", "-e", &format!("{old}^{{commit}}")],
        false,
    )?;
    ensure!(
        ["push_branch", "merge_main"].contains(&s(&item, "mode")) && item["origin"].is_boolean(),
        "Неполный журнал Git; нужна сверка"
    );
    let expected_mode = if t["git_mode"] == "branch"
        && item["origin"] == true
        && item["branch"] != item["main_branch"]
    {
        "push_branch"
    } else {
        "merge_main"
    };
    ensure!(
        item["mode"] == expected_mode,
        "Режим репозитория не соответствует Git-политике"
    );
    let has_origin = git(repo, &["remote"], false)?
        .lines()
        .any(|v| v == "origin");
    ensure!(
        has_origin == (item["origin"] == true),
        "Настройка origin изменилась"
    );
    let branch = if item["mode"] == "push_branch" {
        s(&item, "branch")
    } else {
        s(&item, "main_branch")
    };
    ensure!(!branch.is_empty(), "Неизвестная ветка публикации");
    let local = git(
        repo,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        false,
    )?;
    // A missing or failed merge-base is ambiguous and must not authorize recovery.
    let base = git(repo, &["merge-base", old, &local], false)?;
    ensure!(base != old, "Прежний commit присутствует в целевой ветке");
    if item["origin"] == true {
        let remote = git(
            repo,
            &["ls-remote", "origin", &format!("refs/heads/{branch}")],
            false,
        )?;
        if item["mode"] == "push_branch" {
            ensure!(
                remote.is_empty(),
                "Удалённая ветка уже существует; нужна сверка"
            );
        } else {
            ensure!(
                remote.split_whitespace().next() == Some(local.as_str()),
                "Удалённая ветка не совпадает с локальной; нужна сверка"
            );
        }
    }
    if repeated {
        return checkpoint(c, t, r);
    }
    let entry = json!({"at":store::now(), "by":a["by"], "review":a["review"], "version":frozen["version"], "previous":item, "old_commit":old, "commit":requested, "target_head":local});
    if !r["git_completion"]["recoveries"].is_array() {
        r["git_completion"]["recoveries"] = json!([]);
    }
    r["git_completion"]["recoveries"]
        .as_array_mut()
        .unwrap()
        .push(entry);
    r["git_completion"]["repositories"][i]["commit"] = json!(requested);
    checkpoint(c, t, r)
}
