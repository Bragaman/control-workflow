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
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
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
        if crate::hc::enabled(&c.settings) {
            crate::hc::executable(c)
        } else {
            c.machine["herdr_executable"].as_str().unwrap_or("herdr")
        },
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
    if !crate::hc::enabled(&c.settings)
        && let Some(session) = &c.session
    {
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
    if let Some(config) = &c.config {
        ensure!(
            !config.starts_with(&target),
            "config находится внутри prefix"
        );
    }
    let parent = target.parent().ok_or_else(|| anyhow!("Неверный prefix"))?;
    fs::create_dir_all(parent)?;
    let lock_path = parent.join(format!(
        ".workflow-install-{}.lock",
        digest(target.to_string_lossy().as_bytes())
    ));
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Другая установка уже выполняется"
    );
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
    write("CHANGELOG.md", include_bytes!("../CHANGELOG.md"), false)?;
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
        Some(path)
    } else {
        None
    };
    let path = stage.keep();
    switch_install(&path, &target, backup.as_deref(), |from, to| {
        fs::rename(from, to)
    })?;
    Ok(
        json!({"program_root":target,"files":files.as_object().unwrap().len(),"backup":backup,"state_copied":false}),
    )
}

// The same switch and recovery path is used by the installer and the failure test.
fn switch_install<F>(
    stage: &Path,
    target: &Path,
    backup: Option<&Path>,
    mut rename: F,
) -> Result<()>
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    if let Some(backup) = backup
        && let Err(error) = rename(target, backup)
    {
        let _ = fs::remove_dir_all(stage);
        return Err(error.into());
    }
    if let Err(error) = rename(stage, target) {
        if let Some(backup) = backup {
            rename(backup, target)?;
        }
        let _ = fs::remove_dir_all(stage);
        return Err(error.into());
    }
    Ok(())
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
        "control-git-publish",
        "control-memory",
        "control-sync",
        "control-update",
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

const RELEASES: &str = "https://github.com/Bragaman/control-workflow/releases/download";

fn release_platform(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        _ => Err(anyhow!("Платформа не поддерживается: {os}/{arch}")),
    }
}
fn release_version(version: &str) -> Result<&str> {
    ensure!(
        regex::Regex::new(r"^v[0-9]+\.[0-9]+\.[0-9]+(?:-rc\.[0-9]+)?$")?.is_match(version),
        "Неверная версия выпуска: {version}"
    );
    Ok(version)
}
fn download(url: &str, path: &Path, limit: &str) -> Result<()> {
    let (ok, _, err) = external::output(
        Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "15",
                "--max-time",
                "120",
                "--max-filesize",
                limit,
                "--output",
            ])
            .arg(path)
            .arg(url),
        Duration::from_secs(130),
    )?;
    ensure!(ok, "Ошибка загрузки: {}", err.trim());
    Ok(())
}
// Read only regular files and directories; never let tar interpret links or paths.
fn unpack_release(archive: &Path, destination: &Path) -> Result<()> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new("gzip")
        .arg("-dc")
        .arg(archive)
        .stdout(Stdio::piped())
        .spawn()?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .take(256 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let status = child.wait()?;
    ensure!(
        status.success() && bytes.len() <= 256 * 1024 * 1024,
        "Повреждённый или слишком большой архив"
    );
    let mut offset = 0usize;
    let mut names = std::collections::HashSet::new();
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|&b| b == 0) {
            return Ok(());
        }
        let octal = |b: &[u8]| -> Result<usize> {
            let s = std::str::from_utf8(b)?.trim_matches(['\0', ' ']);
            Ok(if s.is_empty() {
                0
            } else {
                usize::from_str_radix(s, 8)?
            })
        };
        let expected = octal(&header[148..156])?;
        let sum: usize = header
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    b as usize
                }
            })
            .sum();
        ensure!(sum == expected, "Ошибка заголовка tar");
        let text = |b: &[u8]| -> Result<String> {
            Ok(std::str::from_utf8(b.split(|&x| x == 0).next().unwrap())?.to_owned())
        };
        let name = text(&header[..100])?;
        let prefix = text(&header[345..500])?;
        let full = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let rel = Path::new(&full);
        let mut clean = PathBuf::new();
        for part in rel.components() {
            match part {
                std::path::Component::Normal(p) => clean.push(p),
                std::path::Component::CurDir => (),
                _ => return Err(anyhow!("Недопустимый путь в архиве")),
            }
        }
        let size = octal(&header[124..136])?;
        let start = offset + 512;
        let end = start
            .checked_add(size)
            .ok_or_else(|| anyhow!("Некорректный размер"))?;
        ensure!(end <= bytes.len(), "Обрезанный архив");
        let next = end
            .checked_add(511)
            .ok_or_else(|| anyhow!("Некорректный размер"))?
            / 512
            * 512;
        ensure!(next <= bytes.len(), "Обрезанный архив");
        let target = destination.join(&clean);
        match header[156] {
            b'5' if size == 0 => {
                fs::create_dir_all(&target)?;
            }
            b'0' | 0 if !clean.as_os_str().is_empty() => {
                ensure!(names.insert(clean), "Повтор файла в архиве");
                fs::create_dir_all(target.parent().unwrap())?;
                fs::write(&target, &bytes[start..end])?;
            }
            _ => return Err(anyhow!("Недопустимый тип записи tar")),
        }
        offset = next;
    }
    Err(anyhow!("Обрезанный архив"))
}

