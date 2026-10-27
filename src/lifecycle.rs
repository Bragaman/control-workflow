use crate::{
    args::{s, strings},
    config::{Context, location},
    core::{self, extend},
    external::{self, error_code, launcher_alive},
    git,
    store::{self, key, now},
};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn workspace_binding(
    c: &Context,
    p: &mut Value,
    ns: &Value,
    repair: bool,
) -> Result<Option<Value>> {
    ensure!(
        p["layout_pending"].is_null(),
        "Неизвестен результат создания layout; сначала сверьте запуск {}",
        s(p, "layout_pending")
    );
    let session = ns["session"].as_str();
    if p["workspace"].is_null() {
        let result = c.herdr(&["workspace", "list"], session)?;
        ensure!(
            result["workspaces"].is_array(),
            "Некорректный список workspace"
        );
        return Ok(None);
    }
    if p["workspace_namespace"].is_null() {
        let evidence = c
            .runs()?
            .into_iter()
            .filter(|r| r["project"] == p["id"] && r["workspace"] == p["workspace"])
            .map(|r| {
                r.get("herdr_namespace")
                    .filter(|v| !v.is_null())
                    .unwrap_or(&r["herdr_session"])
                    .clone()
            })
            .collect::<Vec<_>>();
        if evidence.is_empty() || evidence.iter().any(|v| v != ns && *v != ns["session"]) {
            if repair && evidence.is_empty() {
                match c.herdr(&["workspace", "get", s(p, "workspace")], session) {
                    Err(e) if error_code(&e) == "workspace_not_found" => {
                        p["workspace"] = Value::Null;
                        p["workspace_namespace"] = ns.clone();
                        return Ok(None);
                    }
                    Err(e) => return Err(e),
                    Ok(_) => {}
                }
            }
            bail!(
                "Старая привязка workspace без подтверждённой сессии; используйте project-set --workspace"
            );
        }
    } else {
        ensure!(
            p["workspace_namespace"] == *ns,
            "Workspace принадлежит другому namespace; нужна явная перепривязка"
        );
    }
    let result = match c.herdr(&["workspace", "get", s(p, "workspace")], session) {
        Ok(v) => {
            ensure!(
                v["workspace"]["workspace_id"] == p["workspace"],
                "Herdr вернул другой workspace"
            );
            Some(v["workspace"].clone())
        }
        Err(e) if error_code(&e) == "workspace_not_found" => None,
        Err(e) => return Err(e),
    };
    p["workspace_namespace"] = ns.clone();
    Ok(result)
}
pub fn require_absent(c: &Context, r: &Value) -> Result<()> {
    ensure!(
        !launcher_alive(r),
        "Подготовитель ещё работает: {}",
        s(r, "id")
    );
    c.check_namespace(r)?;
    ensure!(
        r["pane"].is_null() || !r["herdr_namespace"].is_null() || !r["herdr_session"].is_null(),
        "Не подтверждена сессия прежнего исполнителя: {}",
        s(r, "id")
    );
    ensure!(
        r["layout_intent"].is_null() || !r["pane"].is_null(),
        "Неизвестен результат создания layout: {}",
        s(r, "id")
    );
    if r["pane"].is_null() {
        return Ok(());
    }
    let v = match c.herdr(
        &["pane", "current", "--pane", s(r, "pane")],
        r["herdr_session"].as_str(),
    ) {
        Ok(v) => v,
        Err(e) if error_code(&e) == "pane_not_found" => return Ok(()),
        Err(e) => return Err(e),
    };
    ensure!(
        v["pane"].get("agent").is_some(),
        "Herdr не подтвердил состояние прежнего исполнителя"
    );
    ensure!(
        v["pane"]["agent"].is_null(),
        "Прежний исполнитель ещё работает; продолжите {} в {}",
        s(r, "id"),
        s(r, "pane")
    );
    ensure!(
        r["agent_start_intent"] != true || r["agent_start_completed"] == true,
        "Результат agent start неизвестен; сверьте {} через recover-start",
        s(r, "id")
    );
    Ok(())
}
fn old_workspace_idle(c: &Context, p: &Value, ns: &Value, current: Option<&str>) -> Result<()> {
    for r in c.runs()? {
        if Some(s(&r, "id")) != current
            && r["project"] == p["id"]
            && r["workspace"] == p["workspace"]
        {
            ensure!(
                (r["herdr_namespace"].is_null() || r["herdr_namespace"] == *ns)
                    && r["herdr_session"] == ns["session"],
                "Namespace прежнего запуска не подтверждён: {}",
                s(&r, "id")
            );
            require_absent(c, &r)?;
        }
    }
    Ok(())
}
pub fn agent_state(c: &Context, r: &Value) -> Result<&'static str> {
    c.check_namespace(r)?;
    let v = match c.herdr(
        &["pane", "current", "--pane", s(r, "pane")],
        r["herdr_session"].as_str(),
    ) {
        Ok(v) => v,
        Err(e) if error_code(&e) == "pane_not_found" => return Ok("absent"),
        Err(e) => return Err(e),
    };
    ensure!(
        v["pane"].get("agent").is_some(),
        "Herdr не подтвердил состояние исполнителя"
    );
    if v["pane"]["agent"].is_null() {
        return Ok("absent");
    }
    if v["pane"]["agent"] != external::agent_kind(c, r) {
        return Ok("unconfirmed");
    }
    let text = c.herdr_text(
        &[
            "pane",
            "read",
            s(r, "pane"),
            "--source",
            "recent-unwrapped",
            "--lines",
            "120",
        ],
        r["herdr_session"].as_str(),
        Duration::from_secs(60),
    )?;
    Ok(
        if text.contains(
            &c.run_file(s(r, "id"))?
                .with_file_name("assignment.md")
                .to_string_lossy()
                .to_string(),
        ) {
            "confirmed"
        } else {
            "unconfirmed"
        },
    )
}
pub fn mark_recovered(c: &Context, r: &mut Value, t: &mut Value) -> Result<()> {
    r["agent_start_completed"] = json!(true);
    r.as_object_mut()
        .unwrap()
        .remove("startup_confirmation_pending");
    if let Some(error) = r.as_object_mut().unwrap().remove("error") {
        r["startup_error"] = error;
    }
    r["status"] = json!("running");
    t["status"] = json!("running");
    r["progress"] =
        json!("Старт восстановлен: агент и путь задания подтверждены в созданном терминале");
    c.update(r, t)
}
pub fn start(c: &Context, id: &str, reopen: bool, repair: bool) -> Result<Value> {
    let mut t = c.task(id)?;
    ensure!(
        reopen || ["backlog", "ready"].contains(&s(&t, "status")),
        "Запуск разрешён для backlog/ready. Для blocked сохраните answer и продолжите текущий запуск; для прерванной — interrupt"
    );
    for dep in strings(&t["depends"]) {
        ensure!(
            c.task(&dep)?["status"] == "done",
            "Не завершена зависимость: {dep}"
        );
    }
    let mut p = c.read(&c.path("projects", s(&t, "project"))?)?;
    let ns = c.namespace(None)?;
    let mut previous = if let Some(id) = t["active_run"].as_str() {
        let r = c.read(&c.run_file(id)?)?;
        require_absent(c, &r)?;
        Some(r)
    } else {
        None
    };
    let workspace = workspace_binding(c, &mut p, &ns, repair)?;
    if !p["workspace"].is_null() && workspace.is_none() {
        old_workspace_idle(c, &p, &ns, None)?;
    }
    c.save(&c.path("projects", s(&p, "id"))?, &p)?;
    let repos = if t["repositories"].is_null() {
        &p["repositories"]
    } else {
        &t["repositories"]
    };
    ensure!(
        t["code"] != true || !strings(repos).is_empty(),
        "Для изменения кода сначала укажите репозиторий проекта или задачи"
    );
    let repos = if t["code"] == true {
        strings(repos)
            .iter()
            .map(location)
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![]
    };
    for repo in &repos {
        let top = location(git::git(repo, &["rev-parse", "--show-toplevel"], false)?)?;
        ensure!(
            *repo == top,
            "Укажите корень репозитория: {}",
            top.display()
        );
        git::git(repo, &["rev-parse", "--verify", "HEAD"], false)?;
    }
    let id = key("run-");
    let profile = core::profile(c, &t)?;
    extend(&mut t, profile.clone());
    let mut r = json!({"id":id,"task":t["id"],"project":p["id"],"status":"launching","created_at":now(),"updated_at":now(),"repositories":repos,"worktrees":[],"previous_run":t["active_run"],"workspace":null,"tab":null,"pane":null,"worker_pid":null,"herdr_session":ns["session"],"herdr_namespace":ns,"adapter":c.settings["adapter"],"capabilities":external::capabilities(s(&c.settings,"adapter")),"access_policy":if c.settings["adapter"]=="herdr-claude"{"default"}else{"workspace-write"},"program_root":c.program,"state_root":c.root,"bindings_root":c.bindings,"git_mode":t["git_mode"].as_str().unwrap_or("main")});
    extend(&mut r, profile);
    if let Some(prev) = &previous
        && let Some(wts) = prev["worktrees"].as_array().filter(|w| !w.is_empty())
    {
        ensure!(
            wts.iter()
                .map(|w| w["repository"].clone())
                .collect::<Vec<_>>()
                == repos
                    .iter()
                    .take(wts.len())
                    .map(|r| json!(r))
                    .collect::<Vec<_>>(),
            "Набор репозиториев изменился. Сначала явно разберите прежние worktree"
        );
        r["worktrees"] = prev["worktrees"].clone();
    }
    let dir = c.run_file(&id)?.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir)?;
    let fallback = c.root.join("tasks").join(s(&t, "id")).join("plan.md");
    let plan = previous
        .as_ref()
        .map(|p| c.run_file(s(p, "id")).unwrap().with_file_name("plan.md"))
        .filter(|p| p.exists())
        .unwrap_or(fallback);
    if plan.exists() {
        fs::copy(plan, dir.join("plan.md"))?;
    }
    if reopen && let Some(prev) = &mut previous {
        prev["status"] = json!("interrupted");
        prev["interruption"] = json!(
            "open: подтверждено отсутствие исполнителя, контекст сохранён для нового запуска"
        );
        prev["updated_at"] = json!(now());
        c.save(&c.run_file(s(prev, "id"))?, prev)?;
    }
    t["active_run"] = json!(id);
    t["runs"]
        .as_array_mut()
        .ok_or_else(|| anyhow!("runs должен быть списком"))?
        .push(json!(id));
    t["status"] = json!("running");
    c.update(&mut r, &mut t)?;
    match external::detach(c.child().arg("_launch").arg(&id), &dir.join("launcher.log")) {
        Ok(child) => {
            r["worker_pid"] = json!(child);
            r["worker_identity"] = json!(external::identity(child as i64));
            c.update(&mut r, &mut t)?;
        }
        Err(e) => {
            r["status"] = json!("launch_failed");
            r["error"] = json!(e.to_string());
            t["status"] = json!("blocked");
            c.update(&mut r, &mut t)?;
            return Err(e);
        }
    }
    Ok(json!({"run":id,"status":"launching","state":c.run_file(&id)?}))
}
fn summary(c: &Context, t: &Value, r: Option<&Value>, action: &str) -> Result<Value> {
    let mut v = json!({"task":t["id"],"title":t["title"],"status":t["status"],"goal":t["goal"],"action":action});
    let dir = if let Some(r) = r {
        extend(
            &mut v,
            json!({"run":r["id"],"run_status":r["status"],"state":c.run_file(s(r,"id"))?,"workspace":r["workspace"],"tab":r["tab"],"pane":r["pane"]}),
        );
        for k in ["model", "reasoning_effort", "error", "progress"] {
            if let Some(value) = r.get(k) {
                v[k] = value.clone();
            }
        }
        if let Some(prev) = r["previous_run"].as_str() {
            v["previous_run"] = json!(prev);
            let f = c.run_file(prev)?.with_file_name("checkpoint.md");
            if f.exists() {
                v["previous_checkpoint"] = json!(f);
            }
        }
        c.run_file(s(r, "id"))?.parent().unwrap().to_path_buf()
    } else {
        c.root.join("tasks").join(s(t, "id"))
    };
    for k in ["plan", "checkpoint", "result"] {
        let f = dir.join(format!("{k}.md"));
        if f.exists() {
            v[k] = json!(f);
        }
    }
    if r.is_some() && dir.join("questions.json").exists() {
        let q = c.read(&dir.join("questions.json"))?;
        if q["answer"].is_null() {
            v["question"] = q["question"].clone();
            v["checkpoint"] = q["checkpoint"].clone();
        }
    }
    Ok(v)
}
fn open_one(c: &Context, id: &str) -> Result<Value> {
    let t = c.task(id)?;
    let r = t["active_run"]
        .as_str()
        .map(|id| c.read(&c.run_file(id)?))
        .transpose()?;
    if ["done", "cancelled", "review"].contains(&s(&t, "status")) {
        return summary(c, &t, r.as_ref(), "result");
    }
    if r.as_ref().is_some_and(|r| r["status"] == "blocked") {
        return summary(c, &t, r.as_ref(), "awaiting_answer");
    }
    if let Some(mut r) = r {
        let ns = c.namespace(None)?;
        ensure!(
            (!r["herdr_namespace"].is_null() && r["herdr_namespace"] == ns)
                || (r["herdr_namespace"].is_null() && r["herdr_session"] == ns["session"]),
            "Запуск принадлежит другому или неподтверждённому namespace"
        );
        if launcher_alive(&r) {
            c.check_namespace(&r)?;
            return summary(c, &t, Some(&r), "preparing");
        }
        if !r["pane"].is_null() {
            c.check_namespace(&r)?;
            match c.herdr(
                &["pane", "current", "--pane", s(&r, "pane")],
                r["herdr_session"].as_str(),
            ) {
                Ok(v) => {
                    ensure!(
                        v["pane"].get("agent").is_some(),
                        "Herdr не подтвердил состояние прежнего исполнителя"
                    );
                    if !v["pane"]["agent"].is_null() {
                        ensure!(
                            v["pane"]["agent"] == external::agent_kind(c, &r)
                                && agent_state(c, &r)? == "confirmed",
                            "Терминал не подтверждает исполнителя с заданием этого запуска"
                        );
                        let mut t = t;
                        let mut action = "existing";
                        if ["launch_failed", "interrupted", "launching"].contains(&s(&r, "status"))
                        {
                            mark_recovered(c, &mut r, &mut t)?;
                            action = "recovered";
                        }
                        return summary(c, &t, Some(&r), action);
                    }
                }
                Err(e) if error_code(&e) == "pane_not_found" => {}
                Err(e) => return Err(e),
            }
        }
    }
    ensure!(
        ["backlog", "ready", "running", "blocked"].contains(&s(&t, "status")),
        "Открытие недоступно для статуса {}",
        s(&t, "status")
    );
    start(c, id, !t["active_run"].is_null(), true)?;
    let t = c.task(id)?;
    let r = c.read(&c.run_file(s(&t, "active_run"))?)?;
    summary(
        c,
        &t,
        Some(&r),
        if r["previous_run"].is_null() {
            "started"
        } else {
            "restarted"
        },
    )
}
pub fn open(c: &Context, a: &Value) -> Result<Value> {
    c.require_herdr()?;
    let mut results = vec![];
    let mut seen = std::collections::HashSet::new();
    for id in strings(&a["ids"]) {
        if !seen.insert(id.clone()) {
            continue;
        }
        let _lock = c.lock()?;
        match open_one(c, &id) {
            Ok(v) => results.push(v),
            Err(e) => {
                let mut v = json!({"task":id,"action":"error","error":e.to_string(),"error_code":error_code(&e),"next_action":"Прочитайте файлы указанного запуска и docs/recovery.md программы"});
                if let Ok(t) = c.task(&id) {
                    extend(
                        &mut v,
                        crate::args::subset(&t, &["title", "status", "goal"]),
                    );
                    if let Some(run) = t["active_run"].as_str()
                        && let Ok(r) = c.read(&c.run_file(run)?)
                    {
                        extend(
                            &mut v,
                            json!({"run":run,"run_status":r["status"],"state":c.run_file(run)?,"tab":r["tab"],"pane":r["pane"]}),
                        );
                    }
                }
                results.push(v);
            }
        }
    }
    Ok(Value::Array(results))
}

