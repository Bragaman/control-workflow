use crate::{
    args::s,
    config::{Context, location},
};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use std::fs;
use std::{
    io::{Read, Seek, SeekFrom},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
/// Output is spooled to anonymous files so neither stream can fill a pipe.
pub fn output(cmd: &mut Command, timeout: Duration) -> Result<(bool, String, String)> {
    let mut out = tempfile::tempfile()?;
    let mut err = tempfile::tempfile()?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(err.try_clone()?)
        .spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Превышено время ожидания внешней команды");
        }
        thread::sleep(Duration::from_millis(2));
    };
    out.seek(SeekFrom::Start(0))?;
    err.seek(SeekFrom::Start(0))?;
    let mut stdout = String::new();
    let mut stderr = String::new();
    out.read_to_string(&mut stdout)?;
    err.read_to_string(&mut stderr)?;
    Ok((status.success(), stdout, stderr))
}
pub fn command(argv: &[&str]) -> Result<String> {
    ensure!(!argv.is_empty(), "Пустая команда");
    let (ok, out, err) = output(
        Command::new(argv[0]).args(&argv[1..]),
        Duration::from_secs(60),
    )?;
    ensure!(
        ok,
        "{}",
        if err.trim().is_empty() {
            out.trim()
        } else {
            err.trim()
        }
    );
    Ok(out)
}
pub fn detach(cmd: &mut Command, log: &Path) -> Result<u32> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    cmd.stdin(Stdio::null()).stdout(f.try_clone()?).stderr(f);
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}
pub fn error_code(error: &anyhow::Error) -> String {
    serde_json::from_str::<Value>(&error.to_string())
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or("panel_error".into())
}
pub fn pid_alive(pid: i64) -> bool {
    pid > 0
        && (unsafe { libc::kill(pid as i32, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}
#[cfg(target_os = "linux")]
pub fn identity(pid: i64) -> Option<Value> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(
        json!({"boot_id":fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim(),"start_ticks":fields.get(19)?}),
    )
}
#[cfg(target_os = "linux")]
pub fn process_running(pid: i64) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|v| {
            v.rsplit_once(')')
                .map(|(_, f)| !f.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}
#[cfg(target_os = "macos")]
#[repr(C)]
struct BsdInfo {
    fields: [u32; 12],
    comm: [u8; 16],
    name: [u8; 32],
    tail: [u32; 5],
    nice: i32,
    start_sec: u64,
    start_usec: u64,
}
#[cfg(target_os = "macos")]
fn mac_info(pid: i64) -> Option<BsdInfo> {
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            size: i32,
        ) -> i32;
    }
    let mut v = std::mem::MaybeUninit::<BsdInfo>::uninit();
    let size = std::mem::size_of::<BsdInfo>() as i32;
    if unsafe { proc_pidinfo(pid as i32, 3, 0, v.as_mut_ptr().cast(), size) } == size {
        Some(unsafe { v.assume_init() })
    } else {
        None
    }
}
#[cfg(target_os = "macos")]
pub fn identity(pid: i64) -> Option<Value> {
    let v = mac_info(pid)?;
    Some(
        json!({"boot_id":command(&["/usr/sbin/sysctl","-n","kern.boottime"]).ok()?.trim(),"start_sec":v.start_sec,"start_usec":v.start_usec}),
    )
}
#[cfg(target_os = "macos")]
pub fn process_running(pid: i64) -> bool {
    mac_info(pid).is_some_and(|v| v.fields[1] != 5)
}
pub fn launcher_alive(r: &Value) -> bool {
    let pid = r["worker_pid"].as_i64().unwrap_or(0);
    pid_alive(pid)
        && (r["worker_identity"].is_null()
            || identity(pid).is_some_and(|i| i == r["worker_identity"]))
}
pub fn request_alive(r: &Value) -> bool {
    let pid = r["pid"].as_i64().unwrap_or(0);
    pid_alive(pid)
        && process_running(pid)
        && !r["identity"].is_null()
        && identity(pid).is_some_and(|v| v == r["identity"])
}
pub fn machine_id() -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        Ok(fs::read_to_string("/etc/machine-id")?.trim().into())
    }
    #[cfg(target_os = "macos")]
    {
        let text = command(&["/usr/sbin/ioreg", "-rd1", "-c", "IOPlatformExpertDevice"])?;
        let re = regex::Regex::new(r#""IOPlatformUUID"\s*=\s*"([^"\n]+)""#)?;
        Ok(re
            .captures(&text)
            .ok_or_else(|| anyhow!("Нет machine identity"))?[1]
            .into())
    }
}
impl Context {
    pub fn require_herdr(&self) -> Result<()> {
        ensure!(
            self.session.is_some() || std::env::var("HERDR_ENV").as_deref() == Ok("1"),
            "Укажите --herdr-session NAME или запустите команду внутри herdr. Состояние задачи не меняется"
        );
        Ok(())
    }
    pub fn herdr_text(
        &self,
        args: &[&str],
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<String> {
        let selected = session.or(self.session.as_deref());
        ensure!(
            selected.is_some() || std::env::var("HERDR_ENV").as_deref() == Ok("1"),
            "Укажите --herdr-session NAME или запустите команду внутри herdr"
        );
        let mut cmd = Command::new(self.machine["herdr_executable"].as_str().unwrap_or("herdr"));
        if let Some(sel) = selected {
            cmd.args(["--session", sel]);
        }
        cmd.args(args);
        let (ok, out, err) = output(&mut cmd, timeout)?;
        ensure!(
            ok,
            "{}",
            if err.trim().is_empty() {
                out.trim()
            } else {
                err.trim()
            }
        );
        Ok(out)
    }
    pub fn herdr(&self, args: &[&str], session: Option<&str>) -> Result<Value> {
        self.herdr_timeout(args, session, Duration::from_secs(60))
    }
    pub fn herdr_timeout(
        &self,
        args: &[&str],
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value> {
        let text = self.herdr_text(args, session, timeout)?;
        // herdr 0.9.3 acknowledges these commands with exit status and no body.
        // Creation/start/probes must still return evidence; never infer it here.
        if text.trim().is_empty()
            && matches!(
                args.get(..2),
                Some(
                    ["pane", "rename"]
                        | ["pane", "run"]
                        | ["tab", "rename"]
                        | ["tab", "close"]
                        | ["notification", "show"]
                )
            )
        {
            return Ok(json!({}));
        }
        let v: Value = serde_json::from_str(&text)?;
        ensure!(v.get("error").is_none(), "{v}");
        ensure!(v["result"].is_object(), "Некорректный ответ herdr");
        Ok(v["result"].clone())
    }
    pub fn namespace(&self, selected: Option<&str>) -> Result<Value> {
        let selected = selected.or(self.session.as_deref());
        let mut cmd = Command::new(self.machine["herdr_executable"].as_str().unwrap_or("herdr"));
        if let Some(sel) = selected {
            cmd.args(["--session", sel]);
        }
        cmd.args(["session", "list", "--json"]);
        let (ok, out, err) = output(&mut cmd, Duration::from_secs(60))?;
        ensure!(ok, "{err}");
        let v: Value = serde_json::from_str(&out)?;
        let inherited = std::env::var("HERDR_SOCKET_PATH").ok();
        let matches = v["sessions"]
            .as_array()
            .ok_or_else(|| anyhow!("Некорректный список session"))?
            .iter()
            .filter(|v| {
                if let Some(sel) = selected {
                    v["name"] == sel
                } else {
                    inherited
                        .as_ref()
                        .is_some_and(|p| location(p).ok() == location(s(v, "socket_path")).ok())
                }
            })
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "Не удалось однозначно определить namespace herdr; укажите --herdr-session NAME"
        );
        let item = matches[0];
        Ok(
            json!({"session":item["name"],"socket":location(s(item,"socket_path"))?,"machine":machine_id()?,"uid":unsafe{libc::getuid()}}),
        )
    }
    pub fn check_namespace(&self, r: &Value) -> Result<()> {
        if let Some(ns) = r.get("herdr_namespace").filter(|v| !v.is_null()) {
            ensure!(
                self.namespace(ns["session"].as_str())? == *ns,
                "Namespace сохранённого запуска изменился; нужна явная сверка"
            );
        }
        Ok(())
    }
}
pub fn capabilities(adapter: &str) -> Value {
    if adapter == "herdr-claude" {
        json!({"interactive_start":true,"assignment_confirmation":true,"resume_by_id":false,"session_id_capture":false,"turn_events":false,"platforms_tested":[],"experimental":true})
    } else {
        json!({"interactive_start":true,"assignment_confirmation":true,"resume_by_id":false,"session_id_capture":false,"turn_events":false,"platforms_tested":["linux"]})
    }
}
pub fn agent_kind<'a>(c: &'a Context, r: &'a Value) -> &'static str {
    if r["adapter"].as_str().unwrap_or(s(&c.settings, "adapter")) == "herdr-claude" {
        "claude"
    } else {
        "codex"
    }
}
pub fn start_args(
    c: &Context,
    r: &mut Value,
    t: &Value,
    cwd: &Path,
    assignment: &Path,
) -> Vec<String> {
    let kind = agent_kind(c, r);
    let mut a = vec![
        "agent".into(),
        "start".into(),
        format!("cp-{}", s(r, "id").trim_start_matches("run-")),
        "--kind".into(),
        kind.into(),
        "--pane".into(),
        s(r, "pane").into(),
        "--timeout".into(),
        c.settings["agent_start_timeout_ms"].to_string(),
        "--".into(),
    ];
    if kind == "claude" {
        if r["requested_agent_session_id"].is_null() {
            r["requested_agent_session_id"] = json!(uuid::Uuid::new_v4().to_string());
        }
        a.extend([
            "--session-id".into(),
            s(r, "requested_agent_session_id").into(),
            "--permission-mode".into(),
            "default".into(),
            "--effort".into(),
            s(r, "reasoning_effort").into(),
        ]);
    } else {
        a.extend([
            "--sandbox".into(),
            "workspace-write".into(),
            "--cd".into(),
            cwd.to_string_lossy().into(),
            "--config".into(),
            format!("model_reasoning_effort={}", s(r, "reasoning_effort")),
        ]);
    }
    if r["model"] != "default" {
        a.extend(["--model".into(), s(r, "model").into()]);
    }
    let mut dirs = vec![c.root.clone()];
    if let Some(wts) = r["worktrees"].as_array() {
        dirs.extend(
            wts.iter()
                .skip(1)
                .map(|w| Path::new(s(w, "path")).to_path_buf()),
        );
    }
    dirs.dedup();
    for dir in dirs {
        if dir != cwd {
            a.extend(["--add-dir".into(), dir.to_string_lossy().into()]);
        }
    }
    a.push(format!("{}: Прочитай задание {} и выполни его. Покажи результат в чате и сохрани полезные артефакты по {}.",s(t,"id"),assignment.display(),c.program.join("docs/worker.md").display()));
    a
}
