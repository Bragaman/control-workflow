use crate::{args::s, config::Context, core, lifecycle, store};
use anyhow::{Context as _, Result, bail, ensure};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Value, json};
use std::path::PathBuf;

fn path(c: &Context, id: &str) -> Result<PathBuf> {
    Ok(c.root
        .join("schedules")
        .join(format!("{}.json", store::ident(id)?)))
}
fn parse_days(input: &str) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for day in input.split(',') {
        let n = match day {
            "mon" => 0,
            "tue" => 1,
            "wed" => 2,
            "thu" => 3,
            "fri" => 4,
            "sat" => 5,
            "sun" => 6,
            _ => bail!("Дни: mon,tue,wed,thu,fri,sat,sun"),
        };
        ensure!(!out.contains(&n), "Повторный день: {day}");
        out.push(n);
    }
    Ok(out)
}
fn spec(v: &Value) -> Result<(Tz, NaiveTime, Vec<u32>)> {
    let tz: Tz = s(v, "timezone")
        .parse()
        .map_err(|_| anyhow::anyhow!("Неизвестный IANA timezone"))?;
    let time = NaiveTime::parse_from_str(s(v, "time"), "%H:%M")?;
    ensure!(
        time.format("%H:%M").to_string() == s(v, "time"),
        "Время должно быть HH:MM"
    );
    Ok((tz, time, parse_days(s(v, "days"))?))
}
fn instant(tz: Tz, time: NaiveTime, date: NaiveDate) -> Option<DateTime<Utc>> {
    match tz.from_local_datetime(&date.and_time(time)) {
        LocalResult::Single(v) => Some(v.with_timezone(&Utc)),
        LocalResult::Ambiguous(a, b) => Some(a.min(b).with_timezone(&Utc)),
        LocalResult::None => None,
    }
}
fn next(v: &Value, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let (tz, time, days) = spec(v)?;
    let base = after.with_timezone(&tz).date_naive();
    for offset in 0..=14 {
        let date = base
            .checked_add_signed(Duration::days(offset))
            .context("Дата вне диапазона")?;
        if days.contains(&date.weekday().num_days_from_monday())
            && let Some(at) = instant(tz, time, date)
            && at > after
        {
            return Ok(at);
        }
    }
    bail!("Не удалось рассчитать следующее срабатывание")
}
fn latest_due(
    v: &Value,
    cursor: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    let mut after = cursor.max(now - Duration::hours(24));
    let mut latest = None;
    loop {
        let at = next(v, after)?;
        if at > now {
            return Ok(latest);
        }
        latest = Some(at);
        after = at;
    }
}
fn time(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc))
}
fn display(mut v: Value, now: DateTime<Utc>) -> Result<Value> {
    v["next_at"] = if v["status"] == "active" {
        json!(next(&v, now)?.to_rfc3339())
    } else {
        Value::Null
    };
    v["scheduler"] = json!({"mode":"external_tick", "last_tick":v["last_tick"], "note":"Встроенного scheduler нет; внешний вызов schedule-tick не проверяется"});
    Ok(v)
}
fn write(c: &Context, v: &Value) -> Result<()> {
    c.save(&path(c, s(v, "id"))?, v)
}
fn records(c: &Context) -> Result<Vec<Value>> {
    store::json_files(&c.root.join("schedules"))?
        .iter()
        .map(|p| c.read(p))
        .collect()
}