pub fn launch(c: &Context, id: &str) -> Result<Value> {
    let result = launch_inner(c, id);
    if let Err(error) = &result {
        let _lock = c.lock()?;
        let mut r = c.read(&c.run_file(id)?)?;
        let mut t = c.task(s(&r, "task"))?;
        if t["active_run"] == id
            && r["status"] == "launching"
            && r["worker_pid"].as_u64() == Some(std::process::id() as u64)
        {
            r["status"] = json!("launch_failed");
            r["error"] = json!(error.to_string());
            t["status"] = json!("blocked");
            let f = c.run_file(id)?.with_file_name("startup-checkpoint.md");
            store::atomic(&f,format!("# Ошибка подготовки\n\n{error}\n\nСостояние и намерения сохранены в state.json. Не повторяйте create/start вслепую. Сверьте layout и исполнителя в сохранённом namespace. При живом агенте с assignment этого запуска используйте recover-start.\n").as_bytes())?;
            r["checkpoint"] = json!(f);
            c.update(&mut r, &mut t)?;
            core::notification(
                c,
                &format!("Не удалось запустить {}", s(&t, "id")),
                &error.to_string(),
                Some(&r),
            )?;
        }
    }
    result
}
fn launch_inner(c: &Context, id: &str) -> Result<Value> {
    let mut r;
    let mut t;
    {
        let _lock = c.lock()?;
        r = c.read(&c.run_file(id)?)?;
        t = c.task(s(&r, "task"))?;
        c.current(&t, id)?;
        ensure!(
            r["status"] == "launching"
                && r["worker_pid"].as_u64() == Some(std::process::id() as u64)
                && r["worker_identity"] == json!(external::identity(std::process::id() as i64)),
            "Запуск уже подготовлен или принадлежит другому подготовителю"
        );
        c.check_namespace(&r)?;
    }
    let repos = strings(&r["repositories"]);
    let done = r["worktrees"]
        .as_array()
        .ok_or_else(|| anyhow!("Нет worktrees"))?
        .len();
    for (index, repo) in repos.iter().enumerate().skip(done) {
        let wt = location(
            c.machine["worktrees_root"]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or(c.root.join("worktrees")),
        )?
        .join(s(&t, "id"))
        .join(format!("{index}-{id}"));
        let branch = format!("control/{}/{id}", s(&t, "id"));
        fs::create_dir_all(wt.parent().unwrap())?;
        git::git(
            Path::new(repo),
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &wt.to_string_lossy(),
                "HEAD",
            ],
            false,
        )?;
        r["worktrees"].as_array_mut().unwrap().push(json!({"repository":repo,"path":wt,"branch":branch,"main_branch":git::main_branch(Path::new(repo),None)?}));
        let _lock = c.lock()?;
        let mut fresh = c.current(&c.task(s(&r, "task"))?, id)?;
        ensure!(
            fresh["status"] == "launching",
            "Подготовка остановлена: {}",
            s(&fresh, "status")
        );
        fresh["worktrees"] = r["worktrees"].clone();
        r = fresh;
        t = c.task(s(&r, "task"))?;
        c.update(&mut r, &mut t)?;
    }
    for wt in r["worktrees"].as_array().unwrap() {
        ensure!(
            Path::new(s(wt, "path")).is_dir(),
            "Не найден сохранённый worktree: {}",
            s(wt, "path")
        );
    }
    let project = c.read(&c.path("projects", s(&t, "project"))?)?;
    let cwd = if let Some(wt) = r["worktrees"].as_array().unwrap().first() {
        PathBuf::from(s(wt, "path"))
    } else {
        location(
            project["cwd"]
                .as_str()
                .map(PathBuf::from)
                .or_else(|| c.machine["cwd"].as_str().map(PathBuf::from))
                .unwrap_or(c.root.clone()),
        )?
    };
    ensure!(cwd.is_dir(), "Не найден рабочий каталог: {}", cwd.display());
    let layout;
    {
        let _lock = c.lock()?;
        t = c.task(s(&r, "task"))?;
        r = c.current(&t, id)?;
        ensure!(
            r["status"] == "launching",
            "Подготовка остановлена: {}",
            s(&r, "status")
        );
        let mut p = c.read(&c.path("projects", s(&t, "project"))?)?;
        let ns = r["herdr_namespace"].clone();
        let workspace = workspace_binding(c, &mut p, &ns, false)?;
        if !p["workspace"].is_null() && workspace.is_none() {
            old_workspace_idle(c, &p, &ns, Some(id))?;
        }
        r["layout_intent"] = json!({"kind":if workspace.is_some(){"tab"}else{"workspace"},"old_workspace":p["workspace"]});
        p["layout_pending"] = json!(id);
        c.save(&c.path("projects", s(&p, "id"))?, &p)?;
        c.update(&mut r, &mut t)?;
        layout = if let Some(workspace) = workspace {
            r["workspace"] = workspace["workspace_id"].clone();
            c.update(&mut r, &mut t)?;
            c.herdr(
                &[
                    "tab",
                    "create",
                    "--workspace",
                    s(&r, "workspace"),
                    "--cwd",
                    &cwd.to_string_lossy(),
                    "--label",
                    s(&t, "id"),
                    "--no-focus",
                ],
                r["herdr_session"].as_str(),
            )?
        } else {
            let layout = c.herdr(
                &[
                    "workspace",
                    "create",
                    "--cwd",
                    &cwd.to_string_lossy(),
                    "--label",
                    s(&p, "name"),
                    "--no-focus",
                ],
                r["herdr_session"].as_str(),
            )?;
            r["workspace"] = layout["workspace"]["workspace_id"].clone();
            if !p["workspace"].is_null() {
                r["workspace_recovery"] = json!({"old":p["workspace"],"new":r["workspace"],"reason":"workspace_not_found"});
            }
            layout
        };
        r["tab"] = layout["tab"]["tab_id"].clone();
        r["pane"] = layout["root_pane"]["pane_id"].clone();
        ensure!(
            ["workspace", "tab", "pane"]
                .iter()
                .all(|k| !s(&r, k).is_empty()),
            "Herdr не подтвердил созданный layout"
        );
        p["workspace"] = r["workspace"].clone();
        p["workspace_namespace"] = ns;
        c.update(&mut r, &mut t)?;
        p.as_object_mut().unwrap().remove("layout_pending");
        c.save(&c.path("projects", s(&p, "id"))?, &p)?;
    }
    if layout.get("workspace").is_some() {
        c.herdr(
            &["tab", "rename", s(&r, "tab"), s(&t, "id")],
            r["herdr_session"].as_str(),
        )?;
    }
    if r["model"].is_null() || r["reasoning_effort"].is_null() {
        extend(&mut r, core::profile(c, &t)?);
    }
    let assignment = c.run_file(id)?.with_file_name("assignment.md");
    store::atomic(&assignment, assignment_text(c, &r, &t)?.as_bytes())?;
    let argv = external::start_args(c, &mut r, &t, &cwd, &assignment);
    {
        let _lock = c.lock()?;
        let requested = r["requested_agent_session_id"].clone();
        t = c.task(s(&r, "task"))?;
        r = c.current(&t, id)?;
        ensure!(
            r["status"] == "launching",
            "Подготовка остановлена: {}",
            s(&r, "status")
        );
        r["agent_start_intent"] = json!(true);
        r["cwd"] = json!(cwd);
        r["launch_argv"] = json!(argv);
        if !requested.is_null() {
            r["requested_agent_session_id"] = requested;
        }
        c.update(&mut r, &mut t)?;
    }
    if let Err(error) = c.herdr_timeout(
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        r["herdr_session"].as_str(),
        Duration::from_millis(
            c.settings["agent_start_timeout_ms"]
                .as_u64()
                .unwrap()
                .saturating_add(30000),
        ),
    ) {
        let mut state = "unknown";
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(250));
            }
            state = agent_state(c, &r).unwrap_or("unknown");
            if state == "confirmed" {
                break;
            }
        }
        let _lock = c.lock()?;
        t = c.task(s(&r, "task"))?;
        let mut fresh = c.current(&t, id)?;
        fresh["startup_error"] = json!(error.to_string());
        let reported = ["running", "blocked", "review"].contains(&s(&fresh, "status"))
            || !s(&fresh, "progress").is_empty();
        if state != "confirmed" && !reported {
            if state == "absent" {
                return Err(error);
            }
            fresh["startup_confirmation_pending"] = json!(true);
            c.update(&mut fresh, &mut t)?;
            return Ok(json!({"run":id,"status":"launching"}));
        }
        c.update(&mut fresh, &mut t)?;
    }
    {
        let _lock = c.lock()?;
        let mut fresh = c.read(&c.run_file(id)?)?;
        t = c.task(s(&r, "task"))?;
        c.current(&t, id)?;
        fresh["agent_start_completed"] = json!(true);
        fresh
            .as_object_mut()
            .unwrap()
            .remove("startup_confirmation_pending");
        if fresh["status"] == "launching" {
            fresh["status"] = json!("running");
        }
        c.update(&mut fresh, &mut t)?;
        r = fresh;
    }
    Ok(json!({"run":id,"status":r["status"]}))
}
fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}
pub fn entry_command(c: &Context) -> String {
    let mut args = vec![
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        "--state-root".into(),
        c.root.to_string_lossy().to_string(),
        "--bindings-root".into(),
        c.bindings.to_string_lossy().to_string(),
        "--program-root".into(),
        c.program.to_string_lossy().to_string(),
    ];
    if let Some(p) = &c.config {
        args.extend(["--config".into(), p.to_string_lossy().into()]);
    }
    args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
}
pub fn assignment_text(c: &Context, r: &Value, t: &Value) -> Result<String> {
    let dir = c.run_file(s(r, "id"))?.parent().unwrap().to_path_buf();
    let previous = if let Some(id) = r["previous_run"].as_str() {
        format!(
            "{}\nПри восстановлении сначала прочитай checkpoint.md предыдущего запуска, затем state.json, questions.json, progress.md и result.md.",
            c.run_file(id)?.parent().unwrap().display()
        )
    } else {
        "нет".into()
    };
    Ok(format!(
        "# Задача {}: {}\n\nЦель: {}\n\nКритерии готовности: {}\n\nСостояние панели: {}\nРесурсы программы: {}\nЗапуск: {}\nАдаптер: {}\nМодель: {}; reasoning effort: {}.\nПроект: {}\nРабочие деревья: {}\nЗавершение Git: {}; контракт: {}\nПриёмка: {}.\n\nДо работы прочитай контракт исполнителя: {}.\nПравила планирования: {}.\nПрочитай {}, если он есть; иначе {}.\nПредыдущий запуск: {}.\nАртефакты и checkpoint текущего запуска сохраняй в {}.\n\nКоманда панели (из любого cwd): {}\nВ progress/block/finish/memory-add указывай --run {}.\nДля памяти: memory-list --project {} --query 'тема' --limit 10; читай только нужные записи.\nСамостоятельный finish оставляет review; complete требует поручения пользователя.\n",
        s(t, "id"),
        s(t, "title"),
        s(t, "goal"),
        s(t, "acceptance"),
        c.root.display(),
        c.program.display(),
        s(r, "id"),
        s(r, "adapter"),
        s(r, "model"),
        s(r, "reasoning_effort"),
        c.path("projects", s(t, "project"))?.display(),
        r["worktrees"],
        s(t, "git_mode"),
        c.program.join("docs/git-policy.md").display(),
        s(t, "reviewer"),
        c.program.join("docs/worker.md").display(),
        c.program.join("docs/planning.md").display(),
        dir.join("plan.md").display(),
        c.root
            .join("tasks")
            .join(s(t, "id"))
            .join("plan.md")
            .display(),
        previous,
        dir.display(),
        entry_command(c),
        s(r, "id"),
        s(t, "project")
    ))
}
pub fn plan(c: &Context, a: &Value) -> Result<Value> {
    let t = c.task(s(a, "id"))?;
    let target = a["pane"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            std::env::var("HERDR_PANE_ID")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .ok_or_else(|| {
            anyhow!("Для просмотра вне herdr укажите --herdr-session NAME и --pane ID нужного таба")
        })?;
    c.herdr(&["pane", "get", &target], None)?;
    let dir = if let Some(run) = t["active_run"].as_str() {
        c.run_file(run)?.parent().unwrap().to_path_buf()
    } else {
        c.root.join("tasks").join(s(&t, "id"))
    };
    let file = if let Some(file) = a["file"].as_str() {
        let f = location(file)?;
        ensure!(f.is_file(), "Не найден файл плана");
        f
    } else if dir.join("plan.md").is_file() {
        dir.join("plan.md")
    } else {
        let mut content = format!(
            "# {}: {}\n\nСтатус: {}\n\nОтдельный plan.md пока не сохранён. Ниже цель, критерии и checkpoint задачи.\n\n## Цель\n\n{}\n\n## Критерии готовности\n\n{}\n",
            s(&t, "id"),
            s(&t, "title"),
            s(&t, "status"),
            s(&t, "goal"),
            s(&t, "acceptance")
        );
        if dir.join("questions.json").is_file() {
            let q = c.read(&dir.join("questions.json"))?;
            content.push_str(&format!(
                "\n## Checkpoint\n\n{}\n\n## Вопросы\n\n{}\n",
                s(&q, "checkpoint"),
                s(&q, "question")
            ));
            if !q["answer"].is_null() {
                content.push_str(&format!("\n## Ответ\n\n{}\n", s(&q, "answer")));
            }
        }
        let f = dir.join("plan-view.md");
        store::atomic(&f, content.as_bytes())?;
        f
    };
    let pager = crate::shipping::which("less")
        .or_else(|| crate::shipping::which("more"))
        .ok_or_else(|| anyhow!("Для просмотра установите less или more"))?;
    let layout = c.herdr(
        &[
            "pane",
            "split",
            "--pane",
            &target,
            "--direction",
            "right",
            "--cwd",
            &c.root.to_string_lossy(),
            "--no-focus",
        ],
        None,
    )?;
    let pane = s(&layout["pane"], "pane_id");
    ensure!(!pane.is_empty(), "Нет pane_id");
    let view = json!({"task":t["id"],"pane":pane,"parent_pane":target,"file":file,"herdr_session":c.session,"created_at":now()});
    c.save(
        &c.root.join("views").join(s(&t, "id")).join("state.json"),
        &view,
    )?;
    c.herdr(
        &["pane", "rename", pane, &format!("План: {}", s(&t, "id"))],
        None,
    )?;
    c.herdr(
        &[
            "pane",
            "run",
            pane,
            &format!(
                "{} -- {}",
                quote(&pager.to_string_lossy()),
                quote(&file.to_string_lossy())
            ),
        ],
        None,
    )?;
    Ok(view)
}
fn saved_tab(c: &Context, r: &Value) -> Result<Option<Value>> {
    c.check_namespace(r)?;
    match c.herdr(&["tab", "get", s(r, "tab")], r["herdr_session"].as_str()) {
        Ok(v) => Ok(Some(v["tab"].clone())),
        Err(e) if error_code(&e) == "tab_not_found" => Ok(None),
        Err(e) => Err(e),
    }
}
fn request_file(c: &Context, r: &Value) -> Result<PathBuf> {
    Ok(c.run_file(s(r, "id"))?
        .with_file_name("completion-request.json"))
}
fn queue_completion(c: &Context, a: &Value, r: &Value) -> Result<Value> {
    let file = request_file(c, r)?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut request = json!({"token":token,"run":r["id"],"task":a["id"],"status":"pending","requested_at":now(),"by":a["by"],"note":a["note"],"target_status":a["status"],"commit_message":a["commit_message"]});
    c.save(&file, &request)?;
    let mut cmd = c.child();
    cmd.arg("_complete-request")
        .arg(s(r, "id"))
        .arg(&token)
        .current_dir(&c.root);
    for k in ["HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"] {
        cmd.env_remove(k);
    }
    match external::detach(&mut cmd, &file.with_extension("log")) {
        Ok(child) => {
            request["pid"] = json!(child);
            request["identity"] = json!(external::identity(child as i64));
            c.save(&file, &request)?;
        }
        Err(e) => {
            request["status"] = json!("failed");
            request["error"] = json!(e.to_string());
            request["finished_at"] = json!(now());
            c.save(&file, &request)?;
            return Err(e);
        }
    }
    Ok(
        json!({"task":a["id"],"run":r["id"],"status":"completion_requested","request":file,"next":"Итог сохранится в completion-request.json и state.json; при сбое повторите complete"}),
    )
}
pub fn complete_request(c: &Context, a: &Value) -> Result<Value> {
    let r = c.read(&c.run_file(s(a, "run"))?)?;
    let file = request_file(c, &r)?;
    let mut request = c.read(&file)?;
    ensure!(
        request["token"] == a["token"] && request["status"] == "pending",
        "Запрос завершения заменён или уже обработан"
    );
    request["status"] = json!("running");
    request["started_at"] = json!(now());
    c.save(&file, &request)?;
    let result = (|| {
        c.current(&c.task(s(&r, "task"))?, s(a, "run"))?;
        let args = json!({"id":r["task"],"status":request["target_status"],"by":request["by"],"note":request["note"],"commit_message":request["commit_message"]});
        complete_locked(c, &args)
    })();
    match result {
        Ok(v) if !failure(&v) => request["status"] = json!("completed"),
        Ok(_) => {
            request["status"] = json!("failed");
            request["error"] =
                json!("Завершение не выполнено; проверьте git_completion и cleanup в state.json");
        }
        Err(e) => {
            request["status"] = json!("failed");
            request["error"] = json!(e.to_string());
        }
    }
    request["finished_at"] = json!(now());
    c.save(&file, &request)?;
    Ok(request)
}
pub fn failure(v: &Value) -> bool {
    v["action"] == "error"
        || v["git_completion"]["status"] == "failed"
        || v["cleanup"]["status"] == "failed"
        || v.as_array().is_some_and(|a| a.iter().any(failure))
}
pub fn complete(c: &Context, a: &Value) -> Result<Value> {
    let t = c.task(s(a, "id"))?;
    let r = t["active_run"]
        .as_str()
        .map(|run| c.current(&t, run))
        .transpose()?;
    if let Some(r) = &r {
        let file = request_file(c, r)?;
        if file.exists() {
            let mut req = c.read(&file)?;
            if ["pending", "running"].contains(&s(&req, "status")) {
                if external::request_alive(&req) {
                    ensure!(
                        a["status"] == req["target_status"] && a["by"] == req["by"],
                        "Уже выполняется другой запрос завершения"
                    );
                    return Ok(
                        json!({"task":a["id"],"status":"completion_requested","request":file}),
                    );
                }
                req["status"] = json!("interrupted");
                req["finished_at"] = json!(now());
                req["error"] =
                    json!("Процесс завершения исчез; повторный complete восстанавливает операцию");
                c.save(&file, &req)?;
            }
        }
    }
    let result = complete_locked(c, a);
    if let Err(e) = &result
        && let Some(r) = &r
        && std::env::var("HERDR_PANE_ID").ok().as_deref() == r["pane"].as_str()
        && r["pane"].is_string()
    {
        let file = request_file(c, r)?;
        if !file.exists() || !["pending", "running"].contains(&s(&c.read(&file)?, "status")) {
            c.save(&file,&json!({"token":uuid::Uuid::new_v4().simple().to_string(),"run":r["id"],"task":a["id"],"status":"failed","target_status":a["status"],"by":a["by"],"note":a["note"],"commit_message":a["commit_message"],"requested_at":now(),"finished_at":now(),"error":e.to_string()}))?;
        }
    }
    result
}
fn complete_locked(c: &Context, a: &Value) -> Result<Value> {
    let mut t = c.task(s(a, "id"))?;
    let mut r = t["active_run"]
        .as_str()
        .map(|run| c.current(&t, run))
        .transpose()?;
    let status = s(a, "status");
    ensure!(
        !["done", "cancelled"].contains(&s(&t, "status")) || t["status"] == status,
        "Задача уже завершена с другим статусом"
    );
    if status == "done" {
        ensure!(
            ["review", "done"].contains(&s(&t, "status")),
            "Сначала нужен результат в review; для отмены используйте --status cancelled"
        );
        ensure!(
            t["reviewer"] == a["by"],
            "Принимает назначенный reviewer: {}",
            s(&t, "reviewer")
        );
    }
    let mut tab = None;
    if let Some(r) = &r {
        ensure!(
            r["status"] != "launching" || !launcher_alive(r),
            "Подготовитель ещё работает; завершение повторите после подготовки запуска"
        );
        if let (Some(saved), Some(selected)) = (r["herdr_session"].as_str(), c.session.as_deref()) {
            ensure!(
                saved == selected,
                "Используйте сохранённую сессию запуска: {saved}"
            );
        }
        if !r["tab"].is_null() {
            tab = saved_tab(c, r)?;
            if let Some(tab) = &tab {
                ensure!(
                    tab["workspace_id"] == r["workspace"],
                    "Таб не принадлежит сохранённому workspace"
                );
                let panes = c.herdr(
                    &["pane", "list", "--workspace", s(r, "workspace")],
                    r["herdr_session"].as_str(),
                )?;
                let members = panes["panes"]
                    .as_array()
                    .ok_or_else(|| anyhow!("Некорректный список panes"))?
                    .iter()
                    .filter(|p| p["tab_id"] == r["tab"])
                    .collect::<Vec<_>>();
                ensure!(
                    members.iter().any(|p| p["pane_id"] == r["pane"]),
                    "Сохранённая панель исполнителя отсутствует в табе; проверьте его вручную"
                );
                ensure!(
                    !members
                        .iter()
                        .any(|p| !p["agent"].is_null() && p["pane_id"] != r["pane"]),
                    "В табе есть другой агент; сначала отделите его от таба задачи"
                );
                if let Ok(caller) = std::env::var("HERDR_PANE_ID")
                    && members.iter().any(|p| p["pane_id"] == caller)
                {
                    ensure!(
                        r["pane"] == caller,
                        "Текущий агент находится в чужой панели закрываемого таба"
                    );
                    return queue_completion(c, a, r);
                }
            }
        }
    }
    if let Some(r) = &mut r
        && status == "done"
        && git::complete_code(c, &mut t, r, a["commit_message"].as_str()).is_err()
    {
        let mut result = json!({"task":t["id"],"status":t["status"],"next":"Код и таб сохранены; устраните причину и повторите complete"});
        result["git_completion"] = r["git_completion"].clone();
        return Ok(result);
    }
    if t["status"] != status {
        if status == "done" {
            let r = r
                .as_mut()
                .ok_or_else(|| anyhow!("Нет запуска для приёмки"))?;
            core::accept(c, &mut t, r, s(a, "by"), s(a, "note"))?;
        } else {
            t["status"] = json!("cancelled");
            t["cancelled"] = json!({"note":a["note"],"at":now()});
            if let Some(r) = &mut r {
                r["status"] = json!("cancelled");
                c.update(r, &mut t)?;
            } else {
                t["updated_at"] = json!(now());
                c.save(&c.path("tasks", s(&t, "id"))?, &t)?;
            }
        }
    }
    let Some(r) = &mut r else {
        return Ok(json!({"task":t["id"],"status":t["status"],"cleanup":"no_run"}));
    };
    r["cleanup"] = json!({"tab":r["tab"],"pane":r["pane"],"at":now(),"status":"pending"});
    c.update(r, &mut t)?;
    let cleanup = (|| {
        if tab.is_some() {
            c.herdr(&["tab", "close", s(r, "tab")], r["herdr_session"].as_str())?;
            ensure!(
                saved_tab(c, r)?.is_none(),
                "herdr не подтвердил закрытие таба"
            );
        }
        Ok(())
    })();
    match cleanup {
        Ok(()) => {
            r["cleanup"]["status"] = json!(if r["tab"].is_null() {
                "no_tab"
            } else {
                "closed"
            })
        }
        Err(e) => {
            r["cleanup"]["status"] = json!("failed");
            r["cleanup"]["error"] = json!(e.to_string());
        }
    }
    c.update(r, &mut t)?;
    Ok(json!({"task":t["id"],"run":r["id"],"status":t["status"],"cleanup":r["cleanup"]}))
}
