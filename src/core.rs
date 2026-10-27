use crate::{
    args::{s, strings, subset},
    config::{Context, location},
    external, git, lifecycle, review,
    store::{self, key, now},
};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

pub fn profile(c: &Context, t: &Value) -> Result<Value> {
    profile_for_run(c, t, None)
}
pub fn profile_for_run(c: &Context, t: &Value, explicit: Option<&str>) -> Result<Value> {
    let title = crate::casefold::fold(s(t, "title"));
    let title = title.split("по результатам").next().unwrap_or(&title);
    let kind = t["task_kind"].as_str().unwrap_or_else(|| {
        if ["исслед", "эксперимент", "research", "experiment"]
            .iter()
            .any(|w| title.contains(w))
        {
            "research"
        } else if ["план", "архитектур", "planning", "design"]
            .iter()
            .any(|w| title.contains(w))
        {
            "planning"
        } else if ["рассуж", "анализ", "оценить", "reasoning", "analysis"]
            .iter()
            .any(|w| title.contains(w))
        {
            "reasoning"
        } else {
            "implementation"
        }
    });
    let complexity = t["complexity"].as_str().unwrap_or("normal");
    ensure!(
        ["implementation", "research", "reasoning", "planning"].contains(&kind)
            && ["normal", "complex"].contains(&complexity),
        "Неизвестный тип или сложность задачи"
    );
    crate::policy::resolve(c, t, kind, complexity, explicit)
}
pub fn extend(v: &mut Value, extra: Value) {
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
}
fn task_response(t: Value, compact: bool) -> Value {
    if !compact {
        return t;
    }
    let mut v = subset(
        &t,
        &[
            "id",
            "project",
            "title",
            "status",
            "code",
            "active_run",
            "task_kind",
            "complexity",
            "model",
            "reasoning_effort",
            "profile",
            "model_selection",
            "reviewer",
            "agent_review_policy",
            "git_mode",
        ],
    );
    v["details"] = json!(format!("show tasks {}", s(&t, "id")));
    v
}
/// Executes one command. Multi-step lifecycle operations own their lock scopes.
pub fn execute(c: &Context, name: &str, a: &Value) -> Result<Value> {
    crate::args::validate(name, a)?;
    match name {
        "start" => {
            c.require_herdr()?;
            let _lock = c.lock()?;
            lifecycle::start(c, s(a, "id"), false, false, a["profile"].as_str())
        }
        "open" => lifecycle::open(c, a),
        "_launch" => lifecycle::launch(c, s(a, "run")),
        "plan" => lifecycle::plan(c, a),
        "mcp" => bail!("mcp — отдельный транспорт"),
        "install" | "package" => crate::shipping::install(c, s(a, "prefix"), name == "package"),
        "doctor" => crate::shipping::doctor(c),
        "skills" => crate::shipping::skills(c, s(a, "destination"), s(a, "action")),
        "_complete-request" => {
            let _lock = c.lock()?;
            lifecycle::complete_request(c, a)
        }
        "complete" => {
            let _lock = c.lock()?;
            lifecycle::complete(c, a)
        }
        name if name.starts_with("review-") => {
            let _lock = c.lock()?;
            review::handle(c, name, a)
        }
        _ => {
            let _lock = if [
                "environment",
                "projects",
                "tasks",
                "show",
                "run",
                "memory-list",
                "notifications",
            ]
            .contains(&name)
            {
                None
            } else {
                Some(c.lock()?)
            };
            execute_simple(c, name, a)
        }
    }
}
fn execute_simple(c: &Context, name: &str, a: &Value) -> Result<Value> {
    let id = s(a, "id");
    match name {
        "environment" => Ok(
            json!({"program_root":c.program,"state_root":c.root,"bindings_root":c.bindings,"config":c.config,"model_policy":c.settings["model_policy_origin"],"model_policy_enabled":c.settings["model_policy_enabled"],"model_profiles":c.settings["resolved_model_policy"],"adapter":c.settings["adapter"],"git_mode":c.settings["git_mode"],"documents":(["worker.md","planning.md","git-policy.md","recovery.md"].into_iter().map(|n|(n,c.program.join("docs").join(n))).collect::<std::collections::BTreeMap<_,_>>())}),
        ),
        "project-create" => {
            ensure!(id != "shared", "ID shared зарезервирован для общей памяти");
            let file = c.path("projects", id)?;
            ensure!(!file.exists(), "Проект с таким ID уже существует");
            ensure!(
                !c.rows("projects")?
                    .iter()
                    .any(|p| crate::casefold::fold(s(p, "name"))
                        == crate::casefold::fold(s(a, "name"))),
                "Проект с таким названием уже существует"
            );
            let p = json!({"id":id,"name":a["name"],"description":a["description"],"repositories":[],"workspace":null,"created_at":now()});
            c.save(&file, &p)?;
            Ok(p)
        }
        "project-set" => {
            let file = c.path("projects", id)?;
            let mut p = c.read(&file)?;
            for k in ["name", "description"] {
                if !a[k].is_null() {
                    p[k] = a[k].clone();
                }
            }
            if !a["repo"].is_null() {
                let mut repos = vec![];
                for path in strings(&a["repo"]) {
                    let r = location(path)?;
                    let top =
                        location(git::git(&r, &["rev-parse", "--show-toplevel"], false)?.trim())?;
                    ensure!(top == r, "Укажите корень репозитория: {}", top.display());
                    if !repos.contains(&r) {
                        repos.push(r);
                    }
                }
                p["repositories"] = json!(repos);
            }
            if !a["cwd"].is_null() {
                let cwd = location(s(a, "cwd"))?;
                ensure!(cwd.is_dir(), "Не найден рабочий каталог: {}", cwd.display());
                p["cwd"] = json!(cwd);
            }
            if !a["workspace"].is_null() {
                let ns = c.namespace(None)?;
                let live = c.herdr(
                    &["workspace", "get", s(a, "workspace")],
                    ns["session"].as_str(),
                )?;
                p["workspace"] = live["workspace"]["workspace_id"].clone();
                p["workspace_namespace"] = ns;
            }
            if a["clear_workspace"] == true {
                p["workspace"] = Value::Null;
                p["workspace_namespace"] = Value::Null;
            }
            c.save(&file, &p)?;
            Ok(p)
        }
        "task-create" => {
            c.read(&c.path("projects", s(a, "project"))?)?;
            let file = c.path("tasks", id)?;
            ensure!(!file.exists(), "Такая задача уже существует");
            for dep in strings(&a["depends"]) {
                ensure!(dep != id, "Задача не может зависеть от себя");
                c.task(&dep)?;
            }
            let mut t = json!({"id":id,"project":a["project"],"title":a["title"],"goal":a["goal"],"acceptance":a["acceptance"],"status":"backlog","code":a["code"],"repositories":a["repo"],"depends":a["depends"],"reviewer":"user","git_mode":a["git_mode"].as_str().unwrap_or(s(&c.settings,"git_mode")),"agent_review_policy":a["agent_review_policy"],"active_run":null,"runs":[],"created_at":now(),"updated_at":now(),"task_kind":a["task_kind"],"complexity":a["complexity"],"profile":a["profile"],"profile_source":"task"});
            if a["profile"] == "auto" {
                t["profile"] = Value::Null;
                t["profile_source"] = json!("rule");
            }
            let p = profile(c, &t)?;
            extend(&mut t, p);
            c.save(&file, &t)?;
            Ok(task_response(t, a["compact"] == true))
        }
        "task-set" => {
            let mut t = c.task(id)?;
            ensure!(
                !["running", "blocked"].contains(&s(&t, "status")),
                "Для текущего запуска используйте progress/block/answer/interrupt"
            );
            for k in [
                "title",
                "goal",
                "acceptance",
                "reviewer",
                "git_mode",
                "agent_review_policy",
                "code",
                "task_kind",
                "complexity",
                "status",
            ] {
                if !a[k].is_null() {
                    t[k] = a[k].clone();
                }
            }
            if !a["repo"].is_null() {
                t["repositories"] = json!(
                    strings(&a["repo"])
                        .iter()
                        .map(location)
                        .collect::<Result<Vec<_>>>()?
                );
            }
            if let Some(name) = a["profile"].as_str() {
                t["profile"] = if name == "auto" {
                    Value::Null
                } else {
                    json!(name)
                };
                t["profile_source"] = json!("task");
            }
            let p = profile(c, &t)?;
            extend(&mut t, p);
            t["updated_at"] = json!(now());
            c.save(&c.path("tasks", id)?, &t)?;
            Ok(task_response(t, a["compact"] == true))
        }
        "projects" | "tasks" => {
            let mut rows = c.rows(name)?;
            if name == "tasks" {
                rows.retain(|t| {
                    (a["project"].is_null() || t["project"] == a["project"])
                        && (a["status"].is_null() || t["status"] == a["status"])
                        && (a["all"] == true || !["done", "cancelled"].contains(&s(t, "status")))
                });
                Ok(Value::Array(
                    rows.iter()
                        .map(|t| subset(t, &["id", "project", "title", "status", "active_run"]))
                        .collect::<Vec<_>>(),
                ))
            } else {
                Ok(Value::Array(
                    rows.iter()
                        .map(|p| subset(p, &["id", "name", "description", "workspace"]))
                        .collect::<Vec<_>>(),
                ))
            }
        }
        "show" => c.read(&c.path(s(a, "kind"), id)?),
        "run" => {
            let mut r = c.read(&c.run_file(id)?)?;
            let f = c.run_file(id)?.with_file_name("completion-request.json");
            if f.exists() {
                let req = c.read(&f)?;
                r["completion_process_alive"] = json!(external::request_alive(&req));
                r["completion_request"] = req;
            }
            r["launcher_alive"] = json!(external::launcher_alive(&r));
            let updated = chrono::DateTime::parse_from_rfc3339(s(&r, "updated_at"))?;
            r["seconds_since_update"] =
                json!((chrono::Utc::now() - updated.with_timezone(&chrono::Utc)).num_seconds());
            r["note"] = json!(
                "launcher_alive относится только к подготовителю; состояние исполнителя подтверждается его файлами, а не этим PID"
            );
            if a["compact"] == true {
                let mut v = subset(
                    &r,
                    &[
                        "id",
                        "task",
                        "status",
                        "updated_at",
                        "pane",
                        "tab",
                        "launcher_alive",
                        "seconds_since_update",
                        "completion_process_alive",
                        "error",
                        "startup_error",
                        "note",
                    ],
                );
                v["details"] = json!(format!("run {id}"));
                v["files"] = json!({});
                for n in [
                    "progress.md",
                    "questions.json",
                    "result.md",
                    "checkpoint.md",
                ] {
                    let f = c.run_file(id)?.with_file_name(n);
                    if f.exists() {
                        v["files"][n] = json!(f);
                    }
                }
                Ok(v)
            } else {
                Ok(r)
            }
        }
        "progress" | "block" | "finish" | "recover-start" => worker(c, name, a),
        "answer" => {
            let mut t = c.task(id)?;
            ensure!(t["status"] == "blocked", "Задача не заблокирована");
            let mut r = c.current(&t, s(&t, "active_run"))?;
            ensure!(
                r["status"] != "launch_failed",
                "После ошибки запуска используйте interrupt с объяснением; проверьте созданную панель перед повтором"
            );
            let f = c.run_file(s(&r, "id"))?.with_file_name("questions.json");
            let mut q = c.read(&f)?;
            q["answer"] = a["text"].clone();
            q["answered_at"] = json!(now());
            c.save(&f, &q)?;
            r["status"] = json!("running");
            t["status"] = json!("running");
            c.update(&mut r, &mut t)?;
            Ok(
                json!({"task":id,"run":r["id"],"status":"running","pane":r["pane"],"questions":f,"next":"Продолжите текущий ход исполнителя в сохранённой панели, прочитав questions.json и checkpoint"}),
            )
        }
        "interrupt" => {
            let mut t = c.task(id)?;
            ensure!(
                ["running", "blocked"].contains(&s(&t, "status")),
                "Нет активной/заблокированной работы"
            );
            let mut r = c.current(&t, s(&t, "active_run"))?;
            ensure!(
                r["status"] != "launching" || !external::launcher_alive(&r),
                "Подготовитель ещё работает"
            );
            r["status"] = json!("interrupted");
            r["interruption"] = a["reason"].clone();
            t["status"] = json!("ready");
            c.update(&mut r, &mut t)?;
            Ok(
                json!({"task":id,"status":"ready","warning":"Процесс herdr не остановлен. Перед повторным запуском убедитесь, что прежний исполнитель прекратил работу"}),
            )
        }
        "accept" => {
            let mut t = c.task(id)?;
            let mut r = c.current(&t, s(&t, "active_run"))?;
            accept(c, &mut t, &mut r, s(a, "by"), s(a, "note"))?;
            Ok(t)
        }
        "memory-add" => {
            let project = s(a, "project");
            if project != "shared" {
                c.read(&c.path("projects", project)?)?;
            }
            if let Some(run) = a["run"].as_str() {
                let r = c.read(&c.run_file(run)?)?;
                c.current(&c.task(s(&r, "task"))?, run)?;
                ensure!(
                    project == "shared" || r["project"] == project,
                    "Проект не совпадает с запуском"
                );
            }
            let content = if let Some(file) = a["file"].as_str() {
                fs::read_to_string(file)?
            } else {
                s(a, "text").into()
            };
            let file = memory_write(
                c,
                project,
                s(a, "title"),
                &content,
                s(a, "kind"),
                a["run"].as_str(),
                a["file"].as_str().map(Path::new),
            )?;
            Ok(json!({"file":file}))
        }
        "memory-list" => {
            let project = store::ident(s(a, "project"))?;
            let mut projects = vec!["shared"];
            if project != "shared" {
                projects.push(project);
            }
            let query = crate::casefold::fold(s(a, "query"));
            let mut rows = vec![];
            for p in projects {
                for f in store::json_files(&c.root.join("memory").join(p))? {
                    let m = c.read(&f)?;
                    if query.is_empty()
                        || crate::casefold::fold(&format!(
                            "{} {}",
                            s(&m, "title"),
                            fs::read_to_string(c.root.join(s(&m, "file")))?
                        ))
                        .contains(&query)
                    {
                        rows.push(m);
                    }
                }
            }
            if let Some(limit) = a["limit"].as_u64() {
                rows.sort_by(|a, b| {
                    (s(b, "created_at"), s(b, "id")).cmp(&(s(a, "created_at"), s(a, "id")))
                });
                let offset = a["offset"].as_u64().unwrap_or(0);
                let total = rows.len() as u64;
                let items = rows
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect::<Vec<_>>();
                let mut result = json!({"total":total,"next_offset":if offset.saturating_add(limit)<total{Some(offset+limit)}else{None}});
                result["items"] = Value::Array(items);
                Ok(result)
            } else {
                Ok(Value::Array(rows))
            }
        }
        "notifications" => Ok(Value::Array(
            store::json_files(&c.root.join("notifications"))?
                .iter()
                .map(|f| {
                    let mut v = c.read(f)?;
                    v["id"] = json!(f.file_stem().unwrap().to_string_lossy());
                    Ok(v)
                })
                .collect::<Result<Vec<Value>>>()?
                .into_iter()
                .filter(|v| v["acknowledged"].is_null() || v["acknowledged"] == false)
                .collect::<Vec<_>>(),
        )),
        "notice-ack" => {
            let file = c.path("notifications", id)?;
            let mut n = c.read(&file)?;
            n["acknowledged"] = json!(now());
            c.save(&file, &n)?;
            Ok(n)
        }
        "materialize-bindings" => {
            let mut n = 0;
            for k in ["projects", "tasks"] {
                for f in store::json_files(&c.root.join(k))? {
                    store::save_raw(&f, &c.read(&f)?)?;
                    n += 1;
                }
            }
            Ok(json!({"records":n,"state_root":c.root,"bindings_preserved":true}))
        }
        _ => bail!("Неизвестная команда: {name}"),
    }
}
fn worker(c: &Context, name: &str, a: &Value) -> Result<Value> {
    let run = s(a, "run");
    let mut r = c.read(&c.run_file(run)?)?;
    let mut t = c.task(s(&r, "task"))?;
    c.current(&t, run)?;
    let dir = c.run_file(run)?.parent().unwrap().to_path_buf();
    if name == "recover-start" {
        ensure!(
            ["launch_failed", "interrupted"].contains(&s(&r, "status")),
            "Восстановление старта доступно только для launch_failed/interrupted"
        );
        ensure!(!external::launcher_alive(&r), "Подготовитель ещё работает");
        ensure!(
            lifecycle::agent_state(c, &r)? == "confirmed",
            "Терминал не подтверждает исполнителя с заданием этого запуска"
        );
        lifecycle::mark_recovered(c, &mut r, &mut t)?;
        return Ok(json!({"run":run,"status":"running","pane":r["pane"]}));
    }
    ensure!(
        ["launching", "running"].contains(&s(&r, "status"))
            || (name == "finish" && r["status"] == "launch_failed"),
        "Запуск не выполняется"
    );
    match name {
        "progress" => {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("progress.md"))?;
            writeln!(f, "\n- {}: {}", now(), s(a, "text"))?;
            r["progress"] = a["text"].clone();
            c.update(&mut r, &mut t)?;
            Ok(json!({"run":run,"progress":a["text"]}))
        }
        "block" => {
            let checkpoint = dir.join("checkpoint.md");
            store::atomic(&checkpoint, format!("{}\n", s(a, "checkpoint")).as_bytes())?;
            c.save(&dir.join("questions.json"),&json!({"question":a["question"],"checkpoint":a["checkpoint"],"checkpoint_path":checkpoint,"answer":null,"created_at":now()}))?;
            r["checkpoint"] = json!(checkpoint);
            r["status"] = json!("blocked");
            t["status"] = json!("blocked");
            c.update(&mut r, &mut t)?;
            let event = notification(
                c,
                &format!("Нужен ответ: {}", s(&t, "title")),
                s(a, "question"),
                Some(&r),
            )?;
            Ok(json!({"run":run,"status":"blocked","notification":event}))
        }
        "finish" => {
            let source = Path::new(s(a, "report"));
            let report = fs::read_to_string(source)?;
            ensure!(!report.trim().is_empty(), "Отчёт не может быть пустым");
            let result_file = dir.join("result.md");
            let report = relocate(&report, source, &result_file)?;
            store::atomic(&result_file, report.as_bytes())?;
            memory_write(
                c,
                s(&t, "project"),
                &format!("{}: выполненная работа", s(&t, "title")),
                &report,
                "work",
                Some(run),
                Some(&result_file),
            )?;
            if let Some(file) = a["decisions"].as_str() {
                let text = fs::read_to_string(file)?;
                if !text.trim().is_empty() {
                    memory_write(
                        c,
                        s(&t, "project"),
                        &format!("{}: решения", s(&t, "title")),
                        &text,
                        "decision",
                        Some(run),
                        Some(Path::new(file)),
                    )?;
                }
            }
            if r["status"] == "launch_failed"
                && let Some(error) = r.as_object_mut().unwrap().remove("error")
            {
                r["startup_error"] = error;
            }
            r["status"] = json!("review");
            t["status"] = json!("review");
            r["result_revision"] = json!(r["result_revision"].as_u64().unwrap_or(0) + 1);
            if !a["artifact"].is_null() {
                r["review_artifacts"] = json!(
                    strings(&a["artifact"])
                        .iter()
                        .map(crate::config::absolute)
                        .collect::<Result<Vec<_>>>()?
                );
            }
            c.update(&mut r, &mut t)?;
            let mut agent_review = Value::Null;
            if t["agent_review_policy"] == "auto" {
                agent_review = match review::request(c, &t, &r) {
                    Ok(v) => v,
                    Err(e) => {
                        let v = json!({"status":"failed","error":e.to_string()});
                        c.save(&dir.join("agent-review-error.json"), &v)?;
                        v
                    }
                };
            }
            notification(
                c,
                &format!("Готово к приёмке: {}", s(&t, "title")),
                &result_file.to_string_lossy(),
                Some(&r),
            )?;
            let mut result = json!({"task":t["id"],"status":"review"});
            result["agent_review"] = agent_review;
            Ok(result)
        }
        _ => unreachable!(),
    }
}
pub fn accept(c: &Context, t: &mut Value, r: &mut Value, by: &str, note: &str) -> Result<()> {
    ensure!(
        t["status"] == "review",
        "Приёмка доступна только для review"
    );
    ensure!(
        t["reviewer"] == by,
        "Принимает назначенный reviewer: {}",
        s(t, "reviewer")
    );
    t["status"] = json!("done");
    r["status"] = json!("done");
    t["accepted"] = json!({"by":by,"note":note,"at":now()});
    c.update(r, t)
}
pub fn notification(c: &Context, title: &str, body: &str, r: Option<&Value>) -> Result<Value> {
    let mut n = json!({"title":title,"body":body,"created_at":now(),"delivered":false,"run":r.map(|v|&v["id"])});
    let send = (|| {
        if let Some(r) = r {
            c.check_namespace(r)?;
        }
        c.herdr(
            &[
                "notification",
                "show",
                title,
                "--body",
                body,
                "--sound",
                "request",
            ],
            r.and_then(|v| v["herdr_session"].as_str()),
        )
    })();
    match send {
        Ok(_) => n["delivered"] = json!(true),
        Err(e) => n["error"] = json!(e.to_string()),
    }
    c.save(&c.path("notifications", &key("notice-"))?, &n)?;
    Ok(n)
}
pub fn memory_write(
    c: &Context,
    project: &str,
    title: &str,
    content: &str,
    kind: &str,
    run: Option<&str>,
    source: Option<&Path>,
) -> Result<String> {
    let id = key("mem-");
    let dir = c.root.join("memory").join(store::ident(project)?);
    let file = dir.join(format!("{id}.md"));
    let content = if let Some(source) = source {
        relocate(content, source, &file)?
    } else {
        content.into()
    };
    store::atomic(&file, format!("# {title}\n\n{content}\n").as_bytes())?;
    c.save(&dir.join(format!("{id}.json")),&json!({"id":id,"project":project,"title":title,"kind":kind,"run":run,"created_at":now(),"file":file.strip_prefix(&c.root)?}))?;
    Ok(file.to_string_lossy().into())
}
/// Relocate Markdown links, preserving code fences, inline code and URL suffixes.
pub fn relocate(text: &str, source: &Path, destination: &Path) -> Result<String> {
    let source = location(source)?;
    let destination = location(destination)?;
    if source.parent() == destination.parent() {
        return Ok(text.into());
    }
    let links = regex::Regex::new(
        r#"(\]\()(<[^>\n]+>|[^\s()<>]+)([\s"')])|(?m)(^ {0,3}\[[^\]\n]+\]:\s*)(<[^>\n]+>|\S+)"#,
    )?;
    let code = regex::Regex::new(r"`[^`\n]*`")?;
    let target = |v: &str| -> String {
        let angled = v.starts_with('<') && v.ends_with('>');
        let raw = if angled { &v[1..v.len() - 1] } else { v };
        let cut = raw.find(['?', '#']).unwrap_or(raw.len());
        let path = &raw[..cut];
        // A colon denotes a URI scheme only after an initial ASCII scheme name.
        // Colons later in a relative path (attachments/result:2026.txt) are filenames.
        let has_scheme = path.split_once(':').is_some_and(|(scheme, _)| {
            scheme
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
        });
        if path.is_empty() || path.starts_with('/') || has_scheme {
            return v.into();
        }
        let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
        let Ok(resolved) = location(source.parent().unwrap().join(decoded.as_ref())) else {
            return v.into();
        };
        let relative = pathdiff::diff_paths(resolved, destination.parent().unwrap()).unwrap();
        let encoded = relative
            .to_string_lossy()
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"/.-_~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>();
        let result = format!("{encoded}{}", &raw[cut..]);
        if angled {
            format!("<{result}>")
        } else {
            result
        }
    };
    let replace = |chunk: &str| {
        links
            .replace_all(chunk, |caps: &regex::Captures| {
                if let Some(prefix) = caps.get(1) {
                    format!("{}{}{}", prefix.as_str(), target(&caps[2]), &caps[3])
                } else {
                    format!("{}{}", &caps[4], target(&caps[5]))
                }
            })
            .into_owned()
    };
    let mut out = String::new();
    let mut fence: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        let trim = line.trim_start();
        let marker = if trim.starts_with("```") {
            Some("```")
        } else if trim.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        if let Some(m) = marker {
            if fence == Some(m) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(m);
            }
            out.push_str(line);
            continue;
        }
        if fence.is_some() {
            out.push_str(line);
            continue;
        }
        let mut pos = 0;
        for m in code.find_iter(line) {
            out.push_str(&replace(&line[pos..m.start()]));
            out.push_str(m.as_str());
            pos = m.end();
        }
        out.push_str(&replace(&line[pos..]));
    }
    Ok(out)
}
