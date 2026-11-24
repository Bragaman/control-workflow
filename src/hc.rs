//! Local core on the machine that owns the Workflow state and Git worktrees.
//! Desktop and SSH viewers attach to this core, never spawn another worker.
use crate::{args::s, config::Context, external};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    io::{self, BufRead, BufReader, Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

pub const ADAPTER: &str = "hc-codex";

#[derive(Debug)]
pub(crate) enum ConnectionUnavailable {
    AccessDenied(String),
    Unavailable(String),
}

impl ConnectionUnavailable {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::AccessDenied(_) => "hc_access_denied",
            Self::Unavailable(_) => "hc_unavailable",
        }
    }
}

impl fmt::Display for ConnectionUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessDenied(message) => {
                write!(
                    f,
                    "Нет доступа к локальному hc (сокет или блокировка): {message}"
                )
            }
            Self::Unavailable(message) => write!(f, "Не удалось проверить локальный hc: {message}"),
        }
    }
}

impl std::error::Error for ConnectionUnavailable {}

const ACCESS_ERRNOS: [i32; 3] = [libc::EACCES, libc::EPERM, libc::EROFS];

fn access_error(error: io::Error) -> anyhow::Error {
    if error
        .raw_os_error()
        .is_some_and(|n| ACCESS_ERRNOS.contains(&n))
    {
        return ConnectionUnavailable::AccessDenied(error.to_string()).into();
    }
    error.into()
}

fn reported_error(output: &str) -> Option<anyhow::Error> {
    let response: Value = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())?;
    let error = &response["error"];
    let message = error["message"].as_str()?;
    // hc reports socket and core-lock failures as "unavailable" with a Rust
    // OS error suffix. Other unavailable errors include restarts and timeouts.
    if error["kind"] == "unavailable" {
        let unavailable = if ACCESS_ERRNOS
            .iter()
            .any(|n| message.ends_with(&format!("(os error {n})")))
        {
            ConnectionUnavailable::AccessDenied(message.to_owned())
        } else {
            ConnectionUnavailable::Unavailable(message.to_owned())
        };
        return Some(unavailable.into());
    }
    Some(anyhow!("Не удалось связаться с hc: {message}"))
}

pub(crate) fn retry(c: &Context, operation: &str) -> String {
    let entry = crate::lifecycle::entry_command(c);
    format!(
        "Повторите `{entry} {operation}` в обычной консоли на машине исполнителя под тем же пользователем с доступом к hc"
    )
}

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
    if !ok {
        return Err(reported_error(&err)
            .or_else(|| reported_error(&out))
            .unwrap_or_else(|| anyhow!("Не удалось открыть hc: {err}{out}")));
    }
    let ns = identity(session, &serde_json::from_str(&out)?)?;
    // Starting the core and reaching it over the worker's transport are
    // separate checks. Prove access before the caller records a new run.
    drop(Connection::connect(c, session, &ns)?);
    Ok(ns)
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

fn capture_stderr(child: &mut Child) -> io::Result<mpsc::Receiver<Vec<u8>>> {
    let mut stderr = child.stderr.take().expect("hc connect stderr is piped");
    let (send, receive) = mpsc::sync_channel(1);
    // Keep a bounded diagnostic prefix and drain any further output so the
    // child can finish even with a noisy wrapper or an unusable TMPDIR.
    thread::Builder::new().spawn(move || {
        let mut prefix = Vec::new();
        let _ = stderr.by_ref().take(4096).read_to_end(&mut prefix);
        let _ = send.send(prefix);
        let _ = io::copy(&mut stderr, &mut io::sink());
    })?;
    Ok(receive)
}

impl Connection {
    fn open(c: &Context, r: &Value) -> Result<Self> {
        Self::connect(c, s(r, "herdr_session"), &r["herdr_namespace"])
    }

