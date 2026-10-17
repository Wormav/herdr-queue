use crate::models::{python_string_array, unique_id, Agent, Error, Result, Target};
use serde_json::{json, Map, Value};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const KNOWN_PROMPT_REJECTIONS: &[&str] = &[
    "agent_blocked",
    "agent_not_found",
    "not_found",
    "invalid_params",
    "invalid_target",
    "agent_not_ready",
    "empty_agent_prompt",
];
#[cfg(test)]
type Connector = std::sync::Arc<dyn Fn(&str, Duration) -> io::Result<UnixStream> + Send + Sync>;

#[derive(Clone)]
pub struct HerdrClient {
    pub socket_path: String,
    timeout: Duration,
    demo_target: Option<Target>,
    #[cfg(test)]
    connector: Option<Connector>,
}

impl HerdrClient {
    pub fn new(path: impl Into<String>) -> Self {
        let path = path.into();
        let expanded = if path == "~" || path.starts_with("~/") {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(path.strip_prefix("~/").unwrap_or("")))
                .unwrap_or_else(|| PathBuf::from(&path))
        } else {
            PathBuf::from(path)
        };
        let absolute = if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("/"))
                .join(expanded)
        };
        Self {
            socket_path: absolute.to_string_lossy().into_owned(),
            timeout: Duration::from_secs(5),
            demo_target: None,
            #[cfg(test)]
            connector: None,
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub fn demo(target: Target) -> Self {
        let mut client = Self::new(target.socket_path.clone());
        client.demo_target = Some(target);
        client
    }
    #[cfg(test)]
    pub(crate) fn with_connector(
        mut self,
        connector: impl Fn(&str, Duration) -> io::Result<UnixStream> + Send + Sync + 'static,
    ) -> Self {
        self.connector = Some(std::sync::Arc::new(connector));
        self
    }
    fn connect(&self, deadline: Instant) -> io::Result<UnixStream> {
        #[cfg(test)]
        if let Some(connector) = &self.connector {
            return connector(&self.socket_path, remaining(deadline)?);
        }
        connect_unix(&self.socket_path, deadline)
    }
    pub fn request(&self, method: &str, params: Value) -> Result<Value> {
        if self.demo_target.is_some() {
            return Err(Error::new(
                "Le mode démonstration ne fait aucune requête réseau.",
            ));
        }
        let request_id = unique_id();
        let mut payload =
            serde_json::to_vec(&json!({"id":request_id,"method":method,"params":params}))?;
        payload.push(b'\n');
        let is_prompt = method == "agent.prompt";
        let mut submission_started = false;
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(transport_error)?;
        let result = (|| -> Result<Value> {
            let mut connection = self.connect(deadline).map_err(|_| transport_error())?;
            // Polling preserves one deadline and drains bytes buffered after peer closure.
            connection
                .set_nonblocking(true)
                .map_err(|_| transport_error())?;
            let mut sent = 0;
            while sent < payload.len() {
                remaining(deadline).map_err(|_| transport_error())?;
                match connection.write(&payload[sent..]) {
                    Ok(0) => return Err(transport_error()),
                    Ok(count) => {
                        submission_started = true;
                        sent += count;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        wait_ready(&connection, libc::POLLOUT, deadline)
                            .map_err(|_| transport_error())?;
                    }
                    Err(_) => return Err(transport_error()),
                }
            }
            let mut response = Vec::new();
            let line_end = loop {
                remaining(deadline).map_err(|_| transport_error())?;
                let mut chunk = [0; 8192];
                let count = match connection.read(&mut chunk) {
                    Ok(0) => return Err(transport_error()),
                    Ok(count) => count,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        wait_ready(&connection, libc::POLLIN, deadline)
                            .map_err(|_| transport_error())?;
                        continue;
                    }
                    Err(_) => return Err(transport_error()),
                };
                if response.len() + count > MAX_RESPONSE_BYTES {
                    return Err(invalid_response("Réponse Herdr trop volumineuse."));
                }
                let previous = response.len();
                response.extend_from_slice(&chunk[..count]);
                if let Some(position) = chunk[..count].iter().position(|byte| *byte == b'\n') {
                    break previous + position;
                }
            };
            let decoded: Value = serde_json::from_slice(&response[..line_end])
                .map_err(|_| invalid_response("Format de réponse Herdr invalide."))?;
            let response = json_object(&decoded)?;
            if response.get("id").and_then(Value::as_str) != Some(request_id.as_str()) {
                return Err(invalid_response("Identifiant de réponse Herdr inattendu."));
            }
            if let Some(error) = response.get("error") {
                let error = json_object(error)?;
                let code = required_string(error, "code")?;
                return Err(Error {
                    message: required_string(error, "message")?.into(),
                    code: code.into(),
                    uncertain: is_prompt && !KNOWN_PROMPT_REJECTIONS.contains(&code),
                });
            }
            let result = response.get("result").unwrap_or(&Value::Null);
            json_object(result)?;
            Ok(result.clone())
        })();
        result.map_err(|mut error| {
            if is_prompt
                && submission_started
                && (error.code == "transport" || error.code == "invalid_response")
            {
                error.uncertain = true;
            }
            error
        })
    }
    fn parse_agent(&self, value: &Value) -> Result<Agent> {
        let info = json_object(value)?;
        let session = match info.get("agent_session") {
            Some(value) if !value.is_null() => {
                let reference = json_object(value)?;
                Some(python_string_array(&[
                    Some(required_string(reference, "source")?),
                    Some(required_string(reference, "kind")?),
                    Some(required_string(reference, "value")?),
                ]))
            }
            _ => None,
        };
        let cwd = first_identity_string(info, &["foreground_cwd", "cwd"])?.unwrap_or("");
        let name = first_identity_string(
            info,
            &[
                "name",
                "title",
                "terminal_title_stripped",
                "display_agent",
                "agent",
            ],
        )?
        .ok_or_else(|| invalid_response("Identité d'agent Herdr invalide."))?;
        Ok(Agent {
            target: Target {
                socket_path: self.socket_path.clone(),
                pane_id: required_string(info, "pane_id")?.into(),
                terminal_id: required_string(info, "terminal_id")?.into(),
                workspace_id: required_string(info, "workspace_id")?.into(),
                agent: required_string(info, "agent")?.into(),
                cwd: cwd.into(),
                session,
            },
            status: required_string(info, "agent_status")?.into(),
            name: name.into(),
            workspace_label: String::new(),
            tab_label: String::new(),
            session_label: String::new(),
        })
    }
    pub fn get_agent(&self, pane: &str) -> Result<Agent> {
        if let Some(target) = &self.demo_target {
            if pane != target.pane_id {
                return Err(Error::new("Agent de démonstration introuvable."));
            }
            return Ok(Agent {
                target: target.clone(),
                status: "idle".into(),
                name: "Agent de démonstration".into(),
                workspace_label: "Démonstration".into(),
                tab_label: String::new(),
                session_label: "Démo".into(),
            });
        }
        let result = self.request("agent.get", json!({"target":pane}))?;
        self.parse_agent(&result["agent"])
    }
    pub fn list_agents(&self) -> Result<Vec<Agent>> {
        if let Some(target) = &self.demo_target {
            return Ok(vec![self.get_agent(&target.pane_id)?]);
        }
        let result = self.request("session.snapshot", json!({}))?;
        let snapshot = json_object(&result["snapshot"])?;
        let values = snapshot
            .get("agents")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response("Liste d'agents Herdr invalide."))?;
        let mut labels = std::collections::HashMap::new();
        for (collection, key) in [("workspaces", "workspace_id"), ("tabs", "tab_id")] {
            let records = snapshot
                .get(collection)
                .and_then(Value::as_array)
                .ok_or_else(|| invalid_response("Contexte des agents Herdr invalide."))?;
            let mut names = std::collections::HashMap::new();
            for value in records {
                let record = json_object(value)?;
                let label = record
                    .get("label")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid_response("Nom de projet Herdr invalide."))?;
                names.insert(required_string(record, key)?, label);
            }
            labels.insert(collection, names);
        }
        let mut agents = Vec::with_capacity(values.len());
        for value in values {
            let mut agent = self.parse_agent(value)?;
            agent.workspace_label = labels["workspaces"]
                .get(agent.target.workspace_id.as_str())
                .copied()
                .unwrap_or("")
                .into();
            agent.tab_label = labels["tabs"]
                .get(required_string(json_object(value)?, "tab_id")?)
                .copied()
                .unwrap_or("")
                .into();
            agents.push(agent);
        }
        Ok(agents)
    }
    pub fn prompt(&self, pane: &str, body: &str) -> Result<()> {
        if self.demo_target.is_some() {
            self.get_agent(pane)?;
            return Ok(());
        }
        let result = self.request("agent.prompt", json!({"target":pane,"text":body}))?;
        if result["type"].as_str() != Some("agent_prompted") {
            return Err(Error {
                message: "Confirmation d'envoi Herdr inattendue.".into(),
                code: "transport".into(),
                uncertain: true,
            });
        }
        Ok(())
    }
}

