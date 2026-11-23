//! Local core on the machine that owns the Workflow state and Git worktrees.
//! Desktop and SSH viewers attach to this core, never spawn another worker.
use crate::{args::s, config::Context, external};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

pub const ADAPTER: &str = "hc-codex";

pub fn enabled(record: &Value) -> bool {
    record["adapter"] == ADAPTER
}

pub(crate) fn executable(c: &Context) -> &str {
    c.machine["hc_executable"].as_str().unwrap_or("hc")
}

fn command(c: &Context, session: &str) -> Command {
    let mut cmd = Command::new(executable(c));
    cmd.env("HC_SESSION", session)
        .env_remove("HC_SOCKET")
        .env_remove("HC_TERMINAL_ID")
        .env_remove("HC_TERMINAL_SOCKET");
    cmd
}

fn identity(session: &str, status: &Value) -> Result<Value> {
    ensure!(!s(&status["core"], "id").is_empty(), "hc не вернул core.id");
    Ok(
        json!({"transport":"hc","session":session,"core":status["core"]["id"],
        "machine":external::machine_id()?,"uid":unsafe{libc::getuid()}}),
    )
}

pub fn namespace(c: &Context, session: &str) -> Result<Value> {
    let (ok, out, err) = external::output(
        command(c, session).args(["--json", "server", "start"]),
        Duration::from_secs(30),
    )?;
    ensure!(ok, "Не удалось открыть hc: {err}{out}");
    identity(session, &serde_json::from_str(&out)?)
}

