use crate::{
    args::{s, strings},
    config::Context,
    core, external,
    store::{self, key, now},
};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
pub fn digest(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
/// Python json.dumps(sort_keys=True, ensure_ascii=False), including separator spaces.
pub fn canonical(value: &Value) -> Vec<u8> {
    fn write(v: &Value, out: &mut String) {
        match v {
            Value::Array(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write(v, out);
                }
                out.push(']');
            }
            Value::Object(o) => {
                out.push('{');
                let mut keys = o.keys().collect::<Vec<_>>();
                keys.sort();
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&serde_json::to_string(k).unwrap());
                    out.push_str(": ");
                    write(&o[k], out);
                }
                out.push('}');
            }
            Value::Number(n) => {
                let raw = n.to_string();
                if raw.contains(['.', 'e', 'E']) {
                    let f = raw.parse::<f64>().unwrap();
                    out.push_str(&python_float(f));
                } else {
                    // Python decodes the integer token -0 as 0; float -0.0 keeps its sign.
                    out.push_str(if raw == "-0" { "0" } else { &raw });
                }
            }
            _ => out.push_str(&v.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out.into_bytes()
}
/// Python repr: shortest nearest decimal, ties to even; fixed for -4 <= exp < 16.
fn python_float(value: f64) -> String {
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        }
        .into();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .into();
    }
    let mut buffer = ryu::Buffer::new();
    let repr = buffer.format_finite(value);
    let unsigned = repr.strip_prefix('-').unwrap_or(repr);
    let (mantissa, exponent) = unsigned
        .split_once('e')
        .map(|(m, e)| (m, e.parse::<i32>().unwrap()))
        .unwrap_or((unsigned, 0));
    let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len() as i32);
    let mut digits = mantissa.replace('.', "").trim_start_matches('0').to_owned();
    let mut scale = exponent - fraction;
    while digits.ends_with('0') {
        digits.pop();
        scale += 1;
    }
    let exponent = digits.len() as i32 + scale - 1;
    let mut result = if (-4..16).contains(&exponent) {
        let point = digits.len() as i32 + scale;
        if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point >= digits.len() as i32 {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            let point = point as usize;
            format!("{}.{}", &digits[..point], &digits[point..])
        }
    } else {
        let mantissa = if digits.len() == 1 {
            digits
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!("{mantissa}e{exponent:+03}")
    };
    if value.is_sign_negative() {
        result.insert(0, '-');
    }
    result
}
fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.into()
    } else {
        std::env::current_dir()?.join(path)
    })
}
fn capture(file: &Path, files: &mut Value, destination: Option<&Path>) -> Result<()> {
    let file = absolute(file)?;
    let name = file.to_string_lossy().to_string();
    if files.get(&name).is_some() {
        return Ok(());
    }
    let meta = fs::symlink_metadata(&file)?;
    let (kind, data) = if meta.file_type().is_symlink() {
        (
            "symlink",
            fs::read_link(&file)?
                .as_os_str()
                .as_encoded_bytes()
                .to_vec(),
        )
    } else {
        ensure!(meta.is_file(), "Неподдерживаемый артефакт: {name}");
        ("file", fs::read(&file)?)
    };
    let key = digest(name.as_bytes());
    files[&name] = json!({"sha256":digest(&data),"mode":meta.permissions().mode()&0o7777,"kind":kind,"blob":format!("blobs/{key}")});
    if let Some(dir) = destination {
        store::atomic(&dir.join("blobs").join(key), &data)?;
    }
    Ok(())
}
fn artifact(file: &Path, files: &mut Value, dest: Option<&Path>) -> Result<()> {
    let meta = fs::symlink_metadata(file)?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        let mut entries = fs::read_dir(file)?
            .map(|e| Ok(e?.path()))
            .collect::<Result<Vec<_>>>()?;
        entries.sort();
        for p in entries {
            artifact(&p, files, dest)?;
        }
    } else {
        capture(file, files, dest)?;
    }
    Ok(())
}
pub fn snapshot(c: &Context, t: &Value, r: &Value, destination: Option<&Path>) -> Result<Value> {
    let mut files = json!({});
    let dir = c.run_file(s(r, "id"))?.parent().unwrap().to_path_buf();
    for name in ["assignment.md", "result.md"] {
        capture(&dir.join(name), &mut files, destination)?;
    }
    let project = c.read(&c.path("projects", s(t, "project"))?)?;
    for value in strings(&r["review_artifacts"]) {
        artifact(Path::new(&value), &mut files, destination)?;
    }
    ensure!(
        t["code"] != true || r["worktrees"].as_array().is_some_and(|a| !a.is_empty()),
        "Для кодового результата отсутствуют worktree"
    );
    let mut repos = vec![];
    for item in r["worktrees"].as_array().into_iter().flatten() {
        let wt = Path::new(s(item, "path"));
        let git = |args: &[&str]| -> Result<String> {
            let mut argv = vec!["git", "-C", s(item, "path")];
            argv.extend_from_slice(args);
            external::command(&argv)
        };
        let head = git(&["rev-parse", "HEAD"])?.trim().to_string();
        let base = git(&[
            "merge-base",
            &head,
            item["main_branch"].as_str().unwrap_or("main"),
        ])?
        .trim()
        .to_string();
        let names = git(&[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])?;
        let mut names = names
            .split('\0')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        for name in names {
            let file = wt.join(name);
            if fs::symlink_metadata(&file).is_ok() {
                capture(&file, &mut files, destination)?;
            }
        }
        for parent in wt.ancestors() {
            if parent.join("AGENTS.md").is_file() {
                capture(&parent.join("AGENTS.md"), &mut files, destination)?;
            }
        }
        repos.push(json!({"path":wt,"repository":item["repository"],"head":head,"base":base,"status":git(&["status","--porcelain=v1","--untracked-files=all"])? ,"diff":git(&["diff","--binary",&base,"--"])?}));
    }
    for name in ["worker.md", "agent-review.md"] {
        let file = c.program.join("docs").join(name);
        if file.exists() {
            capture(&file, &mut files, destination)?;
        }
    }
    if let Some(folder) = r["cwd"].as_str().or(project["cwd"].as_str()) {
        for parent in Path::new(folder).ancestors() {
            for name in ["AGENTS.md", "CLAUDE.md"] {
                let file = parent.join(name);
                if file.is_file() {
                    capture(&file, &mut files, destination)?;
                }
            }
        }
    }
    let mut task = json!({});
    for k in ["id", "title", "goal", "acceptance", "code", "project"] {
        task[k] = t[k].clone();
    }
    let mut manifest = json!({"submission":r["result_revision"].as_u64().unwrap_or(0),"task":task,"files":files,"repositories":repos,"boundaries":"Git: tracked и untracked без ignored; внешнее состояние проверяется на момент наблюдения. No-code: только result.md и явно переданные --artifact; секреты не включать."});
    manifest["project"] = project;
    let mut frozen = json!({"version":digest(&canonical(&manifest))});
    frozen["manifest"] = manifest;
    Ok(frozen)
}
fn verify(c: &Context, a: &Value) -> Result<Value> {
    let file = Path::new(s(a, "snapshot"));
    let frozen = c.read(file)?;
    ensure!(
        frozen["version"] == a["version"]
            && digest(&canonical(&frozen["manifest"])) == s(a, "version"),
        "Повреждён манифест снимка"
    );
    let parent = fs::canonicalize(file.parent().unwrap())?;
    for item in frozen["manifest"]["files"]
        .as_object()
        .ok_or_else(|| anyhow!("Нет files"))?
        .values()
    {
        let blob = parent.join(s(item, "blob"));
        ensure!(
            fs::canonicalize(&blob)?.starts_with(&parent)
                && digest(&fs::read(&blob)?) == s(item, "sha256"),
            "Повреждён blob снимка"
        );
    }
    Ok(frozen)
}
fn review_file(c: &Context, r: &Value) -> Result<PathBuf> {
    Ok(c.run_file(s(r, "id"))?.with_file_name("agent-review.json"))
}
fn load(c: &Context, r: &Value) -> Result<Value> {
    let file = review_file(c, r)?;
    if !file.exists() {
        return Ok(json!({"attempts":[]}));
    }
    let mut data = c.read(&file)?;
    if let Some(a) = data["attempts"].as_array_mut().and_then(|a| a.last_mut())
        && a["status"] == "running"
        && chrono::DateTime::parse_from_rfc3339(s(a, "deadline"))?.with_timezone(&chrono::Utc)
            <= chrono::Utc::now()
    {
        a["status"] = json!("failed");
        a["conclusion"] = json!("Истёк срок subagent; требуется новый запрос");
        a["finished_at"] = json!(now());
        c.save(&file, &data)?;
    }
    Ok(data)
}
pub fn request(c: &Context, t: &Value, r: &Value) -> Result<Value> {
    ensure!(
        t["agent_review_policy"] != "off",
        "Агентское ревью отключено для задачи"
    );
    ensure!(
        t["status"] == "review" && r["status"] == "review",
        "Ревью доступно после finish"
    );
    let mut data = load(c, r)?;
    let fresh = snapshot(c, t, r, None)?;
    if let Some(last) = data["attempts"].as_array_mut().unwrap().last_mut() {
        if last["status"] == "running" {
            return Ok(last.clone());
        }
        if last["status"] == "pending" {
            if last["version"] == fresh["version"] {
                return Ok(last.clone());
            }
            last["status"] = json!("failed");
            last["conclusion"] = json!("Снимок устарел до запуска");
            last["finished_at"] = json!(now());
        }
        if last["version"] == fresh["version"]
            && ["passed", "changes_requested"].contains(&s(last, "status"))
        {
            return Ok(last.clone());
        }
    }
    let id = key("review-");
    let dest = c
        .run_file(s(r, "id"))?
        .parent()
        .unwrap()
        .join("reviews")
        .join(&id);
    let frozen = snapshot(c, t, r, Some(&dest))?;
    ensure!(
        frozen == fresh,
        "Результат изменился при создании снимка; повторите запрос"
    );
    c.save(&dest.join("snapshot.json"), &frozen)?;
    let packet = dest.join("packet.md");
    store::atomic(&packet,format!("# Независимое ревью\n\nКоманда входа: `{} review-read --run {} --id {id}`\n\nПроверь task.goal и task.acceptance самостоятельно. Используй --section files/diff/blob/manifest для фиксированных входных данных. Отчёт исполнителя содержит утверждения, которые нужно проверить. Проверяющий не исправляет материалы. Выполняй проверки в отдельном временном каталоге. Недоступность необходимых доказательств означает failed.\n\nВерни JSON: version из entry, status (passed|changes_requested|failed), conclusion, findings, suggestions, evidence (непустой список), limitations (списки строк). passed допускается без обязательных замечаний и не означает complete, merge или push.\n",crate::lifecycle::entry_command(c),s(r,"id")).as_bytes())?;
    let a = json!({"id":id,"version":frozen["version"],"status":"pending","created_at":now(),"packet":packet,"snapshot":dest.join("snapshot.json"),"submission":r["result_revision"].as_u64().unwrap_or(0)});
    data["attempts"].as_array_mut().unwrap().push(a.clone());
    c.save(&review_file(c, r)?, &data)?;
    Ok(a)
}
fn read_section(c: &Context, a: &Value, args: &Value) -> Result<Value> {
    let frozen = verify(c, a)?;
    let m = &frozen["manifest"];
    match s(args, "section") {
        "manifest" => Ok(frozen),
        "files" => Ok(m["files"].clone()),
        "diff" => {
            let index = args["repository"].as_u64().unwrap_or(u64::MAX) as usize;
            Ok(m["repositories"]
                .as_array()
                .and_then(|v| v.get(index))
                .ok_or_else(|| anyhow!("Нет такого репозитория в снимке"))?
                .clone())
        }
        "blob" => {
            let path = s(args, "path");
            let item = m["files"].get(path).ok_or_else(|| {
                anyhow!("Укажите --path из раздела files (исходный абсолютный путь)")
            })?;
            let stored = Path::new(s(a, "snapshot"))
                .parent()
                .unwrap()
                .join(s(item, "blob"));
            let content = String::from_utf8(fs::read(&stored)?).ok();
            let mut out = item.clone();
            core::extend(
                &mut out,
                json!({"path":path,"content":content,"stored_path":stored}),
            );
            Ok(out)
        }
        _ => {
            let important = m["files"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(name, _)| {
                    Path::new(name)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| {
                            [
                                "assignment.md",
                                "result.md",
                                "AGENTS.md",
                                "CLAUDE.md",
                                "worker.md",
                                "agent-review.md",
                            ]
                            .contains(&n)
                        })
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<serde_json::Map<_, _>>();
            let repos = m["repositories"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    r.as_object_mut().unwrap().remove("diff");
                    r
                })
                .collect::<Vec<_>>();
            let mut entry = json!({"version":frozen["version"],"boundaries":m["boundaries"],"submission":m["submission"],"file_count":m["files"].as_object().unwrap().len(),"sections":["files","diff","blob","manifest"],"snapshot":a["snapshot"]});
            entry["task"] = m["task"].clone();
            entry["files"] = Value::Object(important);
            entry["repositories"] = Value::Array(repos);
            Ok(entry)
        }
    }
}
pub fn handle(c: &Context, name: &str, args: &Value) -> Result<Value> {
    let mut r = c.read(&c.run_file(s(args, "run"))?)?;
    let mut t = c.task(s(&r, "task"))?;
    c.current(&t, s(args, "run"))?;
    if name == "review-request" {
        if !args["artifact"].is_null() {
            ensure!(
                t["status"] == "review" && t["agent_review_policy"] != "off",
                "Добавление артефактов доступно для разрешённого ревью"
            );
            let mut artifacts = strings(&r["review_artifacts"]);
            for p in strings(&args["artifact"]) {
                let p = crate::config::absolute(p)?.to_string_lossy().to_string();
                if !artifacts.contains(&p) {
                    artifacts.push(p);
                }
            }
            r["review_artifacts"] = json!(artifacts);
            c.update(&mut r, &mut t)?;
        }
        return request(c, &t, &r);
    }
    let mut data = load(c, &r)?;
    if name == "review-show" {
        let mut view = data.clone();
        if args["latest"] == true {
            view["attempts"] = Value::Array(
                data["attempts"]
                    .as_array()
                    .unwrap()
                    .last()
                    .cloned()
                    .into_iter()
                    .collect(),
            );
        }
        let version = snapshot(c, &t, &r, None).ok().map(|v| v["version"].clone());
        for a in view["attempts"].as_array_mut().unwrap() {
            a["stale"] = json!(version.as_ref() != Some(&a["version"]));
        }
        return Ok(view);
    }
    let attempts = data["attempts"].as_array().unwrap();
    ensure!(!attempts.is_empty(), "Нет запроса ревью");
    let mut index = attempts.len() - 1;
    if name == "review-read" {
        if let Some(id) = args["id"].as_str() {
            index = attempts
                .iter()
                .position(|a| a["id"] == id)
                .ok_or_else(|| anyhow!("Нет такой попытки ревью"))?;
        }
        return read_section(c, &attempts[index], args);
    }
    if name == "review-feedback" && args["delivered"] == true {
        index = attempts
            .iter()
            .position(|a| a["id"] == args["id"])
            .unwrap_or(index);
    }
    let mut a = attempts[index].clone();
    ensure!(a["id"] == args["id"], "Ответ относится к другой попытке");
    if ["review-claim", "review-conclude"].contains(&name) {
        ensure!(
            t["status"] == "review" && r["status"] == "review",
            "Результат уже не ожидает ревью"
        );
    }
    match name {
        "review-claim" => {
            if a["status"] == "running" && a["actor"] == args["actor"] {
                return Ok(a);
            }
            ensure!(a["status"] == "pending", "Попытка уже занята или завершена");
            if snapshot(c, &t, &r, None)?["version"] != a["version"] {
                a["status"] = json!("failed");
                a["conclusion"] = json!("Снимок устарел до запуска");
                data["attempts"][index] = a;
                c.save(&review_file(c, &r)?, &data)?;
                bail!("Снимок устарел до запуска");
            }
            if let Err(e) = verify(c, &a) {
                a["status"] = json!("failed");
                a["conclusion"] = json!(e.to_string());
                a["finished_at"] = json!(now());
                data["attempts"][index] = a;
                c.save(&review_file(c, &r)?, &data)?;
                bail!("Снимок повреждён: {e}");
            }
            a["status"] = json!("running");
            a["actor"] = args["actor"].clone();
            a["started_at"] = json!(now());
            a["deadline"] = json!(
                (chrono::Utc::now()
                    + chrono::Duration::seconds(args["lease"].as_i64().unwrap_or(900)))
                .to_rfc3339()
            );
        }
        "review-conclude" => {
            ensure!(
                a["status"] == "running" && a["actor"] == args["actor"],
                "Нет активной аренды этого subagent"
            );
            if let Err(e) = verify(c, &a) {
                a["status"] = json!("failed");
                a["conclusion"] = json!(e.to_string());
                a["finished_at"] = json!(now());
                data["attempts"][index] = a;
                c.save(&review_file(c, &r)?, &data)?;
                bail!("Снимок повреждён: {e}");
            }
            let result = store::raw(Path::new(s(args, "result")))?;
            ensure!(
                result["version"] == a["version"],
                "Ответ не привязан к версии снимка"
            );
            ensure!(
                ["passed", "changes_requested", "failed"].contains(&s(&result, "status")),
                "Некорректный статус"
            );
            for k in ["findings", "suggestions", "evidence", "limitations"] {
                ensure!(
                    result[k]
                        .as_array()
                        .is_some_and(|v| v.iter().all(Value::is_string)),
                    "Ожидается список строк: {k}"
                );
            }
            ensure!(
                !s(&result, "conclusion").trim().is_empty()
                    && !result["evidence"].as_array().unwrap().is_empty(),
                "Нужны заключение и доказательства"
            );
            ensure!(
                result["status"] != "passed" || result["findings"].as_array().unwrap().is_empty(),
                "passed несовместим с обязательными замечаниями"
            );
            ensure!(
                result["status"] != "changes_requested"
                    || !result["findings"].as_array().unwrap().is_empty(),
                "Нужны конкретные замечания"
            );
            let stale = snapshot(c, &t, &r, None)
                .ok()
                .is_none_or(|v| v["version"] != a["version"]);
            a["status"] = if stale {
                json!("failed")
            } else {
                result["status"].clone()
            };
            a["result"] = result;
            a["finished_at"] = json!(now());
            if stale {
                a["conclusion"] = json!("Результат изменён во время ревью; заключение устарело");
            }
        }
        "review-feedback" => return feedback(c, &mut t, &mut r, data, index, a, args),
        _ => bail!("Неизвестная команда ревью"),
    }
    data["attempts"][index] = a.clone();
    c.save(&review_file(c, &r)?, &data)?;
    Ok(a)
}
fn feedback(
    c: &Context,
    t: &mut Value,
    r: &mut Value,
    mut data: Value,
    index: usize,
    mut a: Value,
    args: &Value,
) -> Result<Value> {
    if !a["feedback_delivered"].is_null() {
        return Ok(a);
    }
    let legacy = !s(&a, "feedback_message").is_empty()
        && a["feedback_started"].is_null()
        && a["status"] == "changes_requested";
    ensure!(
        args["delivered"] != true || !a["feedback_started"].is_null() || legacy,
        "Сначала подготовьте возврат через review-feedback"
    );
    if a["feedback_started"].is_null() {
        let findings = strings(&a["result"]["findings"]);
        ensure!(
            !findings.is_empty()
                && (a["status"] == "changes_requested"
                    || (args["user_requested"] == true && a["status"] == "failed")),
            "Нужны замечания; failed возобновляется только по поручению пользователя"
        );
        if args["user_requested"] != true && !legacy {
            ensure!(
                snapshot(c, t, r, None)?["version"] == a["version"],
                "Нужны актуальные замечания"
            );
        }
        let count = data["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| {
                (!a["feedback_started"].is_null() || !a["feedback_delivered"].is_null())
                    && a["feedback_user_requested"] != true
            })
            .count();
        ensure!(
            count < 2 || args["user_requested"] == true,
            "Лимит двух автоматических циклов; нужен пользователь"
        );
        ensure!(
            t["status"] == "review" && r["status"] == "review",
            "Задача уже не в review"
        );
        a["feedback_started"] = json!(now());
        a["feedback_user_requested"] = args["user_requested"].clone();
        if s(&a, "feedback_message").is_empty() {
            a["feedback_message"] = json!(format!(
                "{}: Исправь только замечания в исходном объёме assignment.md. Спорные требования передай пользователю. Затем finish в том же запуске.\n{}",
                s(t, "id"),
                findings.join("\n")
            ));
        }
        a["delivery_required"] = json!(
            "Отправь feedback_message исходному живому исполнителю, затем подтверди --delivered. При сбое доставки повтори её или обратись к пользователю; второй исполнитель не создаётся."
        );
        if legacy {
            a["feedback_migrated"] = json!(true);
            a["submission"] = json!(r["result_revision"].as_u64().unwrap_or(0));
        }
        data["attempts"][index] = a.clone();
        c.save(&review_file(c, r)?, &data)?;
    }
    if r["result_revision"].as_u64().unwrap_or(0) == a["submission"].as_u64().unwrap_or(0) {
        ensure!(
            ["review", "running"].contains(&s(t, "status"))
                && ["review", "running"].contains(&s(r, "status")),
            "Нельзя возобновить этот запуск"
        );
        if r["feedback_attempt"] != a["id"] || r["status"] != "running" || t["status"] != "running"
        {
            r["feedback_attempt"] = a["id"].clone();
            r["status"] = json!("running");
            t["status"] = json!("running");
            c.update(r, t)?;
        }
    }
    if args["delivered"] == true {
        a["feedback_delivered"] = json!(now());
        data["attempts"][index] = a.clone();
        c.save(&review_file(c, r)?, &data)?;
    }
    Ok(a)
}
