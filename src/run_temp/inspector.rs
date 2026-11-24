//! The privileged entry point reports one bit about a verified, caller-owned
//! directory. All mutations and process signals stay in the unprivileged CLI.
use super::linux::{FileIds, check_visibility, file_reference};
use crate::{args::s, external, store};
use anyhow::{Context as _, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Read, Seek, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path},
    process::Command,
    time::Duration,
};

const ENTRY: &str = "_temp-inspect";
const PROTOCOL: u64 = 1;
const REQUEST_LIMIT: u64 = 8192;

pub(super) fn query(program: &Value, path: &Path, record: &Value) -> Result<bool> {
    let program = Path::new(
        program
            .as_str()
            .context("temp_inspector должен быть путём")?,
    );
    ensure!(
        normal_absolute(program),
        "Укажите абсолютный путь temp_inspector"
    );
    for item in program.ancestors() {
        let meta = fs::symlink_metadata(item)
            .with_context(|| format!("Недоступен помощник или его каталог: {}", item.display()))?;
        ensure!(
            meta.uid() == 0
                && meta.mode() & 0o022 == 0
                && if item == program {
                    meta.is_file()
                } else {
                    meta.is_dir()
                },
            "Помощник и его каталоги должны принадлежать root и быть защищены от записи: {}",
            item.display()
        );
    }

    let data = serde_json::to_vec(&json!({
        "version":PROTOCOL,"path":path,"directory":record["directory"],
        "pid_namespace":record["pid_namespace"],
    }))?;
    ensure!(
        data.len() as u64 <= REQUEST_LIMIT,
        "Запрос помощнику слишком велик"
    );
    let mut input = tempfile::tempfile()?;
    input.write_all(&data)?;
    input.rewind()?;
    let (ok, out, err) = external::output_with_input(
        Command::new("/usr/bin/sudo")
            .arg("-n")
            .arg("--")
            .arg(program)
            .arg(ENTRY),
        Duration::from_secs(20),
        input.into(),
    )
    .context("Не удалось выполнить проверку помощником")?;
    if !ok {
        let detail: String = err.trim().chars().take(400).collect();
        bail!("temp_inspector не подтвердил доступ к /proc: {detail}");
    }
    let response: Value =
        serde_json::from_str(&out).context("Некорректный ответ temp_inspector")?;
    ensure!(
        response["version"] == PROTOCOL,
        "Версия temp_inspector не совпадает"
    );
    response["used"]
        .as_bool()
        .context("В ответе temp_inspector нет признака использования")
}

// Dispatch before loading user-owned config, resources, or any runtime binding.
pub fn inspect_entry() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(ENTRY.as_ref()) {
        return false;
    }
    let result = (|| {
        ensure!(
            args.next().is_none(),
            "Помощник принимает только запрос на stdin"
        );
        ensure!(
            unsafe { libc::geteuid() } == 0,
            "Помощник должен выполняться через sudo"
        );
        let caller: u32 = std::env::var("SUDO_UID").context("Нет SUDO_UID")?.parse()?;
        ensure!(
            caller != 0,
            "Ожидался непривилегированный владелец каталога"
        );
        ensure!(has_ptrace()?, "У помощника нет CAP_SYS_PTRACE");
        let mut data = Vec::new();
        io::stdin().take(REQUEST_LIMIT + 1).read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 <= REQUEST_LIMIT,
            "Запрос помощнику слишком велик"
        );
        let request: Value =
            serde_json::from_slice(&data).context("Некорректный запрос помощнику")?;
        let used = inspect(&request, caller)?;
        println!("{}", json!({"version":PROTOCOL,"used":used}));
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
    true
}

fn has_ptrace() -> Result<bool> {
    const CAP_SYS_PTRACE: u32 = 19;
    let status = fs::read_to_string("/proc/self/status")?;
    let hex = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .context("Нет CapEff")?;
    Ok(u64::from_str_radix(hex.trim(), 16)? & (1 << CAP_SYS_PTRACE) != 0)
}

fn normal_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

