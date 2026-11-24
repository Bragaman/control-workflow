//! Linux-only storage for disposable run data. Artifacts and Git worktrees live
//! outside this directory, and the lease survives removal of the payload.
use crate::config::Context;
use anyhow::Result;
use serde_json::Value;

#[cfg(not(target_os = "linux"))]
pub fn cleanup_task(_: &Context, _: &Value) -> Result<Value> {
    Ok(serde_json::json!({"status":"unsupported"}))
}

#[cfg(not(target_os = "linux"))]
pub fn sweep(_: &Context) -> Result<Value> {
    anyhow::bail!("temp-sweep доступен на Linux")
}

#[cfg(not(target_os = "linux"))]
pub fn stop_task(_: &Context, _: &Value) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
pub use linux::{cleanup_task, execute, stop_task, sweep};

#[cfg(target_os = "linux")]
mod inspector;
#[cfg(target_os = "linux")]
pub use inspector::inspect_entry;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{args::s, external, store};
    use anyhow::{Context as _, anyhow, bail, ensure};
    use serde_json::json;
    use std::{
        collections::HashSet,
        ffi::{CString, OsStr},
        fs::{self, DirBuilder, File, FileTimes, Metadata, OpenOptions},
        io,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{
                ffi::OsStrExt,
                fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
                process::{CommandExt, ExitStatusExt},
            },
        },
        path::{Path, PathBuf},
        process::Command,
        time::{Duration, SystemTime},
    };

    const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
    const OWNER_ENV: &str = "CONTROL_WORKFLOW_TEMP_OWNER";
    const CACHE_DIRS: [&str; 5] = ["cargo", "uv", "xdg", "ruff", "python"];

    fn uid() -> u32 {
        unsafe { libc::geteuid() }
    }

    fn missing<T>(result: io::Result<T>) -> io::Result<Option<T>> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn visible<T>(result: io::Result<T>) -> io::Result<Option<T>> {
        match missing(result) {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(None),
            result => result,
        }
    }

    fn identity(meta: &Metadata) -> Value {
        json!([meta.dev(), meta.ino()])
    }

    fn private_dir(path: &Path) -> Result<Option<Metadata>> {
        let Some(meta) = missing(fs::symlink_metadata(path))? else {
            return Ok(None);
        };
        ensure!(
            meta.is_dir() && meta.uid() == uid() && meta.mode() & 0o777 == 0o700,
            "Ожидался личный каталог Workflow: {}",
            path.display()
        );
        Ok(Some(meta))
    }

    fn mkdir(path: &Path) -> Result<()> {
        if private_dir(path)?.is_none() {
            DirBuilder::new().mode(0o700).create(path)?;
        }
        Ok(())
    }

    struct RunTemp {
        path: PathBuf,
        quarantined: PathBuf,
        record: PathBuf,
        lease: PathBuf,
    }

    impl RunTemp {
        fn new(c: &Context, run: &str) -> Result<Self> {
            let base = c.root.join("workflow-temp");
            let state = c.run_file(run)?;
            Ok(Self {
                path: base.join(run),
                quarantined: base.join(format!(".{run}.deleting")),
                record: state.with_file_name("workflow-temp.json"),
                lease: state.with_file_name("workflow-temp.lock"),
            })
        }

        fn for_saved_run(c: &Context, run: &Value) -> Result<Self> {
            let id = s(run, "id");
            let canonical = c.read(&c.run_file(id)?)?;
            ensure!(
                canonical["id"] == id && canonical["task"] == run["task"],
                "ID или задача не совпадают с записью запуска {id}"
            );
            Self::new(c, id)
        }

        fn existing(&self) -> Result<Option<(PathBuf, Value)>> {
            if private_dir(self.path.parent().unwrap())?.is_none() {
                return Ok(None);
            }
            let active = private_dir(&self.path)?;
            let quarantined = private_dir(&self.quarantined)?;
            ensure!(
                active.is_none() || quarantined.is_none(),
                "Два временных каталога одного запуска"
            );
            let Some((path, meta)) = active
                .map(|meta| (&self.path, meta))
                .or_else(|| quarantined.map(|meta| (&self.quarantined, meta)))
            else {
                return Ok(None);
            };
            self.check_ledger()?;
            let meta_record = fs::symlink_metadata(&self.record)
                .context("Нет записи владельца временных данных")?;
            ensure!(
                meta_record.is_file()
                    && meta_record.nlink() == 1
                    && meta_record.uid() == uid()
                    && meta_record.mode() & 0o777 == 0o600,
                "Некорректная запись владельца временных данных"
            );
            let record = store::raw(&self.record)?;
            ensure!(
                record["directory"] == identity(&meta)
                    && record["path"] == json!(self.path)
                    && record["uid"] == uid()
                    && record["machine"] == external::machine_id()?
                    && record["token"]
                        .as_str()
                        .is_some_and(|token| uuid::Uuid::parse_str(token).is_ok()),
                "Не подтверждён владелец временного каталога {}",
                path.display()
            );
            Ok(Some((path.clone(), record)))
        }

        fn create(&self, r: &Value) -> Result<Value> {
            self.check_ledger()?;
            let base = self.path.parent().unwrap();
            mkdir(base)?;
            let lease = self.lock_file(true)?;
            touch(&lease)?;
            let staging = tempfile::Builder::new()
                .prefix(".new-")
                .permissions(fs::Permissions::from_mode(0o700))
                .tempdir_in(base)?;
            let record = json!({
                "directory":identity(&fs::metadata(staging.path())?),
                "path":self.path,
                "lease":identity(&lease.metadata()?),
                "uid":uid(),
                "machine":external::machine_id()?,
                "pid_namespace":r["worker_identity"]["pid_namespace"],
                "token":uuid::Uuid::new_v4().simple().to_string(),
            });
            ensure!(
                record["pid_namespace"].is_string(),
                "В запуске нет подтверждённого PID namespace подготовителя"
            );
            store::save_raw(&self.record, &record)?;
            fs::rename(staging.path(), &self.path)?;
            Ok(record)
        }

        fn check_ledger(&self) -> Result<()> {
            for parent in self.record.ancestors().skip(1).take(3) {
                let meta = fs::symlink_metadata(parent)?;
                ensure!(
                    meta.is_dir() && meta.uid() == uid() && meta.mode() & 0o022 == 0,
                    "Небезопасный каталог записей запуска: {}",
                    parent.display()
                );
            }
            Ok(())
        }

        fn lock_file(&self, create: bool) -> Result<File> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(create)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&self.lease)?;
            let meta = file.metadata()?;
            ensure!(
                meta.is_file()
                    && meta.nlink() == 1
                    && meta.uid() == uid()
                    && meta.mode() & 0o777 == 0o600,
                "Некорректный файл блокировки временных данных"
            );
            Ok(file)
        }

        fn lease(&self, record: &Value) -> Result<File> {
            let file = self.lock_file(false)?;
            ensure!(
                record["lease"] == identity(&file.metadata()?),
                "Файл блокировки временных данных заменён"
            );
            Ok(file)
        }
    }

    fn touch(file: &File) -> io::Result<()> {
        file.set_times(FileTimes::new().set_modified(SystemTime::now()))
    }

    fn lock(file: &File, mode: libc::c_int) -> io::Result<bool> {
        if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(e)
        }
    }

    fn expired(last_use: SystemTime, now: SystemTime) -> bool {
        now.duration_since(last_use)
            .is_ok_and(|age| age >= RETENTION)
    }

    pub(super) fn check_visibility(namespace: &str) -> Result<()> {
        let current = fs::read_link("/proc/self/ns/pid")?;
        ensure!(
            current.as_os_str() == OsStr::new(namespace),
            "Проверку процессов выполните в PID namespace подготовителя запуска"
        );
        ensure!(
            fs::read_link("/proc/self")? == Path::new(&std::process::id().to_string()),
            "/proc не соответствует текущему PID namespace"
        );
        let mounts = fs::read_to_string("/proc/self/mountinfo")?;
        ensure!(
            !mounts
                .lines()
                .any(|line| line.split_whitespace().nth(4) == Some("/proc")
                    && line.contains("hidepid=")
                    && !line.contains("hidepid=0")),
            "/proc скрывает процессы"
        );
        Ok(())
    }

    fn unescape_path(path: &[u8]) -> PathBuf {
        let mut decoded = Vec::with_capacity(path.len());
        let mut i = 0;
        while i < path.len() {
            if path[i] == b'\\'
                && let Some(escape) = path.get(i + 1..i + 4)
                && escape[0] <= b'3'
                && escape.iter().all(|digit| (b'0'..=b'7').contains(digit))
            {
                decoded.push((escape[0] - b'0') * 64 + (escape[1] - b'0') * 8 + escape[2] - b'0');
                i += 4;
            } else {
                decoded.push(path[i]);
                i += 1;
            }
        }
        PathBuf::from(OsStr::from_bytes(&decoded))
    }

    fn check_mounts(path: &Path) -> Result<()> {
        for line in fs::read("/proc/self/mountinfo")?.split(|&b| b == b'\n') {
            if let Some(mount) = line.split(|&b| b == b' ').nth(4) {
                let mount = unescape_path(mount);
                ensure!(
                    !mount.starts_with(path) && Some(mount.as_path()) != path.parent(),
                    "Временные данные содержат отдельное монтирование: {}",
                    mount.display()
                );
            }
        }
        Ok(())
    }

    pub(super) type FileIds = HashSet<(u64, u64)>;

    fn tree_files(path: &Path, files: &mut FileIds) -> Result<()> {
        let root = fs::metadata(path)?;
        files.insert((root.dev(), root.ino()));
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            ensure!(
                meta.uid() == uid() && (meta.is_dir() || meta.is_file() || meta.is_symlink()),
                "Временные данные содержат чужой файл или специальный объект: {}",
                entry.path().display()
            );
            if meta.is_dir() {
                tree_files(&entry.path(), files)?;
            }
        }
        Ok(())
    }

    fn within(target: &Path, path: &Path) -> bool {
        let bytes = target.as_os_str().as_bytes();
        let target = bytes.strip_suffix(b" (deleted)").unwrap_or(bytes);
        Path::new(OsStr::from_bytes(target)).starts_with(path)
    }

    pub(super) fn file_reference(
        link: &Path,
        target: &Path,
        path: &Path,
        files: &FileIds,
    ) -> Result<bool> {
        Ok(within(target, path)
            || (!files.is_empty()
                && target.is_absolute()
                && missing(fs::metadata(link))?
                    .is_some_and(|meta| files.contains(&(meta.dev(), meta.ino())))))
    }

    // Kernel leases detect an open file or mapping even if /proc denies access
    // to the holder. Keep the leases until the quarantined tree is unlinked.
    struct KernelLeases {
        files: Vec<File>,
        _sigio: IgnoreSigio,
        _capacity: FileLimit,
    }

    struct FileLimit(libc::rlimit);

    impl FileLimit {
        fn raise() -> io::Result<Self> {
            let mut original = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut original) } < 0 {
                return Err(io::Error::last_os_error());
            }
            if original.rlim_cur != original.rlim_max {
                let limits = libc::rlimit {
                    rlim_cur: original.rlim_max,
                    rlim_max: original.rlim_max,
                };
                if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limits) } < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(Self(original))
        }
    }

    impl Drop for FileLimit {
        fn drop(&mut self) {
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &self.0) };
        }
    }

    struct IgnoreSigio(libc::sigaction);

    impl IgnoreSigio {
        fn new() -> io::Result<Self> {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = libc::SIG_IGN;
            if unsafe { libc::sigaction(libc::SIGIO, &action, &mut previous) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(previous))
        }
    }

    impl Drop for IgnoreSigio {
        fn drop(&mut self) {
            unsafe {
                libc::sigaction(libc::SIGIO, &self.0, std::ptr::null_mut());
            }
        }
    }

    fn kernel_leases(path: &Path) -> Result<Option<KernelLeases>> {
        let mut leases = KernelLeases {
            files: Vec::new(),
            _sigio: IgnoreSigio::new()?,
            _capacity: FileLimit::raise()?,
        };
        let mut files = FileIds::new();
        let mut dirs = vec![path.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(dir)? {
                let path = entry?.path();
                let meta = fs::symlink_metadata(&path)?;
                if meta.is_dir() {
                    dirs.push(path);
                } else if meta.is_file() && files.insert((meta.dev(), meta.ino())) {
                    match lease_file(&path, &meta)? {
                        Some(file) => leases.files.push(file),
                        None => return Ok(None),
                    }
                }
            }
        }
        for file in &leases.files {
            let current = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLEASE) };
            if current < 0 {
                return Err(io::Error::last_os_error().into());
            }
            if current != libc::F_WRLCK {
                return Ok(None);
            }
        }
        Ok(Some(leases))
    }

    fn lease_file(path: &Path, expected: &Metadata) -> Result<Option<File>> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let meta = file.metadata()?;
        ensure!(
            meta.dev() == expected.dev() && meta.ino() == expected.ino(),
            "Файл заменён при проверке использования: {}",
            path.display()
        );
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLEASE, libc::F_WRLCK) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error).with_context(|| {
                format!("Не удалось проверить файловую аренду: {}", path.display())
            });
        }
        Ok(Some(file))
    }

    struct Process {
        pid: i32,
        parent: i32,
        fd: OwnedFd,
        supervisor: bool,
        owned: bool,
        used: bool,
        unreadable: bool,
    }

    fn pidfd(pid: i32) -> io::Result<OwnedFd> {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    }

    impl Process {
        fn alive(&self) -> bool {
            let mut fd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            unsafe { libc::poll(&mut fd, 1, 0) == 0 }
        }

        fn signal(&self, signal: libc::c_int) -> io::Result<()> {
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.fd.as_raw_fd(),
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0u32,
                )
            };
            if rc < 0 {
                missing(Err::<(), _>(io::Error::last_os_error()))?;
            }
            Ok(())
        }
    }

    fn inspect(
        proc: &Path,
        pid: i32,
        path: &Path,
        owner: &[u8],
        supervisor: &Path,
        files: &FileIds,
    ) -> Result<Option<Process>> {
        let Some(meta) = missing(fs::metadata(proc))? else {
            return Ok(None);
        };
        // The 0700 managed root restricts access to its owner and privileged users.
        if meta.uid() != uid() {
            let Some(status) = missing(fs::read_to_string(proc.join("status")))? else {
                return Ok(None);
            };
            // A non-dumpable same-user process can have root-owned /proc entries.
            if !status.lines().any(|line| {
                line.strip_prefix("Uid:")
                    .is_some_and(|uids| uids.split_whitespace().any(|id| id.parse() == Ok(uid())))
            }) {
                return Ok(None);
            }
        }
        let Some(fd) = missing(pidfd(pid))? else {
            return Ok(None);
        };
        let Some(stat) = missing(fs::read_to_string(proc.join("stat")))? else {
            return Ok(None);
        };
        let (_, fields) = stat
            .rsplit_once(')')
            .ok_or_else(|| anyhow!("Формат /proc/stat"))?;
        let mut fields = fields.split_ascii_whitespace();
        if matches!(fields.next(), Some("Z" | "X")) {
            return Ok(None);
        }
        let parent = fields.next().ok_or_else(|| anyhow!("Нет PPid"))?.parse()?;
        let mut active = Process {
            pid,
            parent,
            fd,
            supervisor: false,
            owned: false,
            used: false,
            unreadable: false,
        };
        if let Some(vars) = visible(fs::read(proc.join("environ")))? {
            active.owned = vars.split(|&b| b == 0).any(|v| v == owner);
        }
        let refs = (|| -> Result<()> {
            for name in ["cwd", "root", "exe"] {
                let link = proc.join(name);
                if let Some(target) = missing(fs::read_link(&link))? {
                    active.used |= file_reference(&link, &target, path, files)?;
                }
            }
            if let Some(fds) = missing(fs::read_dir(proc.join("fd")))? {
                for entry in fds {
                    let link = entry?.path();
                    if let Some(target) = missing(fs::read_link(&link))? {
                        active.used |= file_reference(&link, &target, path, files)?;
                        active.supervisor |= target == supervisor;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = refs {
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::PermissionDenied)
            {
                active.unreadable = true;
            } else {
                return Err(error);
            }
        }
        Ok(Some(active))
    }

    fn processes(path: &Path, record: &Value, files: &FileIds) -> Result<Vec<Process>> {
        check_visibility(s(record, "pid_namespace"))?;
        let token = s(record, "token");
        let owner = format!("{OWNER_ENV}={token}");
        let supervisor = PathBuf::from(format!("/memfd:wf-{token} (deleted)"));
        let mut items = Vec::new();
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            match inspect(
                &entry.path(),
                pid,
                path,
                owner.as_bytes(),
                &supervisor,
                files,
            ) {
                Ok(Some(process)) => items.push(process),
                Ok(None) => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Не удалось проверить использование данных процессом {pid}")
                    });
                }
            }
        }
        // Confirm the parent PIDs still refer to the sampled processes after
        // collecting PPids, before extending ownership to their descendants.
        items.retain(Process::alive);
        let mut owned: HashSet<_> = items
            .iter()
            .filter(|p| p.owned || p.supervisor)
            .map(|p| p.pid)
            .collect();
        loop {
            let mut changed = false;
            for process in &items {
                if owned.contains(&process.parent) {
                    changed |= owned.insert(process.pid);
                }
            }
            if !changed {
                break;
            }
        }
        for p in &mut items {
            p.owned = owned.contains(&p.pid);
        }
        Ok(items)
    }

    fn stop_owned(path: &Path, record: &Value) -> Result<()> {
        for attempt in 0..20 {
            let processes = processes(path, record, &FileIds::new())?;
            ensure!(
                !processes
                    .iter()
                    .any(|p| p.owned && p.pid == std::process::id() as i32),
                "Вызовите complete вне команды temp-exec"
            );
            let mut children = processes
                .iter()
                .filter(|p| p.owned && !p.supervisor)
                .peekable();
            if children.peek().is_none() {
                if !processes.iter().any(|p| p.supervisor) {
                    return Ok(());
                }
            } else {
                for child in children {
                    child.signal(if attempt < 5 {
                        libc::SIGTERM
                    } else {
                        libc::SIGKILL
                    })?;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("Процессы временного каталога ещё работают")
    }

    fn in_use(c: &Context, path: &Path, record: &Value, files: &FileIds) -> Result<bool> {
        let processes = processes(path, record, files)?;
        if processes.iter().any(|p| p.owned || p.used) {
            return Ok(true);
        }
        if !c.machine["temp_inspector"].is_null() {
            return super::inspector::query(&c.machine["temp_inspector"], path, record);
        }
        if let Some(process) = processes.iter().find(|p| p.unreadable) {
            bail!(
                "Не удалось исключить использование временного каталога: /proc/{}/cwd или fd недоступны",
                process.pid
            );
        }
        super::inspector::scan_threads(path, files).context(
            "Не подтверждена видимость ссылок процессов; проверьте настройку temp_inspector",
        )
    }

    fn remove(
        c: &Context,
        temp: &RunTemp,
        terminal: bool,
        now: SystemTime,
    ) -> Result<&'static str> {
        let Some((path, record)) = temp.existing()? else {
            return Ok("absent");
        };
        let lease = temp.lease(&record)?;
        if !terminal && path == temp.path && !expired(lease.metadata()?.modified()?, now) {
            return Ok("recent");
        }
        check_mounts(&path)?;
        if terminal {
            stop_owned(&path, &record)?;
        }
        let locked = lock(&lease, libc::LOCK_EX)?;
        // The last shared holder can finish and renew the lease between the
        // fast age check and acquisition of the exclusive lock.
        if locked && !terminal && path == temp.path && !expired(lease.metadata()?.modified()?, now)
        {
            return Ok("recent");
        }
        let mut files = FileIds::new();
        if locked {
            tree_files(&path, &mut files)?;
        }
        if !locked || in_use(c, &path, &record, &files)? {
            if locked && path == temp.quarantined {
                fs::rename(&temp.quarantined, &temp.path)?;
            }
            touch(&lease)?;
            ensure!(!terminal, "Временный каталог ещё используется");
            return Ok("in_use");
        }
        if path == temp.path {
            fs::rename(&path, &temp.quarantined)?;
        }
        // Check again under the quarantine name. New opens by the original
        // path cannot race deletion, and existing descriptors follow the rename.
        let leases = (|| {
            if in_use(c, &temp.quarantined, &record, &files)? {
                return Ok(None);
            }
            kernel_leases(&temp.quarantined)
        })();
        if !matches!(leases, Ok(Some(_))) {
            fs::rename(&temp.quarantined, &temp.path)?;
            leases?;
            touch(&lease)?;
            ensure!(!terminal, "Временный каталог ещё используется");
            return Ok("in_use");
        }
        let _leases = leases?;
        ensure!(
            temp.existing()?
                .is_some_and(|(path, current)| path == temp.quarantined && current == record),
            "Временный каталог заменён перед удалением"
        );
        let activity = temp.quarantined.join(".active");
        if !terminal
            && private_dir(&activity)?.is_some()
            && fs::read_dir(&activity)?.next().transpose()?.is_some()
        {
            // A killed supervisor could not record when it stopped. The first
            // confirmed idle observation starts the retention period.
            fs::rename(&temp.quarantined, &temp.path)?;
            touch(&lease)?;
            fs::remove_dir_all(temp.path.join(".active"))?;
            return Ok("recent");
        }
        fs::remove_dir_all(&temp.quarantined)?;
        Ok("removed")
    }

    pub fn cleanup_task(c: &Context, task: &Value) -> Result<Value> {
        let mut items = Vec::new();
        for run in c.runs()?.into_iter().filter(|r| r["task"] == task["id"]) {
            let id = s(&run, "id");
            let outcome = RunTemp::for_saved_run(c, &run)
                .and_then(|temp| remove(c, &temp, true, SystemTime::now()));
            items.push(item(id, outcome));
        }
        Ok(summary(items))
    }

    pub fn stop_task(c: &Context, task: &Value) -> Result<()> {
        for run in c.runs()?.into_iter().filter(|r| r["task"] == task["id"]) {
            let id = s(&run, "id");
            let temp = RunTemp::for_saved_run(c, &run)?;
            if let Some((path, record)) = temp.existing()? {
                stop_owned(&path, &record).with_context(|| format!("Запуск {id}"))?;
            }
        }
        Ok(())
    }

    pub fn sweep(c: &Context) -> Result<Value> {
        let _guard = c.lock()?;
        let now = SystemTime::now();
        let mut items = Vec::new();
        for r in c.runs()? {
            let id = s(&r, "id");
            let outcome =
                RunTemp::for_saved_run(c, &r).and_then(|temp| remove(c, &temp, false, now));
            if !matches!(outcome, Ok("absent" | "recent")) {
                items.push(item(id, outcome));
            }
        }
        Ok(summary(items))
    }

    fn item(run: &str, outcome: Result<&str>) -> Value {
        match outcome {
            Ok(status) => json!({"run":run,"status":status}),
            Err(e) => json!({"run":run,"status":"failed","error":format!("{e:#}")}),
        }
    }

    fn summary(items: Vec<Value>) -> Value {
        let status = if items.iter().any(|item| item["status"] == "failed") {
            "failed"
        } else {
            "completed"
        };
        json!({"status":status,"items":items,"at":store::now()})
    }

    fn cache_has_contents(cache: &Path) -> Result<bool> {
        for entry in fs::read_dir(cache)? {
            let entry = entry?;
            if !CACHE_DIRS.iter().any(|name| entry.file_name() == *name)
                || private_dir(&entry.path())?.is_none()
                || fs::read_dir(entry.path())?.next().transpose()?.is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn reuse_cache(c: &Context, run: &Value, cache: &Path, slot: &str) -> Result<()> {
        let destination = private_dir(cache)?.is_some();
        if destination && cache_has_contents(cache)? {
            return Ok(());
        }

        let mut seen = HashSet::from([s(run, "id").to_owned()]);
        let mut next = run["previous_run"].as_str().map(str::to_owned);
        let (old_slot, record, _old_lease) = loop {
            let Some(previous) = next else {
                return Ok(());
            };
            ensure!(seen.insert(previous.clone()), "Цикл в истории запусков");
            let old_run = c.read(&c.run_file(&previous)?)?;
            ensure!(
                old_run["task"] == run["task"] && old_run["id"] == previous,
                "Чужой предыдущий запуск"
            );
            next = old_run["previous_run"].as_str().map(str::to_owned);
            let old = RunTemp::new(c, &previous)?;
            let Some((path, record)) = old.existing()? else {
                continue;
            };
            if path != old.path {
                continue;
            }
            check_mounts(&path)?;
            let old_cache = path.join("cache");
            let old_slot = old_cache.join(slot);
            if private_dir(&old_cache)?.is_none() || private_dir(&old_slot)?.is_none() {
                continue;
            }
            let lease = old.lease(&record)?;
            if !lock(&lease, libc::LOCK_EX)? {
                // Keep the most recent candidate intact and retry later.
                return Ok(());
            }
            if expired(lease.metadata()?.modified()?, SystemTime::now())
                || !cache_has_contents(&old_slot)?
            {
                continue;
            }
            break (old_slot, record, lease);
        };

        let mut files = FileIds::new();
        tree_files(&old_slot, &mut files)?;
        if check_visibility(s(&record, "pid_namespace")).is_ok()
            && processes(&old_slot, &record, &files)?
                .iter()
                .any(|p| p.owned || p.used)
        {
            return Ok(());
        }
        let Some(_leases) = kernel_leases(&old_slot)? else {
            return Ok(());
        };
        if destination {
            // Swap the empty scaffold back to the old run. No directory is
            // unlinked, even if a non-Workflow process holds a reference.
            let source = CString::new(old_slot.as_os_str().as_bytes())?;
            let target = CString::new(cache.as_os_str().as_bytes())?;
            if unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    libc::AT_FDCWD,
                    target.as_ptr(),
                    libc::RENAME_EXCHANGE,
                )
            } < 0
            {
                return Err(io::Error::last_os_error().into());
            }
        } else {
            fs::rename(old_slot, cache)?;
        }
        Ok(())
    }

    struct Activity {
        _marker: tempfile::NamedTempFile,
        lease: File,
    }
    impl Drop for Activity {
        fn drop(&mut self) {
            if let Err(error) = touch(&self.lease) {
                eprintln!("Время последнего использования временных данных: {error}");
            }
        }
    }

    pub fn execute(c: &Context, id: &str, slot: &str, argv: &[String]) -> Result<i32> {
        ensure!(!argv.is_empty(), "Укажите команду после --");
        let slot = store::ident(slot)?;
        let (temp, record, activity, cache, scratch, _supervisor) = {
            let _guard = c.lock()?;
            let run = c.read(&c.run_file(id)?)?;
            let task = c.task(s(&run, "task"))?;
            ensure!(
                run["id"] == id
                    && task["active_run"] == id
                    && !["done", "cancelled"].contains(&s(&run, "status"))
                    && !["done", "cancelled"].contains(&s(&task, "status")),
                "Временные данные доступны текущему незавершённому запуску"
            );
            let temp = RunTemp::new(c, id)?;
            let record = match temp.existing()? {
                Some((path, record)) => {
                    ensure!(
                        path == temp.path,
                        "Выполняется удаление временного каталога; повторите temp-sweep"
                    );
                    record
                }
                None => temp.create(&run)?,
            };
            check_mounts(&temp.path)?;
            let lease = temp.lease(&record)?;
            let cache = temp.path.join("cache").join(slot);
            let scratch = temp.path.join("scratch").join(slot);
            mkdir(cache.parent().unwrap())?;
            if lock(&lease, libc::LOCK_EX)?
                && let Err(error) = reuse_cache(c, &run, &cache, slot)
            {
                eprintln!("Кэш прежнего запуска сохранён: {error:#}");
            }
            ensure!(
                lock(&lease, libc::LOCK_SH)?,
                "Временный каталог занят очисткой"
            );
            touch(&lease)?;
            for path in [scratch.parent().unwrap(), &cache, &scratch] {
                mkdir(path)?;
            }
            for name in CACHE_DIRS {
                mkdir(&cache.join(name))?;
            }
            let markers = temp.path.join(".active");
            mkdir(&markers)?;
            let activity = Activity {
                _marker: tempfile::NamedTempFile::new_in(markers)?,
                lease,
            };
            let name = CString::new(format!("wf-{}", s(&record, "token")))?;
            let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
            if fd < 0 {
                return Err(io::Error::last_os_error().into());
            }
            let supervisor = unsafe { OwnedFd::from_raw_fd(fd) };
            (temp, record, activity, cache, scratch, supervisor)
        };
        // Detaching and changing environment do not escape the supervisor's
        // process tree. It stays alive until adopted descendants have exited.
        if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .env(OWNER_ENV, s(&record, "token"))
            .env("CONTROL_WORKFLOW_TEMP", &temp.path)
            .env("TMPDIR", &scratch)
            .env("TMP", &scratch)
            .env("TEMP", &scratch)
            .env("CARGO_TARGET_DIR", cache.join("cargo"))
            .env("UV_CACHE_DIR", cache.join("uv"))
            .env("XDG_CACHE_HOME", cache.join("xdg"))
            .env("RUFF_CACHE_DIR", cache.join("ruff"))
            .env("PYTHONPYCACHEPREFIX", cache.join("python"));
        let lease_fd = activity.lease.as_raw_fd();
        unsafe {
            cmd.pre_exec(move || {
                if libc::fcntl(lease_fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let status = cmd.status()?;
        loop {
            if unsafe { libc::waitpid(-1, std::ptr::null_mut(), 0) } >= 0 {
                continue;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                break;
            }
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        Ok(status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn expires_at_exactly_24_hours() {
            let start = SystemTime::UNIX_EPOCH + RETENTION;
            assert!(!expired(start, start + RETENTION - Duration::from_nanos(1)));
            assert!(expired(start, start + RETENTION));
            assert!(expired(start, start + RETENTION + Duration::from_nanos(1)));
            assert!(!expired(start, SystemTime::UNIX_EPOCH));
        }
    }
}
