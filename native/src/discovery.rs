use crate::herdr::{json_object, required_string, HerdrClient, MAX_RESPONSE_BYTES};
use crate::models::{Agent, Error, HerdrSession, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn discover_sessions(
    environment: &HashMap<String, String>,
    session: Option<&str>,
    socket: Option<&str>,
) -> Result<Vec<HerdrSession>> {
    if let Some(socket) = socket.filter(|socket| !socket.is_empty()) {
        return Ok(vec![HerdrSession {
            name: "socket explicite".into(),
            socket_path: HerdrClient::new(socket).socket_path,
        }]);
    }
    if let Some(socket) = environment
        .get("HERDR_SOCKET_PATH")
        .filter(|socket| !socket.is_empty())
    {
        if session.is_none() {
            return Ok(vec![HerdrSession {
                name: environment
                    .get("HERDR_SESSION")
                    .map(String::as_str)
                    .unwrap_or("session courante")
                    .into(),
                socket_path: socket.clone(),
            }]);
        }
    }
    let binary = environment
        .get("HERDR_BIN_PATH")
        .map(String::as_str)
        .unwrap_or("herdr");
    let child = Command::new(binary)
        .args(["session", "list", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            Error::new(if error.kind() == io::ErrorKind::NotFound {
                "Herdr est introuvable. Vérifiez qu'il est installé dans votre PATH."
            } else {
                "Impossible de lire les sessions Herdr. Vérifiez que Herdr fonctionne."
            })
        })?;
    let output = bounded_output(child, Duration::from_secs(5))?;
    parse_registry(&output, session)
}

pub fn load_agents(sessions: &[HerdrSession]) -> (Vec<Agent>, Vec<String>) {
    let mut agents = Vec::new();
    let mut errors = Vec::new();
    for session in sessions {
        match HerdrClient::new(&session.socket_path).list_agents() {
            Ok(mut inventory) => {
                for agent in &mut inventory {
                    agent.session_label = session.name.clone();
                }
                agents.extend(inventory);
            }
            Err(error) => errors.push(format!("{} : {error}", session.name)),
        }
    }
    (agents, errors)
}

fn registry_error() -> Error {
    Error::new("La liste des sessions Herdr est invalide.")
}
fn bounded_output(mut child: Child, timeout: Duration) -> Result<Vec<u8>> {
    let result = (|| -> Result<Vec<u8>> {
        let mut output = child.stdout.take().ok_or_else(registry_error)?;
        // SAFETY: the child stdout handle owns a live pipe descriptor.
        let flags = unsafe { libc::fcntl(output.as_raw_fd(), libc::F_GETFL) };
        // SAFETY: F_SETFL receives the same live pipe descriptor and integer flags.
        if flags < 0
            || unsafe { libc::fcntl(output.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let deadline = Instant::now() + timeout;
        let mut bytes = Vec::new();
        let mut closed = false;
        loop {
            if Instant::now() >= deadline {
                return Err(Error::new("Herdr ne répond pas. Réessayez avec Ctrl+R."));
            }
            let mut chunk = [0; 8192];
            if !closed {
                match output.read(&mut chunk) {
                    Ok(0) => closed = true,
                    Ok(count) => {
                        if bytes.len() + count > MAX_RESPONSE_BYTES {
                            return Err(registry_error());
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                        continue;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(Error::new(
                        "Impossible de lire les sessions Herdr. Vérifiez que Herdr fonctionne.",
                    ));
                }
                if closed {
                    return Ok(bytes);
                }
            }
            std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn parse_registry(raw: &[u8], session: Option<&str>) -> Result<Vec<HerdrSession>> {
    let registry: Value = serde_json::from_slice(raw).map_err(|_| registry_error())?;
    let registry = json_object(&registry).map_err(|_| registry_error())?;
    let records = registry
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(registry_error)?;
    let mut sessions = Vec::new();
    for value in records {
        let record = json_object(value).map_err(|_| registry_error())?;
        let name = required_string(record, "name").map_err(|_| registry_error())?;
        let path = required_string(record, "socket_path").map_err(|_| registry_error())?;
        let source = HerdrSession {
            name: name.into(),
            socket_path: path.into(),
        };
        if let Some(wanted) = session {
            if name == wanted {
                return Ok(vec![source]);
            }
        } else if record.get("running").and_then(Value::as_bool) == Some(true)
            || Path::new(path).exists()
        {
            sessions.push(source);
        }
    }
    if let Some(session) = session {
        return Err(Error::new(format!(
            "Session Herdr introuvable : {session}."
        )));
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn explicit_socket_and_inherited_context_do_not_launch_cli() {
        let environment = HashMap::from([
            ("HERDR_BIN_PATH".into(), "/not/a/binary".into()),
            ("HERDR_SOCKET_PATH".into(), "/tmp/current.sock".into()),
            ("HERDR_SESSION".into(), "work".into()),
        ]);
        let explicit = discover_sessions(&environment, None, Some("/tmp/chosen.sock")).unwrap();
        assert_eq!(
            explicit,
            vec![HerdrSession {
                name: "socket explicite".into(),
                socket_path: "/tmp/chosen.sock".into()
            }]
        );
        let inherited = discover_sessions(&environment, None, None).unwrap();
        assert_eq!(
            inherited,
            vec![HerdrSession {
                name: "work".into(),
                socket_path: "/tmp/current.sock".into()
            }]
        );
    }
    #[test]
    fn registry_filters_stopped_sessions_but_named_session_is_explicit() {
        let raw=serde_json::to_vec(&json!({"sessions":[{"name":"work","socket_path":"/tmp/work.sock","running":true},{"name":"stopped","socket_path":"/does/not/exist","running":false}]})).unwrap();
        assert_eq!(parse_registry(&raw, None).unwrap().len(), 1);
        assert_eq!(
            parse_registry(&raw, Some("stopped")).unwrap()[0].name,
            "stopped"
        );
        assert!(parse_registry(&raw, Some("absent"))
            .unwrap_err()
            .message
            .contains("introuvable"));
    }
    #[test]
    fn registry_rejects_malformed_or_missing_identity() {
        for raw in [
            b"invalid".as_slice(),
            b"{}",
            br#"{"sessions":{}}"#,
            br#"{"sessions":[{"name":"test","socket_path":false}]}"#,
        ] {
            assert!(parse_registry(raw, None).is_err());
        }
    }
    #[test]
    fn registry_checks_existing_paths_even_if_running_flag_is_false() {
        let path = std::env::temp_dir().join(crate::models::unique_id());
        std::fs::write(&path, []).unwrap();
        let raw = serde_json::to_vec(
            &json!({"sessions":[{"name":"work","socket_path":path,"running":false}]}),
        )
        .unwrap();
        assert_eq!(parse_registry(&raw, None).unwrap().len(), 1);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn unavailable_session_is_reported_in_inventory_errors() {
        let (agents, errors) = load_agents(&[HerdrSession {
            name: "offline".into(),
            socket_path: "/this/socket/does/not/exist".into(),
        }]);
        assert!(agents.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("offline : "));
    }
    #[test]
    fn registry_command_is_killed_at_its_deadline() {
        let start = std::time::Instant::now();
        let child = std::process::Command::new("sleep")
            .arg("2")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        assert!(bounded_output(child, std::time::Duration::from_millis(50)).is_err());
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn discovery_runs_the_cli_with_session_list_json_arguments() {
        // A checked-in executable avoids writable descriptors inherited by parallel spawns.
        let binary =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("native/tests/fixtures/herdr-sessions.sh");
        let environment = HashMap::from([(
            "HERDR_BIN_PATH".into(),
            binary.to_string_lossy().into_owned(),
        )]);
        let result = discover_sessions(&environment, Some("work"), None);
        assert_eq!(result.unwrap()[0].socket_path, "/tmp/work.sock");
    }
}
