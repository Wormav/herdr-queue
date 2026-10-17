use crate::herdr::HerdrClient;
use crate::models::{unique_id, Agent, Error, Result, Target};
use crate::service::DraftService;
use crate::store::{DraftStore, PanelLock};
use crate::ui;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Connect {
        session: Option<String>,
        socket: Option<String>,
    },
    Open,
    Tui,
    Demo,
    Doctor,
    Version,
    Help,
}

pub fn parse_args(arguments: &[String]) -> Result<Command> {
    let mut name = None;
    let mut session = None;
    let mut socket = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-V" => return Ok(Command::Version),
            "--session" | "--socket" => {
                let value = arguments
                    .next()
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| Error::new(format!("Valeur manquante pour {argument}.")))?;
                let option = if argument == "--session" {
                    &mut session
                } else {
                    &mut socket
                };
                if option.replace(value.clone()).is_some() {
                    return Err(Error::new(format!("Option répétée : {argument}.")));
                }
            }
            "connect" | "open" | "tui" | "demo" | "doctor" if name.is_none() => {
                name = Some(argument.as_str())
            }
            _ => {
                return Err(Error::new(format!(
                    "Argument inconnu : {argument}. Utilisez --help."
                )))
            }
        }
    }
    let name = name.unwrap_or("connect");
    if (session.is_some() && socket.is_some())
        || (name != "connect" && (session.is_some() || socket.is_some()))
    {
        return Err(Error::new(
            "Utilisez --session ou --socket uniquement avec connect.",
        ));
    }
    Ok(match name {
        "open" => Command::Open,
        "tui" => Command::Tui,
        "demo" => Command::Demo,
        "doctor" => Command::Doctor,
        _ => Command::Connect { session, socket },
    })
}

pub fn source_pane(environment: &HashMap<String, String>) -> Result<String> {
    if environment.get("HERDR_ENV").map(String::as_str) != Some("1") {
        return Err(Error::new(
            "Ouvrez ce panneau depuis un agent dans Herdr, ou utilisez « demo ».",
        ));
    }
    if let Some(raw) = environment
        .get("HERDR_PLUGIN_CONTEXT_JSON")
        .filter(|raw| !raw.is_empty())
    {
        let context: Value = serde_json::from_str(raw)?;
        if !context.is_object() {
            return Err(Error::new("Contexte du plugin invalide."));
        }
        if let Some(pane) = context
            .get("focused_pane_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return Ok(pane.to_owned());
        }
    }
    environment
        .get("HERDR_PANE_ID")
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| Error::new("Le panneau source Herdr n'est pas identifié."))
}

fn expand_home(path: &str, environment: &HashMap<String, String>) -> PathBuf {
    let home = environment
        .get("HOME")
        .cloned()
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".into());
    if path == "~" {
        PathBuf::from(home)
    } else if let Some(relative) = path.strip_prefix("~/") {
        PathBuf::from(home).join(relative)
    } else {
        PathBuf::from(path)
    }
}

