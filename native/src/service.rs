use crate::herdr::HerdrClient;
use crate::models::{Agent, Draft, Error, Result, Target};
use crate::store::DraftStore;

pub const MAX_PROMPT_BYTES: usize = 64 * 1024;

pub struct DraftService {
    pub store: DraftStore,
    pub client: HerdrClient,
    pub target: Target,
}
impl DraftService {
    pub fn new(store: DraftStore, client: HerdrClient, target: Target) -> Result<Self> {
        let resolved = store.resolve_target(&target)?;
        let mut service = Self {
            store,
            client,
            target: resolved,
        };
        if target.session.is_some() {
            let mut initial = target.clone();
            initial.session = None;
            if let Some(initial) = service.store.recorded_target(&initial.key())? {
                if service.store.resolve_target(&initial)?.session.is_none() {
                    service.target = initial;
                    service.verify_target(&target)?;
                }
            }
        }
        Ok(service)
    }
    pub fn demo(store: DraftStore, target: Target) -> Result<Self> {
        let resolved = store.resolve_target(&target)?;
        Self::new(store, HerdrClient::demo(resolved), target)
    }
    pub fn save(&self) -> Result<Draft> {
        validate_prompt(&self.store.composer(&self.target)?.body)?;
        self.store.commit_composer(&self.target)
    }
    pub fn verify_target(&mut self, current: &Target) -> Result<()> {
        let bound = &self.target;
        if (
            current.socket_path.as_str(),
            current.pane_id.as_str(),
            current.terminal_id.as_str(),
            current.workspace_id.as_str(),
            current.agent.as_str(),
            current.cwd.as_str(),
        ) != (
            bound.socket_path.as_str(),
            bound.pane_id.as_str(),
            bound.terminal_id.as_str(),
            bound.workspace_id.as_str(),
            bound.agent.as_str(),
            bound.cwd.as_str(),
        ) || (bound.session.is_some() && current.session != bound.session)
        {
            return Err(Error::new("La conversation cible a changé. Fermez ce panneau (Ctrl+Q), puis rouvrez-le depuis l'agent souhaité."));
        }
        if bound.session.is_none() && current.session.is_some() {
            self.store.adopt_session(bound, current)?;
            self.target = current.clone();
        }
        Ok(())
    }
    pub fn refresh(&mut self) -> Result<Agent> {
        let live = self.client.get_agent(&self.target.pane_id)?;
        self.verify_target(&live.target)?;
        Ok(live)
    }
    pub fn send(&mut self, id: &str) -> Result<Draft> {
        let draft = self.store.get(id)?;
        if draft.target_key != self.target.key() {
            return Err(Error::new("Ce message appartient à un autre agent."));
        }
        validate_prompt(&draft.body)?;
        self.store.claim(id)?;
        let result = (|| -> Result<()> {
            let live = self.refresh()?;
            if !matches!(live.status.as_str(), "idle" | "done" | "working") {
                return Err(Error::new(if live.status == "blocked" {
                    "L'agent attend une réponse interactive."
                } else {
                    "L'état de l'agent est inconnu. Vérifiez son terminal."
                }));
            }
            self.client.prompt(&self.target.pane_id, &draft.body)
        })();
        match result {
            Ok(()) => self.store.finish(id, "sent", None)?,
            Err(error) => self.store.finish(
                id,
                if error.uncertain {
                    "uncertain"
                } else {
                    "error"
                },
                Some(&error.message),
            )?,
        };
        self.store.get(id)
    }
}