pub fn handle(c: &Context, name: &str, a: &Value) -> Result<Value> {
    let _lock = c.lock()?;
    let now = Utc::now();
    match name {
        "schedule-create" => {
            let id = s(a, "id");
            let p = path(c, id)?;
            ensure!(!p.exists(), "Расписание уже существует");
            let template = c.task(s(a, "template"))?;
            ensure!(
                template["depends"].as_array().is_none_or(|x| x.is_empty()),
                "Шаблон с зависимостями не поддерживается"
            );
            c.require_herdr()?;
            let namespace = c.namespace(None)?;
            let project = c.read(&c.path("projects", s(&template, "project"))?)?;
            let mut snapshot = json!({});
            for k in [
                "project",
                "title",
                "goal",
                "acceptance",
                "code",
                "repositories",
                "git_mode",
                "agent_review_policy",
                "task_kind",
                "complexity",
                "profile",
                "profile_source",
                "model",
                "reasoning_effort",
                "model_selection",
            ] {
                snapshot[k] = template[k].clone();
            }
            if snapshot["repositories"].is_null() {
                snapshot["repositories"] = project["repositories"].clone();
            }
            let v = json!({"id":id,"template_id":s(a,"template"),"template":snapshot,"days":a["days"],"time":a["time"],"timezone":a["timezone"],"namespace":namespace,"status":"active","created_at":now.to_rfc3339(),"cursor":now.to_rfc3339(),"last_tick":null,"history":[]});
            spec(&v)?;
            next(&v, now)?;
            write(c, &v)?;
            display(v, now)
        }
        "schedule-list" => Ok(json!(
            records(c)?
                .into_iter()
                .map(|v| display(v, now))
                .collect::<Result<Vec<_>>>()?
        )),
        "schedule-show" => display(c.read(&path(c, s(a, "id"))?)?, now),
        "schedule-pause" | "schedule-resume" | "schedule-cancel" => {
            let mut v = c.read(&path(c, s(a, "id"))?)?;
            match name {
                "schedule-pause" => {
                    ensure!(v["status"] == "active", "Расписание не активно");
                    v["status"] = json!("paused");
                }
                "schedule-resume" => {
                    ensure!(v["status"] == "paused", "Расписание не приостановлено");
                    v["status"] = json!("active");
                    v["cursor"] = json!(time(s(&v, "cursor"))?.max(now).to_rfc3339());
                }
                _ => {
                    ensure!(v["status"] != "cancelled", "Расписание уже отменено");
                    v["status"] = json!("cancelled");
                }
            }
            write(c, &v)?;
            display(v, now)
        }
        "schedule-tick" => tick(c, now),
        _ => bail!("Неизвестная команда расписания"),
    }
}
fn tick(c: &Context, now: DateTime<Utc>) -> Result<Value> {
    c.require_herdr()?;
    let namespace = c.namespace(None)?;
    let mut output = Vec::new();
    for mut v in records(c)? {
        if v["status"] != "active" {
            continue;
        }
        if v["namespace"] != namespace {
            output.push(json!({"schedule":v["id"],"error":"Другой namespace herdr"}));
            continue;
        }
        // An interrupted start may have spawned a launcher. Never invoke it twice.
        for occurrence in v["history"].as_array_mut().unwrap() {
            if occurrence["state"] == "starting" {
                let task = occurrence["task"].as_str().and_then(|id| c.task(id).ok());
                if let Some(run) = task.as_ref().and_then(|t| t["active_run"].as_str()) {
                    occurrence["run"] = json!(run);
                    occurrence["state"] = json!("started");
                } else {
                    occurrence["state"] = json!("uncertain");
                    occurrence["error"] = json!(
                        "Запуск прерван после записи намерения; проверьте задачу и состояние запуска вручную"
                    );
                }
            }
        }
        write(c, &v)?;
        // A claimed occurrence can be resumed before the launch intent was recorded.
        let pending = v["history"]
            .as_array()
            .unwrap()
            .iter()
            .position(|e| e["state"] == "claimed");
        let index = if let Some(i) = pending {
            Some(i)
        } else {
            let cursor = time(s(&v, "cursor"))?;
            let latest = latest_due(&v, cursor, now)?;
            v["cursor"] = json!(cursor.max(now).to_rfc3339());
            v["last_tick"] = json!(now.to_rfc3339());
            if let Some(at) = latest {
                let previous_busy = v["history"].as_array().unwrap().iter().any(|e| {
                    if ["claimed", "starting"].contains(&s(e, "state")) {
                        return true;
                    }
                    match e["task"].as_str() {
                        Some(id) => c
                            .task(id)
                            .map_or(true, |t| !["done", "cancelled"].contains(&s(&t, "status"))),
                        None => false,
                    }
                });
                let id = store::key("rec-");
                v["history"].as_array_mut().unwrap().push(json!({"scheduled_at":at.to_rfc3339(),"task":if previous_busy {Value::Null} else {json!(id)},"run":null,"state":if previous_busy {"skipped_overlap"} else {"claimed"}}));
                write(c, &v)?;
                if previous_busy {
                    output.push(json!({"schedule":v["id"],"state":"skipped_overlap"}));
                    None
                } else {
                    Some(v["history"].as_array().unwrap().len() - 1)
                }
            } else {
                write(c, &v)?;
                None
            }
        };
        if let Some(i) = index {
            let id = s(&v["history"][i], "task").to_owned();
            let p = c.path("tasks", &id)?;
            if !p.exists() {
                let mut t = v["template"].clone();
                core::extend(
                    &mut t,
                    json!({"id":id,"status":"backlog","depends":[],"reviewer":"user","active_run":null,"runs":[],"created_at":store::now(),"updated_at":store::now(),"schedule":v["id"],"scheduled_at":v["history"][i]["scheduled_at"]}),
                );
                c.save(&p, &t)?;
            }
            let occurrence_task = c.task(&id)?;
            ensure!(
                occurrence_task["schedule"] == v["id"]
                    && occurrence_task["scheduled_at"] == v["history"][i]["scheduled_at"],
                "Конфликт ID задачи срабатывания: {id}"
            );
            v["history"][i]["state"] = json!("starting");
            write(c, &v)?;
            let result = lifecycle::start(c, &id, false, false, None);
            let task = c.task(&id)?;
            v["history"][i]["run"] = task["active_run"].clone();
            match result {
                Ok(_) => v["history"][i]["state"] = json!("started"),
                Err(e) => {
                    v["history"][i]["state"] = json!("failed");
                    v["history"][i]["error"] = json!(e.to_string());
                }
            }
            write(c, &v)?;
            output.push(json!({"schedule":v["id"],"occurrence":v["history"][i]}));
        }
    }
    Ok(json!({"tick_at":now.to_rfc3339(),"items":output}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn schedule(days: &str, hour: &str, zone: &str) -> Value {
        json!({"days":days,"time":hour,"timezone":zone})
    }
    #[test]
    fn weekdays_and_timezone() {
        let v = schedule("mon,tue,wed,thu,fri", "09:00", "Europe/Moscow");
        assert_eq!(
            next(&v, time("2026-11-06T06:00:00Z").unwrap())
                .unwrap()
                .to_rfc3339(),
            "2026-11-09T06:00:00+00:00"
        );
        assert!(spec(&schedule("mon,mon", "09:00", "UTC")).is_err());
        assert!(spec(&schedule("mon", "9:00", "Europe/Moscow")).is_err());
        assert!(spec(&schedule("mon", "09:00", "Fake/Zone")).is_err());
    }
    #[test]
    fn downtime_and_clock_rollback_do_not_backfill() {
        let v = schedule("mon,tue,wed,thu,fri,sat,sun", "09:00", "UTC");
        let now = time("2026-11-11T10:00:00Z").unwrap();
        assert_eq!(
            latest_due(&v, time("2026-11-01T00:00:00Z").unwrap(), now).unwrap(),
            Some(time("2026-11-11T09:00:00Z").unwrap())
        );
        assert_eq!(
            latest_due(&v, time("2026-11-11T09:00:00Z").unwrap(), now).unwrap(),
            None
        );
        assert_eq!(
            latest_due(&v, time("2026-11-12T09:00:00Z").unwrap(), now).unwrap(),
            None
        );
    }
    #[test]
    fn dst_gap_skipped_and_fold_uses_first_instant() {
        let gap = schedule("sun", "02:30", "Europe/Berlin");
        assert_eq!(
            next(&gap, time("2026-03-28T00:00:00Z").unwrap())
                .unwrap()
                .to_rfc3339(),
            "2026-04-05T00:30:00+00:00"
        );
        assert_eq!(
            next(&gap, time("2026-10-24T00:00:00Z").unwrap())
                .unwrap()
                .to_rfc3339(),
            "2026-10-25T00:30:00+00:00"
        );
        assert_eq!(
            next(&gap, time("2026-10-25T00:30:00Z").unwrap())
                .unwrap()
                .to_rfc3339(),
            "2026-11-01T01:30:00+00:00"
        );
    }
}
