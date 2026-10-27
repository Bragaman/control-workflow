use crate::config::Context;
use anyhow::{Result, ensure};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
pub fn ident(s: &str) -> Result<&str> {
    ensure!(
        !s.is_empty()
            && s.len() <= 64
            && s.bytes().enumerate().all(|(i, c)| c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || (i > 0 && (c == b'_' || c == b'-'))),
        "ID: 1–64 символа a-z, 0-9, _ или -"
    );
    Ok(s)
}
pub fn raw(path: &Path) -> Result<Value> {
    python_json(&fs::read(path)?)
}
/// File JSON follows Python's default NaN/Infinity extension. JSON-RPC stays strict.
fn python_json(data: &[u8]) -> Result<Value> {
    let original_error = match serde_json::from_slice(data) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let text = std::str::from_utf8(data)?;
    if !text.contains("NaN") && !text.contains("Infinity") {
        return Err(original_error.into());
    }
    // Numeric placeholders cannot collide with a source number: JSON numbers have
    // no escapes, and this prefix is absent from the original bytes. Strings are
    // never rewritten. Arbitrary precision retains these exact decimal tokens.
    let prefix = loop {
        let candidate = format!("0.{}", uuid::Uuid::new_v4().as_u128());
        if !text.contains(&candidate) {
            break candidate;
        }
    };
    let replacements = [
        ("-Infinity", format!("{prefix}1")),
        ("Infinity", format!("{prefix}2")),
        ("NaN", format!("{prefix}3")),
    ];
    let boundary = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    let mut rewritten = Vec::with_capacity(data.len());
    let (mut quoted, mut escaped, mut count, mut index) = (false, false, 0, 0);
    while index < data.len() {
        let byte = data[index];
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if (index == 0 || boundary(data[index - 1]) || b"[:,".contains(&data[index - 1]))
            && let Some((token, marker)) = replacements.iter().find(|(token, _)| {
                data[index..].starts_with(token.as_bytes())
                    && (index + token.len() == data.len()
                        || boundary(data[index + token.len()])
                        || b",]}".contains(&data[index + token.len()]))
            })
        {
            rewritten.extend_from_slice(marker.as_bytes());
            index += token.len();
            count += 1;
            continue;
        }
        rewritten.push(byte);
        index += 1;
    }
    if count == 0 {
        return Err(original_error.into());
    }
    let mut value: Value = serde_json::from_slice(&rewritten)?;
    fn restore(value: &mut Value, replacements: &[(&str, String)]) {
        match value {
            Value::Number(number) => {
                let raw = number.to_string();
                if let Some((token, _)) = replacements.iter().find(|(_, marker)| *marker == raw) {
                    // Only these three hard-coded Python tokens bypass JSON grammar.
                    *number = serde_json::Number::from_string_unchecked((*token).into());
                }
            }
            Value::Array(values) => {
                for value in values {
                    restore(value, replacements);
                }
            }
            Value::Object(values) => {
                for value in values.values_mut() {
                    restore(value, replacements);
                }
            }
            _ => {}
        }
    }
    restore(&mut value, &replacements);
    Ok(value)
}
pub fn atomic(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut tmp = NamedTempFile::with_prefix_in(".tmp-", parent)?;
    tmp.write_all(data)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn save_raw(path: &Path, v: &Value) -> Result<()> {
    let mut b = serde_json::to_vec_pretty(v)?;
    b.push(b'\n');
    atomic(path, &b)
}
pub struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl Context {
    pub fn lock(&self) -> Result<Lock> {
        fs::create_dir_all(&self.root)?;
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(".control.lock"))?;
        ensure!(
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0,
            "flock: {}",
            std::io::Error::last_os_error()
        );
        Ok(Lock(f))
    }
    pub fn path(&self, kind: &str, key: &str) -> Result<PathBuf> {
        ensure!(
            ["projects", "tasks", "notifications"].contains(&kind),
            "Неизвестный вид записи"
        );
        Ok(self.root.join(kind).join(format!("{}.json", ident(key)?)))
    }
    pub fn run_file(&self, key: &str) -> Result<PathBuf> {
        Ok(self.root.join("runs").join(ident(key)?).join("state.json"))
    }
    pub fn read(&self, path: &Path) -> Result<Value> {
        let mut obj = raw(path)?;
        if let Some(kind) = self.record_kind(path) {
            let binding = self.bindings.join(kind).join(path.file_name().unwrap());
            if binding.exists() {
                let b = raw(&binding)?;
                ensure!(b.is_object(), "Привязка должна быть объектом");
                obj.as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("Запись должна быть объектом"))?
                    .extend(b.as_object().unwrap().clone());
            }
            let o = obj
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Запись должна быть объектом"))?;
            o.entry("repositories").or_insert(if kind == "projects" {
                serde_json::json!([])
            } else {
                Value::Null
            });
            if kind == "projects" {
                o.entry("workspace").or_insert(Value::Null);
            }
        }
        Ok(obj)
    }
    fn record_kind(&self, path: &Path) -> Option<&'static str> {
        ["projects", "tasks"]
            .into_iter()
            .find(|kind| path.parent() == Some(self.root.join(kind).as_path()))
    }
    pub fn save(&self, path: &Path, v: &Value) -> Result<()> {
        if let Some(kind) = self.record_kind(path) {
            let fields = if kind == "projects" {
                vec![
                    "repositories",
                    "workspace",
                    "workspace_namespace",
                    "layout_pending",
                    "cwd",
                ]
            } else {
                vec!["repositories"]
            };
            let binding = self.bindings.join(kind).join(path.file_name().unwrap());
            let mut b = if binding.exists() {
                raw(&binding)?
            } else {
                serde_json::json!({})
            };
            let mut state = v.clone();
            for k in fields {
                if let Some(val) = state.as_object_mut().unwrap().remove(k) {
                    b[k] = val;
                } else {
                    b.as_object_mut().unwrap().remove(k);
                }
            }
            save_raw(&binding, &b)?;
            save_raw(path, &state)
        } else {
            save_raw(path, v)
        }
    }
    pub fn task(&self, key: &str) -> Result<Value> {
        self.read(&self.path("tasks", key)?)
    }
    pub fn current(&self, t: &Value, key: &str) -> Result<Value> {
        ensure!(
            t["active_run"] == key,
            "Этот запуск уже не является текущим для задачи"
        );
        self.read(&self.run_file(key)?)
    }
    pub fn update(&self, r: &mut Value, t: &mut Value) -> Result<()> {
        r["updated_at"] = serde_json::json!(now());
        t["updated_at"] = r["updated_at"].clone();
        self.save(&self.run_file(crate::args::s(r, "id"))?, r)?;
        self.save(&self.path("tasks", crate::args::s(t, "id"))?, t)
    }
    pub fn rows(&self, kind: &str) -> Result<Vec<Value>> {
        json_files(&self.root.join(kind))?
            .iter()
            .map(|p| self.read(p))
            .collect()
    }
    pub fn runs(&self) -> Result<Vec<Value>> {
        let dir = self.root.join("runs");
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut files = fs::read_dir(dir)?
            .map(|e| Ok(e?.path().join("state.json")))
            .collect::<Result<Vec<_>>>()?;
        files.sort();
        files
            .into_iter()
            .filter(|p| p.is_file())
            .map(|p| self.read(&p))
            .collect()
    }
}
pub fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut paths = fs::read_dir(dir)?
        .map(|e| Ok(e?.path()))
        .collect::<Result<Vec<_>>>()?;
    paths.retain(|p| p.extension().is_some_and(|s| s == "json"));
    paths.sort();
    Ok(paths)
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}
pub fn key(prefix: &str) -> String {
    format!(
        "{}{}",
        prefix,
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}