pub fn validate_prompt(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(Error::new("Écrivez un message avant de l'ajouter."));
    }
    if body.len() > MAX_PROMPT_BYTES {
        return Err(Error::new(
            "Le message dépasse 64 Kio. Divisez-le en plusieurs messages.",
        ));
    }
    if body.chars().any(|character| {
        (character < ' ' && character != '\n' && character != '\t')
            || ('\u{7f}'..='\u{9f}').contains(&character)
    }) {
        return Err(Error::new(
            "Le message contient un caractère de contrôle du terminal.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::test_support::*;
    use crate::models::unique_id;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    struct Database(PathBuf);
    impl Database {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("herdr-service-{}.sqlite3", unique_id())))
        }
        fn open(&self) -> DraftStore {
            DraftStore::open(&self.0).unwrap()
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }
    fn add(service: &DraftService, text: &str) -> Draft {
        service
            .store
            .save_composer(&service.target, text, None)
            .unwrap();
        service.save().unwrap()
    }
    #[test]
    fn prompt_validation_rejects_controls_and_limits_utf8_bytes() {
        for body in [
            "",
            "  \n ",
            "a\u{1b}[31m",
            "a\0b",
            "a\rb",
            "a\u{7f}b",
            "a\u{85}b",
        ] {
            assert!(validate_prompt(body).is_err(), "{body:?}");
        }
        assert!(validate_prompt(&"é".repeat(32769)).is_err());
        validate_prompt(&"é".repeat(32768)).unwrap();
        validate_prompt("plusieurs\nlignes\t🎯").unwrap();
    }
    #[test]
    fn manual_send_marks_sent_and_keeps_literal_body() {
        let database = Database::new();
        let (client, workers) = scripted_client(
            vec![
                Some(json!({"result":{"agent":agent_json()}})),
                Some(json!({"result":{"type":"agent_prompted"}})),
            ],
            true,
        );
        let mut service = DraftService::new(database.open(), client, target()).unwrap();
        let draft = add(&service, "À faire ensuite\nmerci 🎯");
        assert_eq!(service.send(&draft.id).unwrap().state, "sent");
        assert!(service.send(&draft.id).is_err());
        let requests: Vec<Value> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(requests[1]["params"]["text"], "À faire ensuite\nmerci 🎯");
        assert_eq!(service.store.get(&draft.id).unwrap().body, draft.body);
    }
    #[test]
    fn lost_acknowledgment_cannot_be_sent_again() {
        let database = Database::new();
        let (client, workers) = scripted_client(
            vec![Some(json!({"result":{"agent":agent_json()}})), None],
            false,
        );
        let mut service = DraftService::new(database.open(), client, target()).unwrap();
        let draft = add(&service, "message");
        assert_eq!(service.send(&draft.id).unwrap().state, "uncertain");
        assert!(service.send(&draft.id).is_err());
        for worker in workers {
            worker.join().unwrap();
        }
    }
    #[test]
    fn unready_or_changed_target_never_reaches_prompt() {
        for (field, replacement) in [
            ("agent_status", json!("blocked")),
            ("agent_status", json!("unknown")),
            ("terminal_id", json!("replacement")),
            ("agent", json!("codex")),
            ("cwd", json!("/other")),
            ("pane_id", json!("w1:p2")),
            ("workspace_id", json!("w2")),
            (
                "agent_session",
                json!({"source":"test","kind":"id","value":"other"}),
            ),
        ] {
            let database = Database::new();
            let mut live = agent_json();
            live[field] = replacement;
            let (client, workers) =
                scripted_client(vec![Some(json!({"result":{"agent":live}}))], false);
            let mut service = DraftService::new(database.open(), client, target()).unwrap();
            let draft = add(&service, "message");
            assert_eq!(service.send(&draft.id).unwrap().state, "error", "{field}");
            assert_eq!(
                workers.into_iter().next().unwrap().join().unwrap()["method"],
                "agent.get"
            );
        }
    }
    #[test]
    fn another_targets_draft_is_not_claimed() {
        let database = Database::new();
        let mut service = DraftService::demo(database.open(), target()).unwrap();
        let mut other = target();
        other.terminal_id = "other".into();
        service
            .store
            .save_composer(&other, "another agent", None)
            .unwrap();
        let draft = service.store.commit_composer(&other).unwrap();
        assert!(service.send(&draft.id).is_err());
        assert_eq!(service.store.get(&draft.id).unwrap().state, "draft");
    }
    #[test]
    fn first_native_session_adopts_queue_and_composer_on_reopen() {
        let database = Database::new();
        let live = target();
        let mut initial = live.clone();
        initial.session = None;
        let store = database.open();
        store.save_composer(&initial, "À garder", None).unwrap();
        let draft = store.commit_composer(&initial).unwrap();
        store
            .save_composer(&initial, "Écriture en cours", None)
            .unwrap();
        let service = DraftService::demo(store, live.clone()).unwrap();
        assert_eq!(service.target, live);
        assert_eq!(
            service.store.list_drafts(&service.target, false).unwrap()[0].id,
            draft.id
        );
        assert_eq!(
            service.store.composer(&service.target).unwrap().body,
            "Écriture en cours"
        );
    }
    #[test]
    fn reopened_original_environment_resolves_adoption_before_recovery() {
        let database = Database::new();
        let live = target();
        let mut initial = live.clone();
        initial.session = None;
        let mut original = DraftService::demo(database.open(), initial.clone()).unwrap();
        let draft = add(&original, "message");
        original.verify_target(&live).unwrap();
        original
            .store
            .save_composer(&original.target, "Texte durable", Some(&draft.id))
            .unwrap();
        original.store.claim(&draft.id).unwrap();
        let restored = DraftService::demo(database.open(), initial).unwrap();
        restored.store.recover_inflight(&restored.target).unwrap();
        assert_eq!(restored.target, live);
        let composer = restored.store.composer(&restored.target).unwrap();
        assert_eq!(composer.body, "Texte durable");
        assert_eq!(composer.editing_id, Some(draft.id.clone()));
        assert_eq!(restored.store.get(&draft.id).unwrap().state, "uncertain");
    }
    #[test]
    fn known_conversation_is_never_rebound_to_another() {
        let database = Database::new();
        let live = target();
        let mut initial = live.clone();
        initial.session = None;
        let mut original = DraftService::demo(database.open(), initial).unwrap();
        let draft = add(&original, "message");
        original.verify_target(&live).unwrap();
        let mut different = live.clone();
        different.session = Some("other conversation".into());
        let reopened = DraftService::demo(database.open(), different.clone()).unwrap();
        assert_eq!(reopened.target, different);
        assert!(reopened
            .store
            .list_drafts(&different, false)
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened.store.get(&draft.id).unwrap().target_key,
            live.key()
        );
    }
    #[test]
    fn demo_reopen_refreshes_the_resolved_native_identity() {
        let database = Database::new();
        let live = target();
        let mut initial = live.clone();
        initial.session = None;
        let mut original = DraftService::demo(database.open(), initial.clone()).unwrap();
        add(&original, "message");
        original.verify_target(&live).unwrap();
        let mut restored = DraftService::demo(database.open(), initial).unwrap();
        assert_eq!(restored.refresh().unwrap().target, live);
    }
    #[test]
    fn failed_revalidation_keeps_draft_retryable_without_prompt() {
        let database = Database::new();
        let (client, workers) = scripted_client(vec![None], false);
        let mut service = DraftService::new(database.open(), client, target()).unwrap();
        let draft = add(&service, "message");
        let failed = service.send(&draft.id).unwrap();
        assert_eq!(failed.state, "error");
        assert!(failed.error.is_some());
        assert_eq!(
            workers.into_iter().next().unwrap().join().unwrap()["method"],
            "agent.get"
        );
    }
}
