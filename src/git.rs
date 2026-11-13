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
pub fn complete_code(
    c: &Context,
    t: &mut Value,
    r: &mut Value,
    message: Option<&str>,
    keep_worktree: bool,
) -> Result<()> {
    if r["worktree_cleanup"].is_null() {
        publish_code(c, t, r, message)?;
        r["worktree_cleanup"] = json!({"status":"pending","keep":keep_worktree,"submission":r["result_revision"],"review_policy":t["agent_review_policy"]});
        checkpoint(c, t, r)?;
    }
    if keep_worktree && r["worktree_cleanup"]["status"] != "completed" {
        r["worktree_cleanup"]["keep"] = json!(true);
    }
    let result = remove_completed_worktrees(c, t, r);
    if let Err(e) = &result {
        r["worktree_cleanup"]["status"] = json!("failed");
        r["worktree_cleanup"]["error"] = json!(e.to_string());
        checkpoint(c, t, r)?;
    }
    result
}

pub fn cancel_cleanup(c: &Context, t: &mut Value, r: &mut Value) -> Result<()> {
    if !r["worktree_cleanup"].is_null() && r["worktree_cleanup"]["status"] != "completed" {
        r["worktree_cleanup"]["status"] = json!("cancelled");
        r["worktree_cleanup"]["keep"] = json!(true);
        r["worktree_cleanup"]
            .as_object_mut()
            .unwrap()
            .remove("error");
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
    if r["git_completion"].is_null() {
        r["git_completion"] = json!({"status":"pending","repositories":[],"git_mode":t["git_mode"].as_str().unwrap_or("main")});
    }
    let result = complete_inner(c, t, r, message.unwrap_or("").trim());
    if let Err(e) = &result {
        r["git_completion"]["status"] = json!("failed");
        r["git_completion"]["error"] = json!(e.to_string());
        checkpoint(c, t, r)?;
    }
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

// Publication and deletion are separate durable phases. Never republish on a
// removal retry: new edits require a new review, rather than another commit.
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
    let items = r["git_completion"]["repositories"]
        .as_array()
        .unwrap()
        .clone();
    let keep = r["worktree_cleanup"]["keep"] == true;
    let mut allowed = vec![std::process::id()];
    if !keep && r["cleanup_processes"].is_array() {
        allowed.extend(
            r["cleanup_processes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p.as_u64().and_then(|p| u32::try_from(p).ok())),
        );
    }
    for item in &items {
        let wt = Path::new(s(item, "path"));
        if item["removed"] == true {
            ensure!(
                !wt.exists(),
                "Путь удалённого worktree снова занят: {}. Кому принадлежит новое дерево?",
                wt.display()
            );
            continue;
        }
        if !wt.exists() && item["removal_pending"] == true {
            ensure!(
                !git(
                    Path::new(s(item, "repository")),
                    &["worktree", "list", "--porcelain"],
                    false
                )?
                .lines()
                .any(|l| l == format!("worktree {}", wt.display())),
                "Worktree отсутствует, но зарегистрирован Git; нужна сверка: {}",
                wt.display()
            );
            continue;
        }
        validate_worktree(item)?;
        if keep {
            continue;
        }
        for path in [&c.root, &c.bindings, &c.program]
            .into_iter()
            .chain(c.config.iter())
        {
            ensure!(
                !std::fs::canonicalize(path)?.starts_with(std::fs::canonicalize(wt)?),
                "Ресурсы панели находятся внутри {}. Куда перенести их перед удалением?",
                wt.display()
            );
        }
        for other in c.runs()? {
            if other["id"] == r["id"]
                || ["done", "cancelled"].contains(&s(&other, "status"))
                || (other["task"] == r["task"] && other["status"] == "interrupted")
            {
                continue;
            }
            ensure!(
                !other["worktrees"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|w| w["path"] == item["path"]),
                "Worktree используется запуском {}. Сохранить дерево через --keep-worktree?",
                s(&other, "id")
            );
        }
        reproducible_cache_files(wt)?;
        validate_local_flags(wt)?;
        processes_outside_pane(wt, &allowed)?;
    }
    for i in 0..items.len() {
        let mut item = r["git_completion"]["repositories"][i].clone();
        if keep {
            if item["removed"] != true {
                item["kept"] = json!(true);
            }
        } else if item["removed"] != true {
            let wt = Path::new(s(&item, "path"));
            if wt.exists() {
                // Recheck immediately before the destructive command.
                validate_worktree(&item)?;
                validate_local_flags(wt)?;
                ensure!(
                    git(wt, &["rev-parse", "HEAD"], false)? == s(&item, "commit"),
                    "В worktree появились неопубликованные коммиты; где сохранить изменения?"
                );
                ensure!(
                    git(wt, &["status", "--porcelain"], false)?.is_empty(),
                    "Worktree изменился перед удалением; где сохранить изменения?"
                );
                let caches = reproducible_cache_files(wt)?;
                processes_outside_pane(wt, &allowed)?;
                item["removal_pending"] = json!(true);
                r["git_completion"]["repositories"][i] = item.clone();
                checkpoint(c, t, r)?;
                for file in caches {
                    std::fs::remove_file(file)?;
                }
                git(
                    Path::new(s(&item, "repository")),
                    &["worktree", "remove", s(&item, "path")],
                    false,
                )?;
            }
            item["removed"] = json!(true);
        }
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

fn validate_local_flags(wt: &Path) -> Result<()> {
    let config = wt.join(git(
        wt,
        &["rev-parse", "--git-path", "config.worktree"],
        false,
    )?);
    ensure!(
        !config.exists(),
        "У worktree {} есть локальный config.worktree. Куда сохранить настройки перед удалением? Можно выбрать --keep-worktree",
        wt.display()
    );
    ensure!(
        git(
            wt,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/worktree/",
                "refs/bisect/",
                "refs/rewritten/"
            ],
            false
        )?
        .is_empty(),
        "У worktree {} есть локальные Git refs. Где сохранить их перед удалением?",
        wt.display()
    );
    let flags = git(wt, &["ls-files", "-v"], false)?;
    ensure!(!flags.lines().any(|l| l.starts_with('S') || l.as_bytes().first().is_some_and(u8::is_ascii_lowercase)),"В {} есть skip-worktree/assume-unchanged файлы. Снимите флаги и сохраните данные либо выберите --keep-worktree",wt.display());
    Ok(())
}

#[cfg(target_os = "linux")]
fn processes_outside_pane(wt: &Path, allowed: &[u32]) -> Result<()> {
    let wt = std::fs::canonicalize(wt)?;
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if allowed.contains(&pid) {
            continue;
        }
        let dir = entry.path();
        use std::os::unix::fs::MetadataExt;
        let Ok(metadata) = std::fs::metadata(&dir) else {
            continue;
        };
        if metadata.uid() != unsafe { libc::geteuid() } {
            continue;
        }
        if !external::process_running(pid as i64) {
            continue;
        }
        for name in ["cwd", "exe"] {
            match std::fs::read_link(dir.join(name)) {
                Ok(path) => ensure!(
                    !path.starts_with(&wt),
                    "Worktree {} использует процесс PID {} ({}). Завершите его или выберите --keep-worktree",
                    wt.display(),
                    pid,
                    name
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    ensure!(
                        !external::process_running(pid as i64),
                        "Нельзя проверить PID {}: {}. Сохранить дерево через --keep-worktree?",
                        pid,
                        e
                    );
                }
            }
        }
        match std::fs::read_to_string(dir.join("maps")) {
            Ok(maps) => {
                for line in maps.lines() {
                    if let Some(start) = line.find('/') {
                        ensure!(
                            !Path::new(&line[start..]).starts_with(&wt),
                            "В worktree {} отображён файл процессом PID {}. Завершите процесс или выберите --keep-worktree",
                            wt.display(),
                            pid
                        );
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => ensure!(
                !external::process_running(pid as i64),
                "Нельзя проверить отображения PID {}: {}. Сохранить дерево через --keep-worktree?",
                pid,
                e
            ),
        }
        let fds = match std::fs::read_dir(dir.join("fd")) {
            Ok(fds) => Some(fds),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                ensure!(
                    !external::process_running(pid as i64),
                    "Нельзя проверить файлы PID {}: {}. Сохранить дерево через --keep-worktree?",
                    pid,
                    e
                );
                None
            }
        };
        if let Some(fds) = fds {
            for fd in fds {
                let fd = fd?;
                match std::fs::read_link(fd.path()) {
                    Ok(path) => ensure!(
                        !path.starts_with(&wt),
                        "В worktree {} открыт файл процессом PID {}. Завершите процесс или выберите --keep-worktree",
                        wt.display(),
                        pid
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => ensure!(
                        !external::process_running(pid as i64),
                        "Нельзя проверить дескриптор PID {}: {}. Сохранить дерево через --keep-worktree?",
                        pid,
                        e
                    ),
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn processes_outside_pane(wt: &Path, allowed: &[u32]) -> Result<()> {
    let (ok, out, err) = external::output(
        Command::new("lsof").args(["-t", "+D"]).arg(wt),
        Duration::from_secs(60),
    )?;
    ensure!(
        err.trim().is_empty() && (ok || out.trim().is_empty()),
        "Не удалось проверить процессы worktree; сохранить через --keep-worktree?"
    );
    for pid in out.lines() {
        let pid = pid.parse::<u32>()?;
        ensure!(
            allowed.contains(&pid),
            "Worktree {} использует PID {}. Завершите процесс или выберите --keep-worktree",
            wt.display(),
            pid
        );
    }
    Ok(())
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

// Only bytecode with a tracked source is proven disposable. A directory name
// such as target/.venv/cache alone is not evidence about all its contents.
fn reproducible_cache_files(wt: &Path) -> Result<Vec<std::path::PathBuf>> {
    let canonical_wt = std::fs::canonicalize(wt)?;
    let ignored = git(
        wt,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ],
        false,
    )?;
    let mut files = vec![];
    for file in ignored.split('\0').filter(|s| !s.is_empty()) {
        let path = Path::new(file);
        let parent = path.parent().unwrap_or(Path::new(""));
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let source = name.split_once(".cpython-").map(|(stem, _)| {
            parent
                .parent()
                .unwrap_or(Path::new(""))
                .join(format!("{stem}.py"))
        });
        let full = wt.join(path);
        let valid = parent.file_name().is_some_and(|p| p == "__pycache__")
            && name.ends_with(".pyc")
            && source.as_ref().is_some_and(|source| {
                wt.join(source).is_file()
                    && git(
                        wt,
                        &[
                            "ls-files",
                            "--error-unmatch",
                            "--",
                            &source.to_string_lossy(),
                        ],
                        true,
                    )
                    .is_ok_and(|s| !s.is_empty())
            })
            && std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_file())
            && std::fs::canonicalize(&full).is_ok_and(|p| p.starts_with(&canonical_wt))
            && std::fs::read(&full).is_ok_and(|b| {
                b.len() > 16
                    && b[2..4] == *b"\r\n"
                    && u32::from_le_bytes(b[4..8].try_into().unwrap()) <= 3
            });
        ensure!(
            valid,
            "В {} есть ignored/local файл {}. Куда перенести нужные данные перед удалением? Можно явно сохранить дерево через --keep-worktree",
            wt.display(),
            file
        );
        files.push(full);
    }
    Ok(files)
}