// Bind identity checks, reads and mutations to one peer. A restarted core
// closes this stream; it cannot receive a kill meant for its predecessor.
struct Connection {
    child: Child,
    stream: BufReader<UnixStream>,
    serial: u32,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Connection {
    fn open(c: &Context, r: &Value) -> Result<Self> {
        let session = s(r, "herdr_session");
        let ns = &r["herdr_namespace"];
        ensure!(
            ns["machine"] == external::machine_id()?
                && ns["session"] == session
                && ns["uid"] == unsafe { libc::getuid() },
            "Запуск hc принадлежит другой машине или сессии"
        );
        let (stream, peer) = UnixStream::pair()?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let child = command(c, session)
            .args(["connect", "--session", session])
            .stdin(OwnedFd::from(peer.try_clone()?))
            .stdout(OwnedFd::from(peer))
            .stderr(Stdio::null())
            .spawn()?;
        let mut conn = Self {
            child,
            stream: BufReader::new(stream),
            serial: 0,
        };
        let status = conn.call("server.status", json!({}))?;
        ensure!(
            identity(session, &status)? == *ns,
            "Изменилась идентичность core запуска"
        );
        Ok(conn)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.serial += 1;
        writeln!(
            self.stream.get_mut(),
            "{}",
            json!({"jsonrpc":"2.0","id":self.serial,"method":method,"params":params})
        )?;
        let mut line = String::new();
        ensure!(
            self.stream.read_line(&mut line)? > 0,
            "hc закрыл соединение во время {method}"
        );
        let v: Value = serde_json::from_str(&line)?;
        ensure!(
            v["id"] == self.serial && v["jsonrpc"] == "2.0",
            "Некорректный ответ hc на {method}"
        );
        Ok(v)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let v = self.request(method, params)?;
        ensure!(v["result"].is_object(), "hc {method}: {}", v["error"]);
        Ok(v["result"].clone())
    }
}

pub fn check(c: &Context, r: &Value) -> Result<()> {
    Connection::open(c, r).map(drop)
}

// A name is unique among live terminals. Before an address is known, a
// missing name after a lost reply cannot prove that no spawn is in flight.
pub fn terminal(c: &Context, r: &Value) -> Result<Option<Value>> {
    Connection::open(c, r)?.terminal(r)
}

impl Connection {
    fn terminal(&mut self, r: &Value) -> Result<Option<Value>> {
        if r["agent_start_intent"] != true {
            ensure!(r["pane"].is_null(), "Нет намерения запуска hc");
            return Ok(None);
        }
        let name = s(r, "hc_name");
        let selector = r["pane"].as_str().unwrap_or(name);
        ensure!(
            !name.is_empty() && !selector.is_empty(),
            "Нет адреса запуска hc"
        );
        let reply = self.request("terminal.get", json!({"terminal":selector}))?;
        if reply["error"]["data"]["kind"] == "not_found" && r["hc_created_ms"].is_u64() {
            return Ok(None);
        }
        ensure!(
            reply["result"].is_object(),
            "hc terminal.get: {}",
            reply["error"]
        );
        let v = &reply["result"];
        ensure!(
            v["name"] == name
                && v["labels"]["workflow.run"] == r["id"]
                && v["labels"]["workflow.owner"].is_string()
                && v["labels"]["workflow.owner"] == r["hc_owner"]
                && v["created_ms"].is_u64()
                && (r["hc_created_ms"].is_null() || v["created_ms"] == r["hc_created_ms"])
                && v["address"]
                    .as_str()
                    .is_some_and(|a| !a.is_empty() && !a.contains('/'))
                && (r["pane"].is_null() || v["address"] == r["pane"]),
            "Идентичность терминала hc не совпадает с записью запуска"
        );
        match v["state"].as_str() {
            Some("finished") => Ok(None),
            Some("live") => Ok(Some(v.clone())),
            _ => bail!("Неизвестное состояние терминала hc"),
        }
    }
}

pub fn confirm(r: &mut Value, v: &Value) -> Result<()> {
    ensure!(
        r["launch_argv"].is_array() && v["argv"] == r["launch_argv"],
        "В сохранённом терминале hc другой процесс, проверьте восстановленный shell"
    );
    r["pane"] = v["address"].clone();
    r["terminal_id"] = v["address"].clone();
    r["hc_created_ms"] = v["created_ms"].clone();
    Ok(())
}

pub fn launch(c: &Context, id: &str, cwd: &Path, t: &mut Value, r: &mut Value) -> Result<Value> {
    let _lock = c.lock()?;
    *t = c.task(s(r, "task"))?;
    *r = c.current(t, id)?;
    ensure!(
        r["status"] == "launching" && r["agent_start_intent"].is_null(),
        "Старт hc уже начат или остановлен"
    );
    let mut conn = Connection::open(c, r)?;
    let assignment = c.run_file(id)?.with_file_name("assignment.md");
    crate::store::atomic(
        &assignment,
        crate::lifecycle::assignment_text(c, r, t)?.as_bytes(),
    )?;
    // The core can have been started inside an unrelated legacy pane. Its
    // inherited integration hooks must never report this worker to that pane.
    let mut argv: Vec<String> = [
        "env",
        "-u",
        "HERDR_ENV",
        "-u",
        "HERDR_SOCKET_PATH",
        "-u",
        "HERDR_PANE_ID",
        "-u",
        "HERDR_TAB_ID",
        "-u",
        "HERDR_WORKSPACE_ID",
        "-u",
        "CODEX_THREAD_ID",
        "codex",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    argv.extend(external::agent_args(c, r, t, cwd, &assignment));
    r["cwd"] = json!(cwd);
    r["launch_argv"] = json!(argv);
    r["agent_start_intent"] = json!(true);
    r["hc_owner"] = json!(uuid::Uuid::new_v4().simple().to_string());
    r["hc_name"] = json!(format!("cp-{}", id.trim_start_matches("run-")));
    r["workspace"] = json!(format!("workflow-{}", s(t, "project")));
    r["tab"] = json!(format!("workflow-{id}"));
    c.update(r, t)?;
    let started = conn.call("terminal.run",json!({"argv":argv,"name":r["hc_name"],"cwd":r["cwd"],"keep":true,"no_parent":true,
        "labels":{"herdr.space":r["workspace"],"herdr.tab":r["tab"],"workflow.run":r["id"],"workflow.owner":r["hc_owner"]}}));
    // On lost acknowledgement reconnect to the same verified core and look up
    // the journalled name. Never submit terminal.run again for this intent.
    let observation = if started.is_err() {
        terminal(c, r)
    } else {
        conn.terminal(r)
    };
    if let Err(e) = started {
        r["startup_error"] = json!(e.to_string());
        c.update(r, t)?;
    }
    let v = observation?.ok_or_else(|| anyhow!("Исполнитель hc завершился при старте"))?;
    confirm(r, &v)?;
    r["agent_start_completed"] = json!(true);
    r["status"] = json!("running");
    c.update(r, t)?;
    Ok(json!({"run":id,"status":"running"}))
}

pub fn client(c: &Context, r: &Value, v: Option<&Value>) -> Value {
    let address = v.map(|v| &v["address"]).unwrap_or(&r["pane"]);
    let mut result = json!({"machine":r["herdr_namespace"]["machine"],"core_id":r["herdr_namespace"]["core"],
        "session":r["herdr_session"],"address":address,"space":r["workspace"],"tab":r["tab"],
        "terminal_state":v.map(|t| &t["state"]),"agent_status":v.map(|t| &t["status"])});
    if let Some(terminal) = v.filter(|v| v["argv"] == r["launch_argv"]) {
        // Local IDs repeat across cores, even with the same HC_SESSION.
        // Let hc resolve the owning core and terminal in the attach request.
        let target = format!(
            "{}/{}",
            s(&r["herdr_namespace"], "core"),
            s(terminal, "address")
        );
        result["attach"] = json!({"env":{"HC_SESSION":r["herdr_session"]},
            "unset_env":["HC_SOCKET","HC_TERMINAL_SOCKET","HC_TERMINAL_ID"],"argv":[executable(c),"attach",target]});
    }
    result
}

/// Reading history still works when the machine is disconnected.
pub fn inspect(c: &Context, r: &Value) -> Value {
    if ["closed", "no_tab"].contains(&s(&r["cleanup"], "status")) {
        let mut info = client(c, r, None);
        info["terminal_state"] = json!("closed");
        return info;
    }
    match terminal(c, r) {
        Ok(Some(v)) => client(c, r, Some(&v)),
        Ok(None) => {
            let mut info = client(c, r, None);
            info["terminal_state"] = json!(if r["agent_start_intent"] == true {
                "absent"
            } else {
                "pending"
            });
            info
        }
        Err(e) => {
            let mut info = client(c, r, None);
            info["error"] = json!(e.to_string());
            info["terminal_state"] = json!("unknown");
            info
        }
    }
}

pub fn closing_terminal(c: &Context, r: &Value) -> Result<Option<Value>> {
    Connection::open(c, r)?.closing_terminal(r)
}

impl Connection {
    fn closing_terminal(&mut self, r: &Value) -> Result<Option<Value>> {
        let Some(v) = self.terminal(r)? else {
            return Ok(None);
        };
        ensure!(
            v["argv"] == r["launch_argv"],
            "В терминале hc сменился процесс; проверьте его перед закрытием"
        );
        let address = &v["address"];
        let listed = self.call("terminal.list", json!({}))?;
        let members = listed["terminals"]
            .as_array()
            .ok_or_else(|| anyhow!("hc не вернул список терминалов"))?;
        ensure!(
            !members
                .iter()
                .any(|t| t["parent"] == *address && t["die_with_parent"] != false),
            "У исполнителя есть зависимые терминалы hc; проверьте их перед закрытием"
        );
        Ok(Some(v))
    }
}

pub fn close(c: &Context, r: &Value) -> Result<()> {
    let mut conn = Connection::open(c, r)?;
    if let Some(v) = conn.closing_terminal(r)? {
        let address = &v["address"];
        let killed = conn.call("terminal.kill", json!({"terminal":address}))?;
        ensure!(
            killed["address"] == *address
                && killed["state"] == "finished"
                && killed["labels"]["workflow.owner"] == r["hc_owner"],
            "hc не подтвердил завершение терминала"
        );
    }
    Ok(())
}
