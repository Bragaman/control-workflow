use crate::{
    args::s,
    config::{Context, location},
    external,
    review::digest,
    store,
};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
#[path = "resources.rs"]
mod resources;
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| p.is_file() && p.metadata().is_ok_and(|m| m.mode() & 0o111 != 0))
}
pub fn doctor(c: &Context) -> Result<Value> {
    let mut v = json!({"platform":std::env::consts::OS,"version":env!("CARGO_PKG_VERSION"),"program_root":c.program,"state_root":c.root,"bindings_root":c.bindings,"config":c.config,"adapter":c.settings["adapter"],"capabilities":external::capabilities(s(&c.settings,"adapter")),"process_identity":external::identity(std::process::id() as i64).is_some(),"machine_identity":!external::machine_id()?.is_empty()});
    for (k, dir) in [
        ("state_writable", &c.root),
        ("bindings_writable", &c.bindings),
    ] {
        fs::create_dir_all(dir)?;
        tempfile::tempfile_in(dir)?;
        v[k] = json!(true);
    }
    for name in [
        "git",
        c.machine["herdr_executable"].as_str().unwrap_or("herdr"),
        if c.settings["adapter"] == "herdr-claude" {
            "claude"
        } else {
            "codex"
        },
    ] {
        let exe = which(name);
        let mut info = json!({"found":exe.is_some()});
        if let Some(exe) = exe {
            let (_, out, _) =
                external::output(Command::new(exe).arg("--version"), Duration::from_secs(10))?;
            info["version"] = json!(out.lines().take(1).collect::<Vec<_>>());
        }
        v[name] = info;
    }
    if let Some(session) = &c.session {
        v["herdr_server"] = json!({"session":session,"reachable":c.herdr_text(&["status","server"],Some(session),Duration::from_secs(10)).is_ok()});
    }
    Ok(v)
}
fn validate_manifest(root: &Path) -> Result<Value> {
    let m = store::raw(&root.join("distribution-rust.json"))?;
    ensure!(
        m["schema_version"] == 1 && m["implementation"] == "rust",
        "Каталог не принадлежит Rust-установщику"
    );
    let files = m["files"].as_object().ok_or_else(|| anyhow!("Нет files"))?;
    for (name, hash) in files {
        let path = Path::new(name);
        ensure!(
            path.is_relative()
                && !path.components().any(|p| matches!(
                    p,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )),
            "Недопустимый путь manifest"
        );
        let full = root.join(path);
        let mut prefix = root.to_path_buf();
        for p in path.components() {
            prefix.push(p);
            ensure!(
                !fs::symlink_metadata(&prefix)?.file_type().is_symlink(),
                "Symlink в дистрибутиве"
            );
        }
        ensure!(
            digest(&fs::read(full)?) == hash.as_str().unwrap_or(""),
            "Хеш manifest не совпал: {name}"
        );
    }
    fn walk(root: &Path, dir: &Path, files: &serde_json::Map<String, Value>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let e = entry?;
            ensure!(!e.file_type()?.is_symlink(), "Symlink в установке");
            if e.file_type()?.is_dir() {
                walk(root, &e.path(), files)?;
            } else {
                let rel = e.path().strip_prefix(root)?.to_string_lossy().to_string();
                ensure!(
                    rel == "distribution-rust.json" || files.contains_key(&rel),
                    "Непредусмотренный файл в установке: {rel}"
                );
            }
        }
        Ok(())
    }
    walk(root, root, files)?;
    Ok(m)
}
pub fn install(c: &Context, prefix: &str, package: bool) -> Result<Value> {
    let requested = crate::config::absolute(prefix)?;
    ensure!(!requested.is_symlink(), "prefix не должен быть symlink");
    let target = location(&requested)?;
    let source = location(env!("CARGO_MANIFEST_DIR"))?;
    for protected in [&source, &c.program, &c.root, &c.bindings] {
        ensure!(
            !target.starts_with(protected) && !protected.starts_with(&target),
            "prefix должен быть вне исходников, ресурсов, state и bindings"
        );
    }
    if target.exists() {
        ensure!(!package, "Каталог пакета уже существует");
        validate_manifest(&target)?;
    }
    let parent = target.parent().ok_or_else(|| anyhow!("Неверный prefix"))?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".workflow-stage-")
        .tempdir_in(parent)?;
    let mut files = json!({});
    let mut write = |name: &str, bytes: &[u8], executable: bool| -> Result<()> {
        let p = stage.path().join(name);
        fs::create_dir_all(p.parent().unwrap())?;
        fs::write(&p, bytes)?;
        fs::set_permissions(
            &p,
            fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
        )?;
        files[name] = json!(digest(bytes));
        Ok(())
    };
    write(
        "bin/control-workflow",
        &fs::read(std::env::current_exe()?)?,
        true,
    )?;
    for (name, text) in resources::FILES {
        write(name, text.as_bytes(), false)?;
    }
    write(
        "config/defaults.json",
        include_bytes!("../config/defaults.json"),
        false,
    )?;
    write("README.md", include_bytes!("../README.md"), false)?;
    write("LICENSE", include_bytes!("../LICENSE"), false)?;
    store::save_raw(
        &stage.path().join("distribution-rust.json"),
        &json!({"schema_version":1,"implementation":"rust","version":env!("CARGO_PKG_VERSION"),"files":files}),
    )?;
    let backup = if target.exists() {
        let temp = tempfile::Builder::new()
            .prefix(".workflow-backup-")
            .tempdir_in(parent)?;
        let path = temp.path().to_path_buf();
        temp.close()?;
        fs::rename(&target, &path)?;
        Some(path)
    } else {
        None
    };
    let path = stage.keep();
    if let Err(e) = fs::rename(&path, &target) {
        if let Some(b) = &backup {
            fs::rename(b, &target)?;
        }
        let _ = fs::remove_dir_all(&path);
        return Err(e.into());
    }
    Ok(
        json!({"program_root":target,"files":files.as_object().unwrap().len(),"backup":backup,"state_copied":false}),
    )
}

/// Validate all links before changing any. Foreign entries are never replaced.
pub fn skills(c: &Context, destination: &str, action: &str) -> Result<Value> {
    let destination = location(destination)?;
    let names = [
        "control-list",
        "control-project",
        "control-task",
        "control-start",
        "control-review",
        "control-complete",
        "control-memory",
        "control-sync",
    ];
    let pairs = names
        .iter()
        .map(|name| (c.program.join("skills").join(name), destination.join(name)))
        .collect::<Vec<_>>();
    for (source, target) in &pairs {
        ensure!(
            source.join("SKILL.md").is_file(),
            "Нет исходника: {}",
            source.display()
        );
        let present = fs::symlink_metadata(target).is_ok();
        let owned =
            target.is_symlink() && fs::canonicalize(target).ok() == fs::canonicalize(source).ok();
        ensure!(
            !present || owned,
            "Чужой файл или ссылка: {}",
            target.display()
        );
        ensure!(
            action != "check" || owned,
            "Не подключён: {}",
            target.display()
        );
    }
    if action == "install" {
        fs::create_dir_all(&destination)?;
    }
    for (source, target) in &pairs {
        if action == "install" && !target.is_symlink() {
            std::os::unix::fs::symlink(source, target)?;
        } else if action == "remove" && target.is_symlink() {
            fs::remove_file(target)?;
        }
    }
    Ok(json!({"action":action,"links":pairs.iter().map(|(_,p)|p).collect::<Vec<_>>()}))
}
