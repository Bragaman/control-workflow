use crate::{args::s, store};
use anyhow::{Result, ensure};
use clap::ArgMatches;
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
};
#[derive(Clone)]
pub struct Context {
    pub root: PathBuf,
    pub bindings: PathBuf,
    pub settings: Value,
    pub machine: Value,
    pub config: Option<PathBuf>,
    pub program: PathBuf,
    pub session: Option<String>,
}
pub fn location(value: impl AsRef<Path>) -> Result<PathBuf> {
    let path = value.as_ref();
    let text = path.to_string_lossy();
    let expanded = if text == "~" {
        PathBuf::from(env::var("HOME")?)
    } else if let Some(p) = text.strip_prefix("~/") {
        PathBuf::from(env::var("HOME")?).join(p)
    } else {
        path.to_path_buf()
    };
    let abs = if expanded.is_absolute() {
        expanded
    } else {
        env::current_dir()?.join(expanded)
    };
    resolve(&abs)
}
/// Expand user paths without following the final symlink (review artifacts).
pub fn absolute(value: impl AsRef<Path>) -> Result<PathBuf> {
    let text = value.as_ref().to_string_lossy();
    let p = if text == "~" {
        PathBuf::from(env::var("HOME")?)
    } else if let Some(rest) = text.strip_prefix("~/") {
        PathBuf::from(env::var("HOME")?).join(rest)
    } else {
        value.as_ref().to_path_buf()
    };
    Ok(if p.is_absolute() {
        p
    } else {
        env::current_dir()?.join(p)
    })
}
fn resolve(p: &Path) -> Result<PathBuf> {
    resolve_depth(p, 0)
}
fn resolve_depth(p: &Path, depth: usize) -> Result<PathBuf> {
    ensure!(depth <= 40, "Слишком много symlink в пути");
    let mut result = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::RootDir => result.push("/"),
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                result.push(name);
                match fs::symlink_metadata(&result) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        let link = fs::read_link(&result)?;
                        let next = if link.is_absolute() {
                            link
                        } else {
                            result.parent().unwrap().join(link)
                        };
                        result = resolve_depth(&next, depth + 1)?;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            _ => anyhow::bail!("Поддерживаются пути Unix"),
        }
    }
    Ok(result)
}
fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
pub fn merge(base: &mut Value, over: &Value) -> Result<()> {
    ensure!(over.is_object(), "Конфигурация должна быть JSON-объектом");
    for (k, v) in over.as_object().unwrap() {
        if base[k].is_object() && v.is_object() {
            merge(&mut base[k], v)?;
        } else {
            base[k] = v.clone();
        }
    }
    Ok(())
}
impl Context {
    pub fn load(a: &ArgMatches) -> Result<Self> {
        let home = PathBuf::from(env::var("HOME")?);
        let (default_state, default_config) = if cfg!(target_os = "macos") {
            let b = home.join("Library/Application Support/control-workflow");
            (b.join("state"), b.join("config.json"))
        } else {
            ensure!(cfg!(target_os = "linux"), "Поддерживаются Linux и macOS");
            (
                env_path("XDG_STATE_HOME")
                    .unwrap_or(home.join(".local/state"))
                    .join("control-workflow"),
                env_path("XDG_CONFIG_HOME")
                    .unwrap_or(home.join(".config"))
                    .join("control-workflow/config.json"),
            )
        };
        let config = a
            .get_one::<String>("config")
            .map(PathBuf::from)
            .or_else(|| env_path("CONTROL_PLANE_CONFIG"))
            .or_else(|| default_config.is_file().then_some(default_config))
            .map(location)
            .transpose()?;
        let mut settings: Value = serde_json::from_str(include_str!("../config/defaults.json"))?;
        if let Some(p) = &config {
            merge(&mut settings, &store::raw(p)?)?;
        }
        for k in ["state_root", "bindings_root"] {
            if let Some(v) = settings[k].as_str() {
                ensure!(
                    v.starts_with('~') || Path::new(v).is_absolute(),
                    "{k} в config должен быть абсолютным путём"
                );
            }
        }
        let root = location(
            a.get_one::<String>("state-root")
                .map(PathBuf::from)
                .or_else(|| env_path("CONTROL_PLANE_STATE"))
                .or_else(|| env_path("CONTROL_PLANE_ROOT"))
                .or_else(|| settings["state_root"].as_str().map(PathBuf::from))
                .unwrap_or(default_state),
        )?;
        let bindings = location(
            a.get_one::<String>("bindings-root")
                .map(PathBuf::from)
                .or_else(|| env_path("CONTROL_PLANE_BINDINGS"))
                .or_else(|| settings["bindings_root"].as_str().map(PathBuf::from))
                .unwrap_or(root.join("bindings")),
        )?;
        let machine = if bindings.join("runtime.json").exists() {
            store::raw(&bindings.join("runtime.json"))?
        } else {
            serde_json::json!({})
        };
        ensure!(
            machine.is_object(),
            "runtime.json должен быть JSON-объектом"
        );
        for k in ["cwd", "worktrees_root"] {
            if let Some(v) = machine[k].as_str() {
                ensure!(
                    v.starts_with('~') || Path::new(v).is_absolute(),
                    "{k} в runtime должен быть абсолютным путём"
                );
            }
        }
        ensure!(
            settings["schema_version"] == 1
                && machine
                    .get("schema_version")
                    .unwrap_or(&serde_json::json!(1))
                    == 1,
            "Неподдержанная schema_version; требуется 1"
        );
        ensure!(
            ["herdr-codex", "herdr-claude"].contains(&s(&settings, "adapter")),
            "Неизвестный adapter"
        );
        ensure!(
            ["main", "branch"].contains(&s(&settings, "git_mode")),
            "Неизвестный git_mode"
        );
        ensure!(
            settings["agent_start_timeout_ms"]
                .as_u64()
                .is_some_and(|n| n > 0),
            "agent_start_timeout_ms должен быть положительным целым числом"
        );
        for kind in ["implementation", "research", "reasoning", "planning"] {
            for c in ["normal", "complex"] {
                let p = &settings["profiles"][kind][c];
                ensure!(
                    !s(p, "model").trim().is_empty(),
                    "Профиль должен содержать model"
                );
                let allowed = if settings["adapter"] == "herdr-claude" {
                    vec!["low", "medium", "high", "xhigh", "max"]
                } else {
                    vec!["low", "medium", "high", "xhigh", "max", "ultra"]
                };
                ensure!(
                    allowed.contains(&s(p, "reasoning_effort")),
                    "Неизвестный reasoning_effort"
                );
            }
        }
        let session = a
            .get_one::<String>("herdr-session")
            .cloned()
            .or_else(|| machine["herdr_session"].as_str().map(str::to_owned));
        let exe = env::current_exe()?;
        let program = location(
            a.get_one::<String>("program-root")
                .map(PathBuf::from)
                .or_else(|| env_path("CONTROL_WORKFLOW_PROGRAM"))
                .unwrap_or_else(|| {
                    let p = exe.parent().unwrap();
                    if p.file_name().is_some_and(|v| v == "bin") {
                        p.parent().unwrap().to_path_buf()
                    } else {
                        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    }
                }),
        )?;
        Ok(Self {
            root,
            bindings,
            settings,
            machine,
            config,
            program,
            session,
        })
    }
    pub fn child(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.arg("--state-root")
            .arg(&self.root)
            .arg("--bindings-root")
            .arg(&self.bindings)
            .arg("--program-root")
            .arg(&self.program);
        if let Some(p) = &self.config {
            cmd.arg("--config").arg(p);
        }
        if let Some(s) = &self.session {
            cmd.arg("--herdr-session").arg(s);
        }
        cmd
    }
}