pub fn state_directory(environment: &HashMap<String, String>) -> PathBuf {
    if let Some(directory) = environment
        .get("HERDR_PLUGIN_STATE_DIR")
        .filter(|value| !value.is_empty())
    {
        return expand_home(directory, environment);
    }
    let base = environment
        .get("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(|path| expand_home(path, environment))
        .unwrap_or_else(|| expand_home("~/.local/state", environment));
    base.join("herdr/plugins/herdr-drafts")
}

pub fn open_panel(client: &HerdrClient, source: &str) -> Result<String> {
    let info = client.request("pane.get", json!({"pane_id": source}))?;
    let pane = info
        .get("pane")
        .filter(|value| value.is_object())
        .ok_or_else(|| Error::new("Panneau Herdr invalide."))?;
    if pane["tokens"]["drafts_owner"]
        .as_str()
        .is_some_and(|value| !value.is_empty())
    {
        client.request("plugin.pane.focus", json!({"pane_id": source}))?;
        return Ok(source.to_owned());
    }
    let target = client.get_agent(source)?.target;
    let result = client.request("pane.list", json!({"workspace_id": target.workspace_id}))?;
    let panes = result
        .get("panes")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("Liste de panneaux Herdr invalide."))?;
    for pane in panes {
        if pane["tokens"]["drafts_owner"].as_str() == Some(&target.lock_key()) {
            if pane["tokens"]["drafts_target"].as_str() != Some(&target.key()) {
                return Err(Error::new("Un panneau est lié à une autre conversation. Fermez-le, puis rouvrez les brouillons."));
            }
            let id = pane
                .get("pane_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| Error::new("Identifiant de panneau manquant."))?;
            client.request("plugin.pane.focus", json!({"pane_id": id}))?;
            return Ok(id.to_owned());
        }
    }
    let result = client.request(
        "plugin.pane.open",
        json!({
            "plugin_id": "herdr-drafts", "entrypoint": "drafts", "placement": "split",
            "target_pane_id": target.pane_id, "direction": "right", "focus": true,
            "env": {"HERDR_DRAFTS_TARGET": target.to_json()?}
        }),
    )?;
    result["plugin_pane"]["pane"]["pane_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::new("Le nouveau panneau Herdr n'est pas identifié."))
}

fn run_drafts(
    client: HerdrClient,
    target: Target,
    directory: &Path,
    panel_id: Option<String>,
    allow_switch: bool,
) -> Result<bool> {
    let _lock = PanelLock::acquire(directory, &target)?;
    let store = DraftStore::open(&directory.join("drafts.sqlite3"))?;
    let service = DraftService::new(store, client, target)?;
    service.store.recover_inflight(&service.target)?;
    ui::run_queue(service, panel_id, allow_switch)
}

struct DemoDirectory(PathBuf);
impl Drop for DemoDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_demo() -> Result<()> {
    let directory =
        DemoDirectory(std::env::temp_dir().join(format!("herdr-queue-demo-{}", unique_id())));
    DirBuilder::new().mode(0o700).create(&directory.0)?;
    let target = Target {
        socket_path: "/demo".into(),
        pane_id: "démo".into(),
        terminal_id: "demo-terminal".into(),
        workspace_id: "demo".into(),
        agent: "codex".into(),
        cwd: "~/mon-projet".into(),
        session: Some("demo".into()),
    };
    let store = DraftStore::open(&directory.0.join("drafts.sqlite3"))?;
    for body in [
        "Quand les tests seront terminés, vérifie aussi le comportement sur mobile.",
        "Prépare un résumé des changements\net des points à tester ensemble.",
    ] {
        store.save_composer(&target, body, None)?;
        store.commit_composer(&target)?;
    }
    ui::run_queue(DraftService::demo(store, target)?, None, false)?;
    Ok(())
}

fn doctor(environment: &HashMap<String, String>) -> Result<()> {
    let binary = environment
        .get("HERDR_BIN_PATH")
        .map(String::as_str)
        .unwrap_or("herdr");
    let mut child = std::process::Command::new(binary)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::new("Herdr ne répond pas."));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(Error::new("Herdr n'a pas pu communiquer sa version."));
    }
    println!(
        "Herdr Queue {} (Rust natif)\n{}",
        env!("CARGO_PKG_VERSION"),
        String::from_utf8_lossy(&output.stdout).trim()
    );
    println!(
        "Contexte Herdr : {}",
        if environment.get("HERDR_ENV").map(String::as_str) == Some("1") {
            "oui"
        } else {
            "non"
        }
    );
    println!(
        "Données locales : {}\nDiagnostic local uniquement ; aucun message envoyé.",
        state_directory(environment).display()
    );
    Ok(())
}

fn run_terminal(
    mut pick_agent: impl FnMut(&str) -> Result<Option<Agent>>,
    mut open_queue: impl FnMut(Agent) -> Result<bool>,
) -> Result<()> {
    let mut notice = String::new();
    while let Some(agent) = pick_agent(&notice)? {
        match open_queue(agent) {
            Ok(true) => notice.clear(),
            Ok(false) => break,
            Err(error) if error.code == "interrupted" => return Err(error),
            Err(error) => notice = error.to_string(),
        }
    }
    Ok(())
}