pub fn update(c: &Context, version: Option<&str>, check: bool) -> Result<Value> {
    let platform = release_platform(std::env::consts::OS, std::env::consts::ARCH)?;
    let exe = fs::canonicalize(std::env::current_exe()?)?;
    let root = exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("Не удалось определить prefix"))?;
    ensure!(
        exe == root.join("bin/control-workflow") && c.program == root,
        "Запустите установленный bin/control-workflow с его program-root"
    );
    let installed = validate_manifest(root)?;
    let current = installed["version"]
        .as_str()
        .ok_or_else(|| anyhow!("Нет версии установки"))?;
    let parent = root
        .parent()
        .ok_or_else(|| anyhow!("Нет родительского каталога prefix"))?;
    let temp = tempfile::Builder::new()
        .prefix(".workflow-update-")
        .tempdir_in(parent)?;
    let chosen = if let Some(v) = version {
        release_version(v)?.to_owned()
    } else {
        let path = temp.path().join("latest.json");
        download(
            "https://api.github.com/repos/Bragaman/control-workflow/releases/latest",
            &path,
            "1048576",
        )?;
        let response: Value = serde_json::from_slice(&fs::read(path)?)?;
        release_version(
            response["tag_name"]
                .as_str()
                .ok_or_else(|| anyhow!("Нет tag_name в ответе GitHub"))?,
        )?
        .to_owned()
    };
    if check || chosen.strip_prefix('v') == Some(current) {
        return Ok(
            json!({"current":current,"available":chosen,"platform":platform,"status":if check {"checked"} else {"unchanged"}}),
        );
    }
    let archive = format!("control-workflow-{chosen}-{platform}.tar.gz");
    let sums = temp.path().join("SHA256SUMS");
    let file = temp.path().join(&archive);
    download(&format!("{RELEASES}/{chosen}/SHA256SUMS"), &sums, "1048576")?;
    download(
        &format!("{RELEASES}/{chosen}/{archive}"),
        &file,
        "134217728",
    )?;
    let contents = fs::read_to_string(sums)?;
    let matches: Vec<_> = contents
        .lines()
        .filter_map(|l| l.split_once("  "))
        .filter(|(_, name)| *name == archive)
        .collect();
    ensure!(
        matches.len() == 1 && matches[0].0.len() == 64 && digest(&fs::read(&file)?) == matches[0].0,
        "Контрольная сумма поставки не совпала"
    );
    let unpacked = temp.path().join("package");
    fs::create_dir(&unpacked)?;
    unpack_release(&file, &unpacked)?;
    let manifest = validate_manifest(&unpacked)?;
    ensure!(
        manifest["version"].as_str() == chosen.strip_prefix('v'),
        "Версия поставки не совпала с выбранной"
    );
    let candidate = unpacked.join("bin/control-workflow");
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755))?;
    let mut cmd = Command::new(candidate);
    cmd.arg("--program-root")
        .arg(&unpacked)
        .arg("--state-root")
        .arg(&c.root)
        .arg("--bindings-root")
        .arg(&c.bindings);
    if let Some(config) = &c.config {
        cmd.arg("--config").arg(config);
    }
    let (ok, out, err) = external::output(
        cmd.arg("install").arg("--prefix").arg(root),
        Duration::from_secs(120),
    )?;
    ensure!(ok, "Ошибка установки: {} {}", out.trim(), err.trim());
    let result: Value = serde_json::from_str(&out)?;
    Ok(
        json!({"status":"updated","previous":current,"version":chosen,"platform":platform,"installation":result}),
    )
}

#[cfg(test)]
mod update_tests {
    use super::*;
    #[test]
    fn failed_switch_restores_previous_install_and_retry_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("program");
        let stage = temp.path().join("stage");
        let backup = temp.path().join("backup");
        let data = temp.path().join("state");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(target.join("binary"), b"old").unwrap();
        fs::write(stage.join("binary"), b"new").unwrap();
        fs::write(&data, b"context").unwrap();
        let mut calls = 0;
        let failed = switch_install(&stage, &target, Some(&backup), |from, to| {
            calls += 1;
            if calls == 2 {
                Err(std::io::Error::other("injected switch failure"))
            } else {
                fs::rename(from, to)
            }
        });
        assert!(failed.is_err());
        assert_eq!(calls, 3);
        assert_eq!(fs::read(target.join("binary")).unwrap(), b"old");
        assert_eq!(fs::read(&data).unwrap(), b"context");
        assert!(!stage.exists());
        assert!(!backup.exists());
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("binary"), b"new").unwrap();
        switch_install(&stage, &target, Some(&backup), |from, to| {
            fs::rename(from, to)
        })
        .unwrap();
        assert_eq!(fs::read(target.join("binary")).unwrap(), b"new");
        assert_eq!(fs::read(backup.join("binary")).unwrap(), b"old");
        assert_eq!(fs::read(&data).unwrap(), b"context");
    }

    #[test]
    fn platforms_and_versions_are_strict() {
        assert_eq!(
            release_platform("macos", "aarch64").unwrap(),
            "macos-aarch64"
        );
        assert!(release_platform("macos", "x86_64").is_err());
        assert!(release_platform("windows", "x86_64").is_err());
        assert!(release_version("v1.2.3-rc.1").is_ok());
        assert!(release_version("../../latest").is_err());
    }
}