fn first_identity_string<'a>(
    info: &'a Map<String, Value>,
    keys: &[&str],
) -> Result<Option<&'a str>> {
    keys.iter()
        .filter_map(|key| info.get(*key))
        .find(|value| match value {
            Value::Null => false,
            Value::Bool(value) => *value,
            Value::Number(value) => value.as_f64() != Some(0.0),
            Value::String(value) => !value.is_empty(),
            Value::Array(value) => !value.is_empty(),
            Value::Object(value) => !value.is_empty(),
        })
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| invalid_response("Identité d'agent Herdr invalide."))
        })
        .transpose()
}
pub(crate) fn json_object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid_response("Format de réponse Herdr invalide."))
}
pub(crate) fn required_string<'a>(value: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_response(format!("Champ manquant dans la réponse Herdr : {key}.")))
}
fn invalid_response(message: impl Into<String>) -> Error {
    Error {
        message: message.into(),
        code: "invalid_response".into(),
        uncertain: false,
    }
}
fn transport_error() -> Error {
    Error {
        message: "Connexion Herdr indisponible ou réponse interrompue.".into(),
        code: "transport".into(),
        uncertain: false,
    }
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Herdr n'a pas répondu à temps."))
}

fn connect_unix(path: &str, deadline: Instant) -> io::Result<UnixStream> {
    // Keep the connection phase inside the same deadline as reads and writes.
    // SAFETY: the C socket address consists only of zero-valid integer fields.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let path = path.as_bytes();
    if path.contains(&0) || path.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Chemin de socket invalide.",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(path) {
        *slot = *byte as libc::c_char;
    }
    let offset =
        std::ptr::addr_of!(address.sun_path) as usize - std::ptr::addr_of!(address) as usize;
    let length = offset + path.len() + 1;
    #[cfg(target_os = "macos")]
    {
        address.sun_len = length as u8;
    }
    // SAFETY: these constants request an unbound Unix stream without pointer arguments.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket returned a new descriptor and ownership transfers exactly once.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    // SAFETY: the stream owns this live descriptor and F_SETFD accepts an integer flag.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    stream.set_nonblocking(true)?;
    // SAFETY: the initialized address and its checked length remain live for connect.
    let connected = unsafe {
        libc::connect(
            fd,
            std::ptr::addr_of!(address).cast(),
            length as libc::socklen_t,
        )
    };
    if connected < 0 {
        let error = io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(libc::EINPROGRESS) | Some(libc::EALREADY)
        ) {
            return Err(error);
        }
        wait_ready(&stream, libc::POLLOUT, deadline)?;
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
    }
    remaining(deadline)?;
    Ok(stream)
}