    fn connect(c: &Context, session: &str, ns: &Value) -> Result<Self> {
        ensure!(
            ns["machine"] == external::machine_id()?
                && ns["session"] == session
                && ns["uid"] == unsafe { libc::getuid() },
            "Запуск hc принадлежит другой машине или сессии"
        );
        let (stream, peer) = UnixStream::pair().map_err(access_error)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(access_error)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(access_error)?;
        let child = command(c, session)
            .args(["connect", "--session", session])
            .stdin(OwnedFd::from(peer.try_clone()?))
            .stdout(OwnedFd::from(peer))
            .stderr(Stdio::piped())
            .spawn()?;
        let mut conn = Self {
            child,
            stream: BufReader::new(stream),
            serial: 0,
        };
        let diagnostic = capture_stderr(&mut conn.child)?;
        let status = match conn.call("server.status", json!({})) {
            Ok(status) => status,
            Err(error) => {
                drop(conn);
                if error.is::<ConnectionUnavailable>() {
                    return Err(error);
                }
                return Err(diagnostic
                    .recv_timeout(Duration::from_millis(150))
                    .ok()
                    .and_then(|bytes| reported_error(&String::from_utf8_lossy(&bytes)))
                    .unwrap_or_else(|| {
                        ConnectionUnavailable::Unavailable(error.to_string()).into()
                    }));
            }
        };
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
        )
        .map_err(access_error)?;
        let mut line = String::new();
        ensure!(
            self.stream.read_line(&mut line).map_err(access_error)? > 0,
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

fn set_layout(r: &mut Value, t: &Value) {
    let repo = s(&r["worktrees"][0], "repository");
    let (space, name, folder) = if repo.is_empty() {
        let project = s(r, "project");
        (
            format!("workflow-{project}"),
            project.to_owned(),
            r["cwd"].clone(),
        )
    } else {
        // Worktree preparation has already resolved the repository path.
        let hash = format!("{:x}", Sha256::digest(repo.as_bytes()));
        let basename = Path::new(repo)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repo");
        let basename = basename.replace(char::is_control, " ");
        (
            format!("repo-{basename}-{}", &hash[..16]),
            format!("{basename} · {}", &hash[..8]),
            json!(repo),
        )
    };
    let title = s(t, "title").trim();
    let tab_name = if title.is_empty() { s(t, "id") } else { title };
    r["workspace"] = json!(space);
    r["tab"] = json!(format!("task-{}-{}", s(r, "task"), s(r, "id")));
    r["hc_desktop"] = json!({"space_name":name,"tab_name":tab_name,"folder":folder});
}

fn tree_has_terminal(node: &Value, address: &Value) -> bool {
    node["content"]["terminal"] == *address
        || ["children", "floating"].iter().any(|key| {
            node[*key]
                .as_array()
                .is_some_and(|children| children.iter().any(|n| tree_has_terminal(n, address)))
        })
}

// Desktop adopts the labels as IDs and gives an adopted tab an automatic
// caption. Set its name in the same machine-owned document Desktop uses.
fn place_in_desktop(document: &mut Value, r: &Value) -> Result<bool> {
    if document.is_null() {
        *document = json!({"spaces":[]});
    }
    let spaces = document
        .get_mut("spaces")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("Неизвестный формат раскладки Herdr Desktop"))?;
    let tab_name = r["hc_desktop"]["tab_name"].as_str().unwrap_or(s(r, "task"));
    let mut target = None;
    for space in spaces.iter_mut() {
        let wanted_space = space["id"] == r["workspace"];
        let tabs = space
            .get_mut("tabs")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| anyhow!("Нет списка вкладок в раскладке Herdr Desktop"))?;
        for tab in tabs {
            if tree_has_terminal(&tab["root"], &r["pane"]) {
                // Desktop could have adopted the labels already, or someone
                // moved or named the tab. Respect existing placement and names.
                if wanted_space && tab["id"] == r["tab"] && s(tab, "name").is_empty() {
                    tab["name"] = json!(tab_name);
                    if s(space, "name").is_empty() {
                        space["name"] = r["hc_desktop"]["space_name"].clone();
                    }
                    return Ok(true);
                }
                return Ok(false);
            }
            ensure!(
                tab["id"] != r["tab"],
                "Вкладка Herdr Desktop занята другим терминалом"
            );
        }
        if wanted_space {
            ensure!(target.is_none(), "Пространство Herdr Desktop неоднозначно");
            target = Some(space);
        }
    }
    let tab = json!({"id":r["tab"],"name":tab_name,"root":desktop_tab_root(r)});
    if let Some(space) = target {
        space["tabs"].as_array_mut().unwrap().push(tab);
        if s(space, "name").is_empty() {
            space["name"] = r["hc_desktop"]["space_name"].clone();
        }
    } else {
        spaces.push(desktop_space(r, vec![tab]));
    }
    Ok(true)
}

fn desktop_space(r: &Value, tabs: Vec<Value>) -> Value {
    json!({"id":r["workspace"],"name":r["hc_desktop"]["space_name"],
        "folder":r["hc_desktop"]["folder"],"tabs":tabs})
}

fn desktop_tab_root(r: &Value) -> Value {
    json!({"id":r["tab"],"type":"row","children":[{
        "id":format!("pane-{}", s(r, "id")), "type":"block",
        "content":{"terminal":r["pane"]}
    }]})
}

fn remove_from_desktop(document: &mut Value, r: &Value) -> Result<bool> {
    if document.is_null() {
        return Ok(false);
    }
    let spaces = document
        .get_mut("spaces")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("Неизвестный формат раскладки Herdr Desktop"))?;
    let Some(index) = spaces
        .iter()
        .position(|space| space["id"] == r["workspace"])
    else {
        return Ok(false);
    };
    let space = &mut spaces[index];
    let tabs = space
        .get_mut("tabs")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("Нет списка вкладок в раскладке Herdr Desktop"))?;
    let owned_root = desktop_tab_root(r);
    // Clean up the pane Workflow inserted. Desktop handles its own panes and
    // layouts the viewer has since rearranged or split.
    let Some(index_tab) = tabs
        .iter()
        .position(|tab| tab["id"] == r["tab"] && tab["root"] == owned_root)
    else {
        return Ok(false);
    };
    tabs.remove(index_tab);
    if *space == desktop_space(r, Vec::new()) {
        spaces.remove(index);
    }
    Ok(true)
}

