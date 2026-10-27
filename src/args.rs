use anyhow::{Result, anyhow, ensure};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Value, json};

pub fn registry() -> Vec<Value> {
    serde_json::from_str(include_str!("commands.json")).unwrap()
}
pub fn cli() -> Command {
    let mut cmd = Command::new("control-workflow")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Контрольная панель: native CLI / MCP stdio")
        .subcommand_required(true);
    for key in [
        "state-root",
        "bindings-root",
        "config",
        "herdr-session",
        "program-root",
    ] {
        cmd = cmd.arg(Arg::new(key).long(key).global(true));
    }
    for spec in registry() {
        let mut sub = Command::new(spec["name"].as_str().unwrap().to_owned());
        for option in spec["options"].as_array().unwrap() {
            let key = option["name"].as_str().unwrap();
            let flags = option["flags"].as_array().unwrap();
            let mut arg = Arg::new(key.to_owned());
            if option["integer"] == true {
                arg = arg.allow_hyphen_values(true);
            }
            if let Some(flag) = flags.first() {
                arg = arg.long(flag.as_str().unwrap().trim_start_matches('-').to_owned());
            }
            if option["boolean"] == true {
                arg = arg.action(ArgAction::SetTrue);
                if flags.len() > 1 {
                    let no = format!("no-{}", key.replace('_', "-"));
                    sub = sub.arg(
                        Arg::new(no.clone())
                            .long(no.clone())
                            .action(ArgAction::SetTrue)
                            .conflicts_with(key.to_owned()),
                    );
                    arg = arg.conflicts_with(no);
                }
            } else {
                if option["multiple"] == true {
                    arg = arg.action(ArgAction::Append);
                    if flags.is_empty() {
                        arg = arg.num_args(1..);
                    }
                }
                if let Some(choices) = option["choices"].as_array() {
                    arg = arg.value_parser(clap::builder::PossibleValuesParser::new(
                        choices
                            .iter()
                            .map(|v| v.as_str().unwrap().to_owned())
                            .collect::<Vec<_>>(),
                    ));
                }
                arg = arg.required(option["required"] == true);
            }
            sub = sub.arg(arg);
        }
        cmd = cmd.subcommand(sub);
    }
    cmd
}
pub fn values(matches: &ArgMatches) -> Result<(String, Value)> {
    let (name, m) = matches
        .subcommand()
        .ok_or_else(|| anyhow!("Нужна команда"))?;
    let spec = registry().into_iter().find(|s| s["name"] == name).unwrap();
    let mut obj = json!({});
    for o in spec["options"].as_array().unwrap() {
        let k = o["name"].as_str().unwrap();
        let v = if o["boolean"] == true {
            let no = format!("no-{}", k.replace('_', "-"));
            if m.get_flag(k) {
                json!(true)
            } else if o["flags"].as_array().unwrap().len() > 1 && m.get_flag(&no) {
                json!(false)
            } else {
                o["default"].clone()
            }
        } else if o["multiple"] == true {
            m.get_many::<String>(k)
                .map(|vs| json!(vs.collect::<Vec<_>>()))
                .unwrap_or(o["default"].clone())
        } else if let Some(s) = m.get_one::<String>(k) {
            if o["integer"] == true {
                json!(s.parse::<i64>()?)
            } else {
                json!(s)
            }
        } else {
            o["default"].clone()
        };
        obj[k] = v;
    }
    validate(name, &obj)?;
    Ok((name.into(), obj))
}
pub fn validate(name: &str, a: &Value) -> Result<()> {
    ensure!(a.is_object(), "Аргументы должны быть объектом");
    let spec = registry()
        .into_iter()
        .find(|s| s["name"] == name)
        .ok_or_else(|| anyhow!("Неизвестная команда"))?;
    for key in a.as_object().unwrap().keys() {
        ensure!(
            spec["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["name"] == *key),
            "Неизвестный аргумент: {key}"
        );
    }
    for o in spec["options"].as_array().unwrap() {
        let key = o["name"].as_str().unwrap();
        let value = &a[key];
        if value.is_null() {
            ensure!(o["required"] != true, "Требуется {key}");
            continue;
        }
        let valid = if o["boolean"] == true {
            value.is_boolean()
        } else if o["multiple"] == true {
            value
                .as_array()
                .is_some_and(|v| v.iter().all(Value::is_string))
        } else if o["integer"] == true {
            value.is_i64()
        } else {
            value.is_string()
        };
        ensure!(valid, "Некорректный тип {key}");
        if let Some(choices) = o["choices"].as_array() {
            ensure!(choices.contains(value), "Недопустимое значение {key}");
        }
    }
    if name == "memory-add" {
        ensure!(
            a["text"].is_string() ^ a["file"].is_string(),
            "Нужно одно из text/file"
        );
    }
    if name == "memory-list" {
        if let Some(n) = a["limit"].as_i64() {
            ensure!(n > 0, "limit должен быть положительным");
        }
        if let Some(n) = a["offset"].as_i64() {
            ensure!(n >= 0, "offset должен быть неотрицательным");
            ensure!(n == 0 || !a["limit"].is_null(), "--offset требует --limit");
        }
    }
    if name == "review-claim" {
        let n = a["lease"].as_i64().unwrap_or(900);
        ensure!((1..=3600).contains(&n), "Аренда: 1–3600 секунд");
    }
    Ok(())
}
pub fn defaults(name: &str, mut a: Value) -> Result<Value> {
    let spec = registry()
        .into_iter()
        .find(|s| s["name"] == name)
        .ok_or_else(|| anyhow!("Неизвестная команда"))?;
    ensure!(a.is_object(), "Аргументы должны быть объектом");
    for o in spec["options"].as_array().unwrap() {
        let k = o["name"].as_str().unwrap();
        if a.get(k).is_none() {
            a[k] = o["default"].clone();
        }
    }
    validate(name, &a)?;
    Ok(a)
}
pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
pub fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
pub fn subset(v: &Value, keys: &[&str]) -> Value {
    let mut o = json!({});
    for k in keys {
        if let Some(value) = v.get(k) {
            o[*k] = value.clone();
        }
    }
    o
}