fn wait_ready(stream: &UnixStream, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let timeout = remaining(deadline)?
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        let mut ready = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: poll receives one initialized descriptor owned by the live stream.
        let count = unsafe { libc::poll(&mut ready, 1, timeout) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Herdr n'a pas répondu à temps.",
            ));
        }
        return Ok(());
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    pub fn agent_json() -> Value {
        json!({"pane_id":"w1:p1","terminal_id":"term-1","workspace_id":"w1","tab_id":"tab-1","agent":"claude","agent_status":"working","cwd":"/project","name":"Créer le plugin","agent_session":{"source":"test","kind":"id","value":"séance🎯"}})
    }
    pub fn target() -> Target {
        Target {
            socket_path: "/tmp/herdr-test.sock".into(),
            pane_id: "w1:p1".into(),
            terminal_id: "term-1".into(),
            workspace_id: "w1".into(),
            agent: "claude".into(),
            cwd: "/project".into(),
            session: Some(r#"["test", "id", "s\u00e9ance\ud83c\udfaf"]"#.into()),
        }
    }
    pub fn scripted_client(
        responses: Vec<Option<Value>>,
        fragment: bool,
    ) -> (HerdrClient, Vec<JoinHandle<Value>>) {
        let mut streams = VecDeque::new();
        let mut workers = Vec::new();
        for response in responses {
            let (client, mut server) = UnixStream::pair().unwrap();
            streams.push_back(client);
            workers.push(thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(server.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                if let Some(mut response) = response {
                    if response.get("id").is_none() {
                        response["id"] = request["id"].clone();
                    }
                    let bytes = format!("{response}\n");
                    for chunk in bytes
                        .as_bytes()
                        .chunks(if fragment { 3 } else { bytes.len() })
                    {
                        if server.write_all(chunk).is_err() {
                            break;
                        }
                    }
                }
                request
            }));
        }
        let streams = Arc::new(Mutex::new(streams));
        let client = HerdrClient::new("/tmp/herdr-test.sock")
            .with_timeout(Duration::from_millis(500))
            .with_connector(move |_, _| {
                streams.lock().unwrap().pop_front().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no test connection")
                })
            });
        (client, workers)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use serde_json::json;
    #[test]
    fn fragmented_response_preserves_unicode_multiline_prompt() {
        let (client, workers) = scripted_client(
            vec![Some(json!({"result":{"type":"agent_prompted"}}))],
            true,
        );
        let body = "Vérifie 🎯\n`rm -rf /` et $(echo hello)\n\tfin";
        client.prompt("w1:p1", body).unwrap();
        let request = workers.into_iter().next().unwrap().join().unwrap();
        assert_eq!(request["method"], "agent.prompt");
        assert_eq!(request["params"], json!({"target":"w1:p1","text":body}));
    }
    #[test]
    fn parses_native_session_with_python_escaping() {
        let (client, workers) =
            scripted_client(vec![Some(json!({"result":{"agent":agent_json()}}))], false);
        let agent = client.get_agent("w1:p1").unwrap();
        assert_eq!(agent.target, target());
        assert_eq!(agent.status, "working");
        for worker in workers {
            worker.join().unwrap();
        }
    }
    #[test]
    fn snapshot_enriches_workspace_and_tab_labels() {
        let (client, workers) = scripted_client(
            vec![Some(
                json!({"result":{"snapshot":{"agents":[agent_json()],"workspaces":[{"workspace_id":"w1","label":"Projet test"}],"tabs":[{"tab_id":"tab-1","label":"Développement"}]}}}),
            )],
            false,
        );
        let agents = client.list_agents().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].workspace_label, "Projet test");
        assert_eq!(agents[0].tab_label, "Développement");
        for worker in workers {
            worker.join().unwrap();
        }
    }
    #[test]
    fn prompt_rejection_known_or_uncertain_matches_submission_contract() {
        for (code, uncertain) in [
            ("agent_blocked", false),
            ("agent_not_found", false),
            ("not_found", false),
            ("invalid_params", false),
            ("invalid_target", false),
            ("agent_not_ready", false),
            ("empty_agent_prompt", false),
            ("timeout", true),
        ] {
            let (client, workers) = scripted_client(
                vec![Some(json!({"error":{"code":code,"message":"refus"}}))],
                false,
            );
            let error = client.prompt("w1:p1", "message").unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(error.uncertain, uncertain, "{code}");
            for worker in workers {
                worker.join().unwrap();
            }
        }
    }
    #[test]
    fn lost_confirmation_is_uncertain_but_absent_socket_is_retryable() {
        let (client, workers) = scripted_client(vec![None], false);
        assert!(client.prompt("w1:p1", "message").unwrap_err().uncertain);
        assert_eq!(
            workers.into_iter().next().unwrap().join().unwrap()["params"]["text"],
            "message"
        );
        assert!(
            !HerdrClient::new("/this/socket/does/not/exist")
                .prompt("w1:p1", "message")
                .unwrap_err()
                .uncertain
        );
    }
    #[test]
    fn wrong_id_malformed_result_and_wrong_confirmation_are_uncertain_after_prompt() {
        for response in [
            json!({"id":"wrong","result":{}}),
            json!({"result":[]}),
            json!({"result":{"type":"other"}}),
        ] {
            let (client, workers) = scripted_client(vec![Some(response)], false);
            assert!(client.prompt("w1:p1", "message").unwrap_err().uncertain);
            for worker in workers {
                worker.join().unwrap();
            }
        }
    }
    #[test]
    fn demo_uses_the_given_identity_without_connecting() {
        let expected = target();
        let client = HerdrClient::demo(expected.clone());
        assert_eq!(client.get_agent("w1:p1").unwrap().target, expected);
        client.prompt("w1:p1", "message").unwrap();
        assert!(client.get_agent("other").is_err());
    }
    #[test]
    fn response_limit_rejects_oversized_json_without_accepting_prompt() {
        let (client, workers) = scripted_client(
            vec![Some(
                json!({"result":{"type":"agent_prompted","padding":"x".repeat(1024*1024)}}),
            )],
            false,
        );
        let error = client.prompt("w1:p1", "message").unwrap_err();
        assert!(error.uncertain);
        assert_eq!(error.code, "invalid_response");
        for worker in workers {
            worker.join().unwrap();
        }
    }
    #[test]
    fn fragmented_response_uses_one_deadline_for_the_entire_exchange() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        use std::time::Instant;
        let (stream, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            for _ in 0..60 {
                if server.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let stream = Arc::new(Mutex::new(Some(stream)));
        let client = HerdrClient::new("/tmp/test")
            .with_timeout(Duration::from_millis(100))
            .with_connector(move |_, _| Ok(stream.lock().unwrap().take().unwrap()));
        let start = Instant::now();
        assert!(client.prompt("w1:p1", "message").unwrap_err().uncertain);
        assert!(start.elapsed() < Duration::from_millis(500));
        worker.join().unwrap();
    }
    #[test]
    fn missing_agent_identity_never_produces_a_target() {
        let mut info = agent_json();
        info.as_object_mut().unwrap().remove("terminal_id");
        let (client, workers) =
            scripted_client(vec![Some(json!({"result":{"agent":info}}))], false);
        assert_eq!(
            client.get_agent("w1:p1").unwrap_err().code,
            "invalid_response"
        );
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn socket_paths_expand_home_and_become_absolute() {
        assert_eq!(
            HerdrClient::new("relative.sock").socket_path,
            std::env::current_dir()
                .unwrap()
                .join("relative.sock")
                .to_string_lossy()
        );
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                HerdrClient::new("~/herdr.sock").socket_path,
                PathBuf::from(home).join("herdr.sock").to_string_lossy()
            );
        }
    }
    #[test]
    fn demo_clone_keeps_network_disabled() {
        let client = HerdrClient::demo(target()).clone();
        assert_eq!(client.get_agent("w1:p1").unwrap().status, "idle");
        client.prompt("w1:p1", "message").unwrap();
        assert!(client.request("arbitrary.method", json!({})).is_err());
    }
    #[test]
    fn buffered_response_is_drained_after_the_server_closes() {
        let (client, workers) = scripted_client(
            vec![Some(json!({"result":{"padding":"x".repeat(512*1024)}}))],
            false,
        );
        let result = client.request("session.snapshot", json!({})).unwrap();
        assert_eq!(result["padding"].as_str().unwrap().len(), 512 * 1024);
        for worker in workers {
            worker.join().unwrap();
        }
    }
    #[test]
    fn blocked_write_respects_deadline_and_is_uncertain_after_partial_submission() {
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        let (stream, _server) = UnixStream::pair().unwrap();
        let stream = Arc::new(Mutex::new(Some(stream)));
        let client = HerdrClient::new("/tmp/test")
            .with_timeout(Duration::from_millis(80))
            .with_connector(move |_, _| Ok(stream.lock().unwrap().take().unwrap()));
        let start = Instant::now();
        assert!(
            client
                .prompt("w1:p1", &"x".repeat(2 * 1024 * 1024))
                .unwrap_err()
                .uncertain
        );
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}
