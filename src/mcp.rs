//! Newline-delimited JSON-RPC stdio. State is re-read per call; no subprocess CLI.
use crate::{args, config::Context, core, lifecycle};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
const PROTOCOL: &str = "2025-06-18";
const MAX_FRAME: usize = 16 * 1024 * 1024;
pub fn tools() -> Vec<Value> {
    args::registry().into_iter().filter(|v|!v["name"].as_str().unwrap().starts_with('_')&&!["mcp","install","package"].contains(&v["name"].as_str().unwrap())).map(|spec|{
    let name=spec["name"].as_str().unwrap();let read_only=["environment","projects","tasks","show","run","memory-list","notifications","review-read","review-show"].contains(&name);let mut properties=json!({});let mut required=vec![];for option in spec["options"].as_array().unwrap(){let k=option["name"].as_str().unwrap();let mut schema=if option["boolean"]==true{json!({"type":"boolean"})}else if option["integer"]==true{json!({"type":"integer"})}else if option["multiple"]==true{json!({"type":"array","items":{"type":"string"}})}else{json!({"type":"string"})};if !option["choices"].is_null(){schema["enum"]=option["choices"].clone();}
if !option["default"].is_null(){schema["default"]=option["default"].clone();}
if option["required"]==true{required.push(k);}properties[k]=schema;}
    json!({"name":name,"description":description(name),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"outputSchema":{"type":"object","properties":{"result":{},"nonFiniteNumbers":{"type":"object","description":"JSON Pointer to Python non-finite number tag; result uses the same string tag at these paths.","additionalProperties":{"enum":["Infinity","-Infinity","NaN"]}}},"required":["result"]},"annotations":{"readOnlyHint":read_only,"destructiveHint":!read_only,"idempotentHint":read_only,"openWorldHint":!read_only}})
}).collect()
}
fn description(name: &str) -> String {
    match name{"complete"=>"Приёмка/отмена и Git/закрытие таба. Вызывать только по явному поручению пользователя; by должен совпадать с reviewer.","accept"=>"Принять результат только по явному поручению назначенного reviewer. Не выполняет Git/cleanup.","open"=>"Открыть явно выбранные задачи; возвращает текущего исполнителя или готовит новый запуск. Не повторять внешние действия при неизвестном результате.","finish"=>"Сохранить отчёт текущего запуска и передать в review; не означает пользовательскую приёмку.","review-feedback"=>"Подготовить возврат замечаний исходному исполнителю; отправка сообщения выполняется управляющим.",_=>name}.into()
}
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
pub fn serve(c: &Context) -> Result<()> {
    let input = io::stdin();
    let mut input = input.lock();
    let output = io::stdout();
    let mut output = output.lock();
    let mut initialized = false;
    loop {
        let mut bytes = vec![];
        let mut too_long = false;
        loop {
            let available = input.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let end = available
                .iter()
                .position(|b| *b == b'\n')
                .map(|i| i + 1)
                .unwrap_or(available.len());
            if bytes.len().saturating_add(end) > MAX_FRAME {
                too_long = true;
                break;
            }
            bytes.extend_from_slice(&available[..end]);
            input.consume(end);
            if bytes.last() == Some(&b'\n') {
                break;
            }
        }
        if bytes.is_empty() && !too_long {
            break;
        }
        let response = if too_long {
            Some(rpc_error(
                Value::Null,
                -32700,
                "Превышен предел JSON-RPC frame",
            ))
        } else {
            match serde_json::from_slice::<Value>(&bytes) {
                Err(_) => Some(rpc_error(Value::Null, -32700, "Parse error")),
                Ok(v) => dispatch(c, &v, &mut initialized),
            }
        };
        if let Some(v) = response {
            serde_json::to_writer(&mut output, &v)?;
            writeln!(output)?;
            output.flush()?;
        }
        if too_long {
            break;
        }
    }
    Ok(())
}
fn dispatch(c: &Context, v: &Value, initialized: &mut bool) -> Option<Value> {
    let id = v.get("id").cloned();
    if v["jsonrpc"] != "2.0" || !v["method"].is_string() {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "Invalid Request",
        ));
    }
    let method = v["method"].as_str().unwrap();
    let id = id?;
    if !(id.is_string() || id.is_number()) {
        return Some(rpc_error(Value::Null, -32600, "Invalid id"));
    }
    let result = match method {
        "initialize" => {
            if *initialized {
                return Some(rpc_error(id, -32600, "Уже инициализирован"));
            }
            if !v["params"].is_object() || !v["params"]["protocolVersion"].is_string() {
                return Some(rpc_error(id, -32602, "Нужен protocolVersion"));
            }
            *initialized = true;
            json!({"protocolVersion":if ["2024-11-05","2025-03-26",PROTOCOL].contains(&v["params"]["protocolVersion"].as_str().unwrap()){v["params"]["protocolVersion"].as_str().unwrap()}else{PROTOCOL},"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"control-workflow","version":env!("CARGO_PKG_VERSION")},"instructions":"Экземпляр state/config/bindings закреплён аргументами сервера. Запускайте только выбранные пользователем задачи. complete/accept требуют поручения reviewer. Не повторяйте мутации вслепую после разрыва; сверяйте run/intents."})
        }
        "ping" => json!({}),
        _ if !*initialized => return Some(rpc_error(id, -32000, "Сначала initialize")),
        "tools/list" => json!({"tools":tools()}),
        "tools/call" => {
            let name = v["params"]["name"].as_str().unwrap_or("");
            if !tools().iter().any(|t| t["name"] == name) {
                return Some(rpc_error(id, -32602, "Неизвестный tool"));
            }
            let a = v["params"].get("arguments").cloned().unwrap_or(json!({}));
            match args::defaults(name, a).and_then(|a| core::execute(c, name, &a)) {
                Ok(value) => {
                    let is_error =
                        ["open", "complete"].contains(&name) && lifecycle::failure(&value);
                    let text = serde_json::to_string(&value).unwrap();
                    let mut structured = json!({});
                    structured["result"] = value;
                    if contains_nonfinite(&structured["result"]) {
                        let mut tags = json!({});
                        encode_nonfinite(&mut structured["result"], "", &mut tags);
                        structured["nonFiniteNumbers"] = tags;
                    }
                    json!({"content":[{"type":"text","text":text}],"structuredContent":structured,"isError":is_error})
                }
                Err(e) => {
                    json!({"content":[{"type":"text","text":json!({"error":e.to_string(),"error_code":crate::external::error_code(&e)}).to_string()}],"isError":true})
                }
            }
        }
        _ => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

// Python's file JSON permits IEEE values absent from JSON-RPC. Keep the original
// representation in text and losslessly tag structured numbers by JSON Pointer.
fn nonfinite_tag(value: &Value) -> Option<&'static str> {
    let Value::Number(number) = value else {
        return None;
    };
    let raw = number.to_string();
    match raw.as_str() {
        "NaN" => Some("NaN"),
        "Infinity" => Some("Infinity"),
        "-Infinity" => Some("-Infinity"),
        _ if raw.contains(['.', 'e', 'E']) => raw.parse::<f64>().ok().and_then(|n| {
            if n.is_infinite() {
                Some(if n.is_sign_negative() {
                    "-Infinity"
                } else {
                    "Infinity"
                })
            } else {
                None
            }
        }),
        _ => None,
    }
}
fn contains_nonfinite(value: &Value) -> bool {
    match value {
        Value::Number(_) => nonfinite_tag(value).is_some(),
        Value::Array(values) => values.iter().any(contains_nonfinite),
        Value::Object(values) => values.values().any(contains_nonfinite),
        _ => false,
    }
}
fn encode_nonfinite(value: &mut Value, path: &str, tags: &mut Value) {
    if let Some(tag) = nonfinite_tag(value) {
        tags[path] = json!(tag);
        *value = json!(tag);
    } else {
        match value {
            Value::Array(values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    encode_nonfinite(value, &format!("{path}/{index}"), tags);
                }
            }
            Value::Object(values) => {
                for (key, value) in values.iter_mut() {
                    let key = key.replace('~', "~0").replace('/', "~1");
                    encode_nonfinite(value, &format!("{path}/{key}"), tags);
                }
            }
            _ => {}
        }
    }
}