fn inspect(request: &Value, caller: u32) -> Result<bool> {
    ensure!(
        request["version"] == PROTOCOL,
        "Неизвестный протокол помощника"
    );
    let path = Path::new(
        request["path"]
            .as_str()
            .context("Нет пути временного каталога")?,
    );
    ensure!(normal_absolute(path), "Недопустимый временный путь");
    let parent = path.parent().context("Нет родительского каталога")?;
    ensure!(
        parent
            .file_name()
            .is_some_and(|name| name == "workflow-temp"),
        "Путь находится вне workflow-temp"
    );
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Недопустимое имя каталога")?;
    let run = name
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(".deleting"))
        .unwrap_or(name);
    store::ident(run)?;
    ensure!(run.starts_with("run-"), "Путь не относится к запуску");
    check_visibility(s(request, "pid_namespace"))?;

    let base = open_dir(libc::AT_FDCWD, parent, false)?;
    let base_meta = require_owner(&base, caller, true)?;
    let run_dir = open_dir(base.as_raw_fd(), Path::new(name), true)?;
    let root = require_owner(&run_dir, caller, true)?;
    ensure!(
        request["directory"] == json!([root.dev(), root.ino()]),
        "Каталог заменён"
    );
    let mut directories = FileIds::new();
    collect_dirs(&run_dir, caller, &mut directories)?;
    let used = scan_threads(path, &directories)?;
    // Refuse a switched path even if it is still owned by the caller.
    let current_base = open_dir(libc::AT_FDCWD, parent, false)?;
    let current_meta = current_base.metadata()?;
    ensure!(
        (current_meta.dev(), current_meta.ino()) == (base_meta.dev(), base_meta.ino()),
        "Родительский каталог заменён"
    );
    let current = open_dir(current_base.as_raw_fd(), Path::new(name), true)?.metadata()?;
    ensure!(
        (current.dev(), current.ino()) == (root.dev(), root.ino()),
        "Каталог заменён"
    );
    Ok(used)
}

fn require_owner(file: &File, caller: u32, private: bool) -> Result<fs::Metadata> {
    let meta = file.metadata()?;
    ensure!(
        meta.is_dir() && meta.uid() == caller && (!private || meta.mode() & 0o777 == 0o700),
        "Каталог не принадлежит владельцу экземпляра"
    );
    Ok(meta)
}

fn open_dir(parent: libc::c_int, path: &Path, child: bool) -> Result<File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    // Linux openat2(2): prohibit symlinks throughout the path and forbid
    // crossing a mount or leaving the pinned parent when descending the tree.
    const NO_SYMLINKS: u64 = 0x04;
    const NO_XDEV: u64 = 0x01;
    const BENEATH: u64 = 0x08;
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: NO_SYMLINKS | if child { NO_XDEV | BENEATH } else { 0 },
    };
    let name = CString::new(path.as_os_str().as_bytes())?;
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            parent,
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("Невозможно подтвердить путь каталога");
    }
    Ok(unsafe { File::from_raw_fd(fd as libc::c_int) })
}

fn collect_dirs(dir: &File, caller: u32, directories: &mut FileIds) -> Result<()> {
    fn frame(dir: File, caller: u32, directories: &mut FileIds) -> Result<(File, fs::ReadDir)> {
        let meta = require_owner(&dir, caller, false)?;
        directories.insert((meta.dev(), meta.ino()));
        let children = fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))?;
        Ok((dir, children))
    }
    let mut stack = vec![frame(dir.try_clone()?, caller, directories)?];
    while let Some((dir, children)) = stack.last_mut() {
        if let Some(entry) = children.next() {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let child = open_dir(dir.as_raw_fd(), Path::new(&entry.file_name()), true)?;
                stack.push(frame(child, caller, directories)?);
            }
        } else {
            stack.pop();
        }
    }
    Ok(())
}

fn proc_item<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(item) => Ok(Some(item)),
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn scan_threads(path: &Path, directories: &FileIds) -> Result<bool> {
    for process in fs::read_dir("/proc")? {
        let process = process?;
        let Ok(pid) = process.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        // The helper itself holds the validated directory handle.
        if pid == std::process::id() {
            continue;
        }
        let root = process.path();
        if let Some(threads) = proc_item(fs::read_dir(root.join("task")))? {
            for thread in threads {
                let thread = thread?.path();
                if thread_references(&thread, path, directories)
                    .with_context(|| format!("Не удалось проверить cwd или fd процесса {pid}"))?
                {
                    return Ok(true);
                }
            }
        } else if let Some(status) = proc_item(fs::read_to_string(root.join("status")))? {
            // A thread-group leader can exit while its other threads survive.
            let threads = status
                .lines()
                .find_map(|line| line.strip_prefix("Threads:"))
                .context("Нет числа потоков процесса")?
                .trim()
                .parse::<u32>()?;
            ensure!(threads <= 1, "Недоступны потоки процесса {pid}");
        }
    }
    Ok(false)
}

fn thread_references(thread: &Path, path: &Path, directories: &FileIds) -> Result<bool> {
    for name in ["cwd", "root", "exe"] {
        let link = thread.join(name);
        if let Some(target) = proc_item(fs::read_link(&link))?
            && file_reference(&link, &target, path, directories)?
        {
            return Ok(true);
        }
    }
    if let Some(fds) = proc_item(fs::read_dir(thread.join("fd")))? {
        for fd in fds {
            let link = fd?.path();
            if let Some(target) = proc_item(fs::read_link(&link))?
                && file_reference(&link, &target, path, directories)?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