impl Connection {
    fn name_desktop_tab(&mut self, r: &mut Value, terminal: &Value) {
        // Only new records opt in. A terminal whose labels were changed by the
        // viewer is already under its control.
        if r["hc_desktop"].is_null() {
            return;
        }
        let result = if terminal["labels"]["herdr.space"] == r["workspace"]
            && terminal["labels"]["herdr.tab"] == r["tab"]
        {
            self.update_desktop_document(r, place_in_desktop)
        } else {
            Ok(())
        };
        r["hc_desktop"]["error"] = json!(result.err().map(|e| e.to_string()));
    }

    fn update_desktop_document(
        &mut self,
        r: &Value,
        edit: fn(&mut Value, &Value) -> Result<bool>,
    ) -> Result<()> {
        // Bound layout operations under the Workflow state lock. This
        // connection is discarded after updating the document.
        let stream = self.stream.get_mut();
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        for _ in 0..3 {
            let mut doc = self.call("doc.get", json!({"name":"herdr-app"}))?;
            ensure!(
                doc["version"].is_u64() && doc.get("value").is_some(),
                "Некорректный ответ hc doc.get"
            );
            if !edit(&mut doc["value"], r)? {
                return Ok(());
            }
            let reply = self.request(
                "doc.set",
                json!({"name":"herdr-app", "version":doc["version"], "value":doc["value"]}),
            )?;
            if reply["error"]["data"]["kind"] == "conflict" {
                continue;
            }
            ensure!(
                reply["result"]["version"].is_u64(),
                "hc doc.set: {}",
                reply["error"]
            );
            return Ok(());
        }
        bail!("Раскладка Herdr Desktop менялась во время записи")
    }
}

pub fn recover(c: &Context, r: &mut Value) -> Result<()> {
    let mut conn = Connection::open(c, r)?;
    let v = conn
        .terminal(r)?
        .ok_or_else(|| anyhow!("Терминал hc отсутствует"))?;
    confirm(r, &v)?;
    conn.name_desktop_tab(r, &v);
    Ok(())
}

pub fn retry_desktop(c: &Context, r: &mut Value) -> bool {
    if !enabled(r) || !r["hc_desktop"]["error"].is_string() {
        return false;
    }
    let previous = r["hc_desktop"]["error"].clone();
    if let Err(error) = recover(c, r) {
        r["hc_desktop"]["error"] = json!(error.to_string());
    }
    r["hc_desktop"]["error"] != previous
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
    set_layout(r, t);
    c.update(r, t)?;
    let started = conn.call("terminal.run",json!({"argv":argv,"name":r["hc_name"],"cwd":r["cwd"],"keep":true,"no_parent":true,
        "labels":{"herdr.space":r["workspace"],"herdr.tab":r["tab"],"workflow.run":r["id"],"workflow.owner":r["hc_owner"]}}));
    // On lost acknowledgement reconnect to the same verified core and look up
    // the journalled name. Never submit terminal.run again for this intent.
    if let Err(e) = started {
        r["startup_error"] = json!(e.to_string());
        c.update(r, t)?;
        conn = Connection::open(c, r)?;
    }
    let v = conn
        .terminal(r)?
        .ok_or_else(|| anyhow!("Исполнитель hc завершился при старте"))?;
    confirm(r, &v)?;
    conn.name_desktop_tab(r, &v);
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
    if let Some(error) = r["hc_desktop"]["error"].as_str() {
        result["desktop_layout_error"] = json!(error);
    }
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
            if let Some(unavailable) = e.downcast_ref::<ConnectionUnavailable>() {
                info["error_code"] = json!(unavailable.code());
                info["next_action"] = json!(retry(c, &format!("run {} --compact", s(r, "id"))));
            }
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

pub fn close(c: &Context, r: &mut Value) -> Result<()> {
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
    if desktop_cleanup_pending(r) {
        let result = conn.update_desktop_document(r, remove_from_desktop);
        save_desktop_cleanup(r, result);
    }
    Ok(())
}

fn desktop_cleanup_pending(r: &Value) -> bool {
    enabled(r)
        && r["hc_desktop"].is_object()
        && r["pane"].is_string()
        && r["hc_created_ms"].is_u64()
        && r["cleanup"]["desktop_layout_done"] != true
}

fn save_desktop_cleanup(r: &mut Value, result: Result<()>) {
    r["cleanup"]["desktop_layout_done"] = json!(result.is_ok());
    r["cleanup"]["desktop_layout_error"] = json!(result.err().map(|e| e.to_string()));
}

pub fn retry_desktop_cleanup(c: &Context, r: &mut Value) -> bool {
    if !desktop_cleanup_pending(r) {
        return false;
    }
    let result = Connection::open(c, r)
        .and_then(|mut conn| conn.update_desktop_document(r, remove_from_desktop));
    save_desktop_cleanup(r, result);
    true
}
