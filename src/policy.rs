//! Portable model policy; host configuration only supplies a path and sparse overrides.
use crate::{
    args::s,
    config::{Context, location, merge},
    store,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

const KINDS: [&str; 4] = ["implementation", "research", "reasoning", "planning"];

fn keys(v: &Value, allowed: &[&str], label: &str) -> Result<()> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{label} должен быть объектом"))?;
    for k in obj.keys() {
        ensure!(
            allowed.contains(&k.as_str()),
            "Неизвестное поле {label}.{k}"
        );
    }
    Ok(())
}

pub fn validate_model(p: &Value, adapter: &str) -> Result<()> {
    let model = s(p, "model");
    ensure!(!model.trim().is_empty(), "Некорректный model: {model:?}");
    let allowed = if adapter == "herdr-claude" {
        &["low", "medium", "high", "xhigh", "max"][..]
    } else {
        &["low", "medium", "high", "xhigh", "max", "ultra"][..]
    };
    ensure!(
        allowed.contains(&s(p, "reasoning_effort")),
        "Неизвестный reasoning_effort: {}",
        p["reasoning_effort"]
    );
    Ok(())
}

fn validate(p: &Value, adapter: &str) -> Result<()> {
    keys(p, &["schema_version", "profiles", "rules"], "model_policy")?;
    ensure!(
        p["schema_version"] == 1,
        "model_policy требует schema_version=1"
    );
    let profiles = p["profiles"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Нет profiles в model_policy"))?;
    ensure!(!profiles.is_empty(), "Пустой список профилей");
    for (name, model) in profiles {
        store::ident(name)?;
        ensure!(name != "auto", "Имя профиля auto зарезервировано");
        keys(
            model,
            &["model", "reasoning_effort"],
            &format!("profiles.{name}"),
        )?;
        validate_model(model, adapter).map_err(|e| anyhow::anyhow!("Профиль {name}: {e}"))?;
        // Provider identifiers are opaque (e.g. Claude [1m] aliases and Bedrock ARNs).
        ensure!(
            !s(model, "model")
                .chars()
                .any(|c| c.is_whitespace() || c.is_control()),
            "Профиль {name}: model не должен содержать пробельные или управляющие символы"
        );
        ensure!(
            model["model"] != "default",
            "Именованный профиль {name} требует явный model"
        );
    }
    keys(&p["rules"], &KINDS, "rules")?;
    for kind in KINDS {
        keys(
            &p["rules"][kind],
            &["normal", "complex"],
            &format!("rules.{kind}"),
        )?;
        for complexity in ["normal", "complex"] {
            let name = s(&p["rules"][kind], complexity);
            ensure!(
                profiles.contains_key(name),
                "Правило {kind}/{complexity}: неизвестный профиль {name:?}"
            );
        }
    }
    Ok(())
}

pub fn load(settings: &mut Value, config: Option<&Path>, local: &Value) -> Result<()> {
    let selected = local.get("model_policy");
    ensure!(
        selected.is_none() || selected.is_some_and(Value::is_string),
        "model_policy должен быть строкой builtin или путём"
    );
    ensure!(
        selected.is_some() || local.get("model_overrides").is_none(),
        "model_overrides требует model_policy"
    );
    ensure!(
        selected.is_none() || local.get("profiles").is_none(),
        "Нельзя совмещать legacy profiles и model_policy; перенесите изменения в model_overrides"
    );
    let mut policy: Value = serde_json::from_str(include_str!("../config/model-policy.json"))?;
    let mut origin = "builtin".to_owned();
    if let Some(path) = selected.and_then(Value::as_str).filter(|p| *p != "builtin") {
        ensure!(!path.is_empty(), "Пустой путь model_policy");
        let path = if Path::new(path).is_relative() && !path.starts_with('~') {
            config
                .and_then(Path::parent)
                .unwrap_or(Path::new("."))
                .join(path)
        } else {
            path.into()
        };
        let path = location(path)?;
        policy = store::raw(&path)?;
        origin = path.display().to_string();
    }
    validate(&policy, s(settings, "adapter"))?;
    if let Some(over) = local.get("model_overrides") {
        keys(over, &["profiles", "rules"], "model_overrides")?;
        // Validate each container before recursive merge so scalars cannot erase sections.
        for key in ["profiles", "rules"] {
            if let Some(v) = over.get(key) {
                ensure!(v.is_object(), "model_overrides.{key} должен быть объектом");
            }
        }
        merge(&mut policy, over)?;
        validate(&policy, s(settings, "adapter"))?;
    }
    if let Some(models) = local.get("allowed_models") {
        ensure!(
            models
                .as_array()
                .is_some_and(|a| a.iter().all(|m| m.as_str().is_some_and(|s| !s.is_empty()))),
            "allowed_models должен быть массивом имён моделей"
        );
    }
    settings["resolved_model_policy"] = policy;
    settings["model_policy_origin"] = json!(origin);
    settings["model_policy_enabled"] = json!(selected.is_some());
    Ok(())
}

pub fn resolve(
    c: &Context,
    t: &Value,
    kind: &str,
    complexity: &str,
    explicit: Option<&str>,
) -> Result<Value> {
    let task = t["profile"]
        .as_str()
        .filter(|_| t["profile_source"] != "rule");
    let named = explicit.or(task);
    let enabled = c.settings["model_policy_enabled"] == true;
    let (name, source, p) = if named.is_some() || enabled {
        let policy = &c.settings["resolved_model_policy"];
        let name = named.unwrap_or_else(|| s(&policy["rules"][kind], complexity));
        let p = &policy["profiles"][name];
        ensure!(
            p.is_object(),
            "Неизвестный профиль {name:?}; проверьте model_policy"
        );
        (
            Some(name),
            if explicit.is_some() {
                "run"
            } else if task.is_some() {
                "task"
            } else {
                "rule"
            },
            p,
        )
    } else {
        (None, "legacy", &c.settings["profiles"][kind][complexity])
    };
    validate_model(p, s(&c.settings, "adapter"))?;
    if let Some(models) = c.settings["allowed_models"].as_array() {
        ensure!(
            models.contains(&p["model"]),
            "Модель {} запрещена местным allowed_models; автоматической подмены нет",
            p["model"]
        );
    }
    Ok(
        json!({"task_kind":kind,"complexity":complexity,"profile":name,"profile_source":source,
        "model":p["model"],"reasoning_effort":p["reasoning_effort"],
        "model_selection":{"source":source,"policy":if name.is_some(){c.settings["model_policy_origin"].clone()}else{json!("legacy-config")},"local_overrides": name.is_some() && c.settings.get("model_overrides").is_some()}}),
    )
}

/// A reuse request must agree with the saved launch, even if the policy has since changed.
pub fn check_reuse(requested: Option<&Value>, run: &Value) -> Result<()> {
    if let Some(p) = requested {
        ensure!(
            p["model"] == run["model"]
                && p["reasoning_effort"] == run["reasoning_effort"]
                && (run["profile"].is_null() || p["profile"] == run["profile"]),
            "Запуск {} уже использует профиль {}: {} / {}; --profile применяется только к новому запуску. Сохраните checkpoint и явно прервите прежний запуск для смены модели",
            run["id"],
            run["profile"],
            run["model"],
            run["reasoning_effort"]
        );
    }
    Ok(())
}
