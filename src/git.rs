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
fn validate_worktree(item: &Value) -> Result<()> {
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
    let mut text = String::from("# Сохранение кода при завершении\n\n");
    for item in r["git_completion"]["repositories"].as_array().unwrap() {
        text.push_str(&format!("- Репозиторий: {}\n  Ветка: {}; основная: {}\n  Действие: {}; commit: {}\n  Код сохранён: {}\n  Worktree удалён: {}\n",s(item,"repository"),s(item,"branch"),s(item,"main_branch"),s(item,"mode"),s(item,"commit"),item["published"]==true,item["removed"]==true));
    }
    text.push_str(&format!(
        "\nСтатус: {}\n{}\n",
        s(&r["git_completion"], "status"),
        s(&r["git_completion"], "error")
    ));
    store::atomic(&dir.join("git-completion.md"), text.as_bytes())
}
pub fn complete_code(
    c: &Context,
    t: &mut Value,
    r: &mut Value,
    message: Option<&str>,
) -> Result<()> {
    if r["git_completion"]["status"] == "completed" {
        return Ok(());
    }
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
        let wt = Path::new(s(&item, "path"));
        let repo = Path::new(s(&item, "repository"));
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
        if item["removed"] != true {
            validate_worktree(&item)?;
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
        if item["mode"] == "push_branch" {
            if wt.exists() {
                git(repo, &["worktree", "remove", &wt.to_string_lossy()], false)?;
            }
            item["removed"] = json!(true);
        }
        r["git_completion"]["repositories"][i] = item;
        checkpoint(c, t, r)?;
    }
    r["git_completion"]["status"] = json!("completed");
    r["git_completion"].as_object_mut().unwrap().remove("error");
    checkpoint(c, t, r)
}