pub fn execute(command: Command, environment: &HashMap<String, String>) -> Result<()> {
    match command {
        Command::Version => println!("Herdr Queue {}", env!("CARGO_PKG_VERSION")),
        Command::Help => println!("Herdr Queue — préparez plusieurs messages pour votre agent Herdr.\n\nUsage : herdr-queue [connect|open|tui|demo|doctor] [--session NOM | --socket CHEMIN]\n\nconnect : choisir un agent depuis ce terminal (par défaut)\nopen    : ouvrir le panneau à droite de l'agent Herdr actif\ntui     : panneau intégré, lancé par Herdr\ndemo    : démonstration isolée, aucun message réel\ndoctor  : version, contexte et stockage\n\n--version / --help"),
        Command::Doctor => doctor(environment)?,
        Command::Demo => run_demo()?,
        Command::Open => {
            let source = source_pane(environment)?;
            let socket = environment.get("HERDR_SOCKET_PATH").ok_or_else(|| Error::new("Le contexte Herdr ne fournit pas de socket."))?;
            println!("Brouillons ouverts : {}", open_panel(&HerdrClient::new(socket.clone()), &source)?);
        }
        Command::Tui => {
            let raw = environment.get("HERDR_DRAFTS_TARGET").ok_or_else(|| Error::new("Destination manquante. Lancez « Ouvrir les brouillons » dans Herdr."))?;
            let target = Target::from_json(raw)?;
            if environment.get("HERDR_SOCKET_PATH") != Some(&target.socket_path) { return Err(Error::new("Le socket Herdr ne correspond pas à la destination enregistrée.")); }
            let panel = environment.get("HERDR_PANE_ID").cloned();
            let client = HerdrClient::new(target.socket_path.clone());
            run_drafts(client.clone(), target.clone(), &state_directory(environment), panel.clone(), false)?;
            if let Some(panel) = panel.filter(|pane| pane != &target.pane_id) {
                let _ = client.request("plugin.pane.close", json!({"pane_id": panel}));
            }
        }
        Command::Connect { session, socket } => {
            run_terminal(
                |notice| ui::pick_agent(environment, session.as_deref(), socket.as_deref(), notice),
                |agent| run_drafts(HerdrClient::new(agent.target.socket_path.clone()), agent.target, &state_directory(environment), None, true),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::test_support::{agent_json, scripted_client, target};
    use serde_json::json;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn selected_agent() -> Agent {
        Agent {
            target: target(),
            status: "idle".into(),
            name: "Agent de test".into(),
            workspace_label: String::new(),
            tab_label: String::new(),
            session_label: String::new(),
        }
    }

    #[test]
    fn interruption_terminates_standalone_without_reopening_the_picker() {
        let mut selections = 0;
        let result = run_terminal(
            |_| {
                selections += 1;
                Ok((selections == 1).then(selected_agent))
            },
            |_| {
                Err(Error {
                    message: "Fermeture demandée.".into(),
                    code: "interrupted".into(),
                    uncertain: false,
                })
            },
        );
        assert_eq!(
            selections, 1,
            "An interruption must never reopen the picker"
        );
        assert_eq!(result.unwrap_err().code, "interrupted");
    }

    #[test]
    fn queue_conflict_returns_to_the_picker_with_the_error_and_can_be_closed() {
        let mut notices = Vec::new();
        run_terminal(
            |notice| {
                notices.push(notice.to_owned());
                Ok((notices.len() == 1).then(selected_agent))
            },
            |_| Err(Error::new("Un panneau est déjà ouvert pour cet agent.")),
        )
        .unwrap();
        assert_eq!(notices, ["", "Un panneau est déjà ouvert pour cet agent."]);
    }

    #[test]
    fn standalone_options_preserve_default_and_reject_ambiguous_targets() {
        assert_eq!(
            parse_args(&[]).unwrap(),
            Command::Connect {
                session: None,
                socket: None
            }
        );
        assert_eq!(
            parse_args(&args(&["--socket", "/tmp/agent.sock"])).unwrap(),
            Command::Connect {
                session: None,
                socket: Some("/tmp/agent.sock".into())
            }
        );
        assert_eq!(
            parse_args(&args(&["connect", "--session", "default"])).unwrap(),
            Command::Connect {
                session: Some("default".into()),
                socket: None
            }
        );
        for invalid in [
            &["--socket"][..],
            &["tui", "--session", "default"],
            &["--socket", "s", "--session", "a"],
            &["demo", "open"],
            &["--unknown"],
        ] {
            assert!(parse_args(&args(invalid)).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn plugin_action_uses_focused_context_before_its_own_pane() {
        let mut env = HashMap::from([
            ("HERDR_ENV".into(), "1".into()),
            ("HERDR_PANE_ID".into(), "plugin-pane".into()),
            (
                "HERDR_PLUGIN_CONTEXT_JSON".into(),
                r#"{"focused_pane_id":"agent-pane"}"#.into(),
            ),
        ]);
        assert_eq!(source_pane(&env).unwrap(), "agent-pane");
        env.remove("HERDR_PLUGIN_CONTEXT_JSON");
        assert_eq!(source_pane(&env).unwrap(), "plugin-pane");
        env.remove("HERDR_ENV");
        assert!(source_pane(&env).is_err());
    }

    #[test]
    fn storage_keeps_the_existing_plugin_directory_and_override() {
        let env = HashMap::from([("XDG_STATE_HOME".into(), "/tmp/state".into())]);
        assert_eq!(
            state_directory(&env),
            PathBuf::from("/tmp/state/herdr/plugins/herdr-drafts")
        );
        let env = HashMap::from([("HERDR_PLUGIN_STATE_DIR".into(), "/tmp/plugin-state".into())]);
        assert_eq!(state_directory(&env), PathBuf::from("/tmp/plugin-state"));
    }

    #[test]
    fn open_action_opens_a_right_split_with_a_pinned_target() {
        let (client, workers) = scripted_client(
            vec![
                Some(json!({"result":{"pane":{"tokens":{}}}})),
                Some(json!({"result":{"agent":agent_json()}})),
                Some(json!({"result":{"panes":[]}})),
                Some(json!({"result":{"plugin_pane":{"pane":{"pane_id":"queue-pane"}}}})),
            ],
            false,
        );
        assert_eq!(open_panel(&client, "w1:p1").unwrap(), "queue-pane");
        let requests: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(requests[3]["method"], "plugin.pane.open");
        assert_eq!(requests[3]["params"]["direction"], "right");
        assert_eq!(requests[3]["params"]["plugin_id"], "herdr-drafts");
        assert_eq!(requests[3]["params"]["target_pane_id"], "w1:p1");
        let bound = requests[3]["params"]["env"]["HERDR_DRAFTS_TARGET"]
            .as_str()
            .unwrap();
        assert_eq!(crate::models::Target::from_json(bound).unwrap(), target());
    }

    #[test]
    fn reopening_an_owned_panel_focuses_without_creating_another() {
        let bound = target();
        let (client, workers) = scripted_client(
            vec![
                Some(json!({"result":{"pane":{"tokens":{}}}})),
                Some(json!({"result":{"agent":agent_json()}})),
                Some(
                    json!({"result":{"panes":[{"pane_id":"existing","tokens":{"drafts_owner":bound.lock_key(),"drafts_target":bound.key()}}]}}),
                ),
                Some(json!({"result":{}})),
            ],
            false,
        );
        assert_eq!(open_panel(&client, "w1:p1").unwrap(), "existing");
        let requests: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(requests[3]["method"], "plugin.pane.focus");
        assert_eq!(requests[3]["params"], json!({"pane_id":"existing"}));
    }
}
