use crate::models::{unique_id, Composer, Draft, Error, Result, Target};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug)]
pub struct DraftStore {
    pub path: PathBuf,
    connection: Connection,
}

impl DraftStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        drop(file);
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(3))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS targets (
                key TEXT PRIMARY KEY,
                identity TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS composers (
                target_key TEXT PRIMARY KEY REFERENCES targets(key),
                body TEXT NOT NULL,
                editing_id TEXT
            );
            CREATE TABLE IF NOT EXISTS drafts (
                id TEXT PRIMARY KEY,
                target_key TEXT NOT NULL REFERENCES targets(key),
                body TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('draft','sending','sent','error','uncertain')),
                position INTEGER NOT NULL,
                error TEXT,
                updated TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            );
            CREATE INDEX IF NOT EXISTS drafts_target ON drafts(target_key, position);
            CREATE TABLE IF NOT EXISTS target_aliases (
                alias_key TEXT PRIMARY KEY,
                target_key TEXT NOT NULL REFERENCES targets(key)
            );",
        )?;
        Ok(Self {
            path: path.to_owned(),
            connection,
        })
    }

    pub fn composer(&self, target: &Target) -> Result<Composer> {
        Ok(self
            .connection
            .query_row(
                "SELECT body, editing_id FROM composers WHERE target_key =
             COALESCE((SELECT target_key FROM target_aliases WHERE alias_key=?1), ?1)",
                [target.key()],
                |row| {
                    Ok(Composer {
                        body: row.get(0)?,
                        editing_id: row.get(1)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn save_composer(
        &self,
        target: &Target,
        body: &str,
        editing_id: Option<&str>,
    ) -> Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let target = self.resolve_target(target)?;
        register_target(&transaction, &target)?;
        transaction.execute(
            "INSERT INTO composers VALUES (?, ?, ?) ON CONFLICT(target_key)
             DO UPDATE SET body=excluded.body, editing_id=excluded.editing_id",
            params![target.key(), body, editing_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn commit_composer(&self, target: &Target) -> Result<Draft> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let target = self.resolve_target(target)?;
        let composer = self.composer(&target)?;
        if composer.body.trim().is_empty() {
            return Err(Error::new("Le message est vide."));
        }
        register_target(&transaction, &target)?;
        let key = target.key();
        let id = if let Some(id) = composer.editing_id {
            let changed = transaction.execute(
                "UPDATE drafts SET body=?, state='draft', error=NULL
                 WHERE id=? AND target_key=? AND state IN ('draft','error')",
                params![composer.body, id, key],
            )?;
            if changed != 1 {
                return Err(Error::new(
                    "Ce message a changé. Conservez votre texte avant de continuer.",
                ));
            }
            id
        } else {
            let id = unique_id();
            let position: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(position), -1)+1 FROM drafts WHERE target_key=?",
                [&key],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO drafts(id,target_key,body,state,position) VALUES(?,?,?,'draft',?)",
                params![id, key, composer.body, position],
            )?;
            id
        };
        transaction.execute("DELETE FROM composers WHERE target_key=?", [&key])?;
        let draft = self.get(&id)?;
        transaction.commit()?;
        Ok(draft)
    }

    pub fn get(&self, id: &str) -> Result<Draft> {
        self.connection
            .query_row(
                "SELECT id,target_key,body,state,position,error FROM drafts WHERE id=?",
                [id],
                draft_from_row,
            )
            .optional()?
            .ok_or_else(|| Error::new("Ce message n'existe plus."))
    }

    pub fn list_drafts(&self, target: &Target, history: bool) -> Result<Vec<Draft>> {
        let query = if history {
            "SELECT id,target_key,body,state,position,error FROM drafts
             WHERE target_key=COALESCE((SELECT target_key FROM target_aliases WHERE alias_key=?1), ?1)
             AND state='sent' ORDER BY updated DESC, position DESC"
        } else {
            "SELECT id,target_key,body,state,position,error FROM drafts
             WHERE target_key=COALESCE((SELECT target_key FROM target_aliases WHERE alias_key=?1), ?1)
             AND state!='sent' ORDER BY position,id"
        };
        let mut statement = self.connection.prepare(query)?;
        let drafts = statement
            .query_map([target.key()], draft_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(drafts)
    }

    pub fn claim(&self, id: &str) -> Result<Draft> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE drafts SET state='sending', error=NULL WHERE id=? AND state IN ('draft','error')", [id],
        )?;
        if changed != 1 {
            return Err(Error::new(
                "Ce message est déjà transmis ou en cours d'envoi.",
            ));
        }
        let draft = self.get(id)?;
        transaction.commit()?;
        Ok(draft)
    }

    pub fn finish(&self, id: &str, state: &str, error: Option<&str>) -> Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE drafts SET state=?,error=?,updated=strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id=? AND state='sending'",
            params![state, error, id],
        )?;
        if changed != 1 {
            return Err(Error::new("L'état de ce message a changé."));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn resolve(&self, id: &str, delivered: bool) -> Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE drafts SET state=?,error=NULL WHERE id=? AND state='uncertain'",
            params![if delivered { "sent" } else { "draft" }, id],
        )?;
        if changed != 1 {
            return Err(Error::new("Ce message n'attend pas de vérification."));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn recover_inflight(&self, target: &Target) -> Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE drafts SET state='uncertain',error=?1 WHERE target_key=
             COALESCE((SELECT target_key FROM target_aliases WHERE alias_key=?2), ?2) AND state='sending'",
            params!["Envoi interrompu. Vérifiez le terminal avant de réessayer.", target.key()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "DELETE FROM drafts WHERE id=? AND state IN ('draft','error')",
            [id],
        )?;
        if changed != 1 {
            return Err(Error::new(
                "Ce message ne peut pas être supprimé dans son état actuel.",
            ));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn move_draft(&self, id: &str, direction: i32) -> Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let draft = self.get(id)?;
        if draft.state != "draft" && draft.state != "error" {
            return Err(Error::new("Ce message ne peut pas être déplacé."));
        }
        let rows = {
            let mut statement = transaction.prepare(
                "SELECT id,position FROM drafts WHERE target_key=? AND state!='sent' ORDER BY position,id",
            )?;
            let rows = statement
                .query_map([&draft.target_key], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let index = rows
            .iter()
            .position(|(row_id, _)| row_id == id)
            .ok_or_else(|| Error::new("Ce message n'existe plus."))?;
        let neighbor = if direction < 0 {
            index.checked_sub(1)
        } else {
            index.checked_add(1)
        };
        if let Some((neighbor_id, position)) = neighbor.and_then(|index| rows.get(index)) {
            transaction.execute(
                "UPDATE drafts SET position=? WHERE id=?",
                params![position, id],
            )?;
            transaction.execute(
                "UPDATE drafts SET position=? WHERE id=?",
                params![draft.position, neighbor_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn recorded_target(&self, key: &str) -> Result<Option<Target>> {
        let identity: Option<String> = self
            .connection
            .query_row("SELECT identity FROM targets WHERE key=?", [key], |row| {
                row.get(0)
            })
            .optional()?;
        identity.map(|raw| Target::from_json(&raw)).transpose()
    }

    pub fn resolve_target(&self, target: &Target) -> Result<Target> {
        let identity: Option<String> = self.connection.query_row(
            "SELECT targets.identity FROM target_aliases JOIN targets ON targets.key=target_aliases.target_key WHERE alias_key=?",
            [target.key()], |row| row.get(0),
        ).optional()?;
        Ok(match identity {
            Some(raw) => Target::from_json(&raw)?,
            None => target.clone(),
        })
    }

    pub fn adopt_session(&self, previous: &Target, current: &Target) -> Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        register_target(&transaction, current)?;
        let previous_key = previous.key();
        let current_key = current.key();
        transaction.execute(
            "UPDATE drafts SET target_key=? WHERE target_key=?",
            params![current_key, previous_key],
        )?;
        transaction.execute(
            "UPDATE composers SET target_key=? WHERE target_key=?",
            params![current_key, previous_key],
        )?;
        transaction.execute(
            "INSERT INTO target_aliases VALUES (?, ?) ON CONFLICT(alias_key) DO UPDATE SET target_key=excluded.target_key",
            params![previous_key, current_key],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn register_target(connection: &Connection, target: &Target) -> Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO targets VALUES (?, ?)",
        params![target.key(), target.to_json()?],
    )?;
    Ok(())
}

fn draft_from_row(row: &Row<'_>) -> rusqlite::Result<Draft> {
    Ok(Draft {
        id: row.get(0)?,
        target_key: row.get(1)?,
        body: row.get(2)?,
        state: row.get(3)?,
        position: row.get(4)?,
        error: row.get(5)?,
    })
}

#[derive(Debug)]
pub struct PanelLock {
    pub path: PathBuf,
    file: File,
}

impl PanelLock {
    pub fn acquire(directory: &Path, target: &Target) -> Result<Self> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let path = directory.join(format!("panel-{}.lock", target.lock_key()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)?;
        // SAFETY: the descriptor is valid and the operation does not outlive the file.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(Error::new("Un panneau est déjà ouvert pour cet agent."));
            }
            return Err(error.into());
        }
        Ok(Self { path, file })
    }
}

impl Drop for PanelLock {
    fn drop(&mut self) {
        // SAFETY: the owned file remains open until this Drop implementation finishes.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::unique_id;
    use rusqlite::Connection;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};
    use std::thread;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("herdr-store-test-{}", unique_id())))
        }

        fn database(&self) -> PathBuf {
            self.0.join("drafts.sqlite3")
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn target() -> Target {
        Target {
            socket_path: "/tmp/herdr-test.sock".into(),
            pane_id: "w1:p1".into(),
            terminal_id: "term-1".into(),
            workspace_id: "w1".into(),
            agent: "claude".into(),
            cwd: "/project".into(),
            session: Some("s1".into()),
        }
    }

    fn add(store: &DraftStore, target: &Target, body: &str) -> Draft {
        store.save_composer(target, body, None).unwrap();
        store.commit_composer(target).unwrap()
    }

    fn bodies(store: &DraftStore, target: &Target, history: bool) -> Vec<String> {
        store
            .list_drafts(target, history)
            .unwrap()
            .into_iter()
            .map(|draft| draft.body)
            .collect()
    }

    #[test]
    fn composer_and_queue_survive_reopen_without_crossing_sessions() {
        let directory = Directory::new();
        let target = target();
        let first;
        {
            let store = DraftStore::open(&directory.database()).unwrap();
            assert_eq!(store.composer(&target).unwrap(), Composer::default());
            first = add(&store, &target, "Première ligne\nDeuxième ligne 🎯");
            add(&store, &target, "Ensuite, vérifie les tests.");
            store
                .save_composer(&target, "Une idée encore incomplète…", Some(&first.id))
                .unwrap();
        }
        let store = DraftStore::open(&directory.database()).unwrap();
        assert_eq!(store.path, directory.database());
        assert_eq!(
            bodies(&store, &target, false),
            [first.body, "Ensuite, vérifie les tests.".into()]
        );
        assert_eq!(
            store.composer(&target).unwrap(),
            Composer {
                body: "Une idée encore incomplète…".into(),
                editing_id: Some(first.id)
            }
        );
        let other = Target {
            session: Some("s2".into()),
            ..target.clone()
        };
        assert!(store.list_drafts(&other, false).unwrap().is_empty());
        assert_eq!(store.composer(&other).unwrap(), Composer::default());
        assert_eq!(store.recorded_target(&target.key()).unwrap(), Some(target));
        assert_eq!(store.recorded_target("missing").unwrap(), None);
    }

    #[test]
    fn editing_updates_in_place_only_when_saved_and_clears_retry_error() {
        let directory = Directory::new();
        let store = DraftStore::open(&directory.database()).unwrap();
        let target = target();
        let draft = add(&store, &target, "original");
        store.claim(&draft.id).unwrap();
        store
            .finish(&draft.id, "error", Some("Refus connu"))
            .unwrap();
        store
            .save_composer(&target, "modifié", Some(&draft.id))
            .unwrap();
        assert_eq!(store.get(&draft.id).unwrap().body, "original");
        let updated = store.commit_composer(&target).unwrap();
        assert_eq!(updated.id, draft.id);
        assert_eq!(updated.position, draft.position);
        assert_eq!(updated.body, "modifié");
        assert_eq!(updated.state, "draft");
        assert_eq!(updated.error, None);
        assert_eq!(store.composer(&target).unwrap(), Composer::default());
    }

    #[test]
    fn conflicting_edit_and_empty_commit_preserve_composer_text() {
        let directory = Directory::new();
        let store = DraftStore::open(&directory.database()).unwrap();
        let target = target();
        let draft = add(&store, &target, "original");
        store
            .save_composer(&target, "À conserver", Some(&draft.id))
            .unwrap();
        store.claim(&draft.id).unwrap();
        assert!(store.commit_composer(&target).is_err());
        assert_eq!(
            store.composer(&target).unwrap(),
            Composer {
                body: "À conserver".into(),
                editing_id: Some(draft.id.clone())
            }
        );
        assert_eq!(store.get(&draft.id).unwrap().body, "original");
        store.save_composer(&target, " \n\t", None).unwrap();
        assert!(store.commit_composer(&target).is_err());
        assert_eq!(store.composer(&target).unwrap().body, " \n\t");
        let other = Target {
            session: None,
            ..target.clone()
        };
        store
            .save_composer(&other, "other conversation", Some(&draft.id))
            .unwrap();
        assert!(store.commit_composer(&other).is_err());
        assert_eq!(store.composer(&other).unwrap().body, "other conversation");
    }

    #[test]
    fn only_one_connection_can_claim_a_draft() {
        let directory = Directory::new();
        let one = DraftStore::open(&directory.database()).unwrap();
        let two = DraftStore::open(&directory.database()).unwrap();
        let target = target();
        let draft = add(&one, &target, "only once");
        let barrier = Arc::new(Barrier::new(2));
        let handles = [one, two]
            .into_iter()
            .map(|store| {
                let barrier = Arc::clone(&barrier);
                let id = draft.id.clone();
                thread::spawn(move || {
                    barrier.wait();
                    store.claim(&id).is_ok()
                })
            })
            .collect::<Vec<_>>();
        let claimed = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|claimed| *claimed)
            .count();
        assert_eq!(claimed, 1);
        let store = DraftStore::open(&directory.database()).unwrap();
        assert_eq!(store.get(&draft.id).unwrap().state, "sending");
        store.finish(&draft.id, "sent", None).unwrap();
        assert!(store.list_drafts(&target, false).unwrap().is_empty());
        assert_eq!(bodies(&store, &target, true), ["only once"]);
        assert!(store.finish(&draft.id, "sent", None).is_err());
        assert!(store.claim("missing").is_err());
        assert!(store.get("missing").is_err());
    }

    #[test]
    fn interrupted_delivery_requires_explicit_resolution_for_its_own_target() {
        let directory = Directory::new();
        let store = DraftStore::open(&directory.database()).unwrap();
        let target = target();
        let other = Target {
            terminal_id: "another-terminal".into(),
            ..target.clone()
        };
        let first = add(&store, &target, "first");
        let second = add(&store, &other, "second");
        store.claim(&first.id).unwrap();
        store.claim(&second.id).unwrap();
        store.recover_inflight(&target).unwrap();
        assert_eq!(store.get(&first.id).unwrap().state, "uncertain");
        assert!(store.get(&first.id).unwrap().error.is_some());
        assert_eq!(store.get(&second.id).unwrap().state, "sending");
        assert!(store.claim(&first.id).is_err());
        assert!(store.delete(&first.id).is_err());
        assert!(store.move_draft(&first.id, -1).is_err());
        store.resolve(&first.id, false).unwrap();
        assert_eq!(store.get(&first.id).unwrap().error, None);
        store.claim(&first.id).unwrap();
        store
            .finish(&first.id, "uncertain", Some("Connexion perdue"))
            .unwrap();
        store.resolve(&first.id, true).unwrap();
        assert_eq!(store.get(&first.id).unwrap().state, "sent");
        assert!(store.resolve(&first.id, false).is_err());
        assert!(store.delete(&first.id).is_err());
        assert!(store.finish(&second.id, "invalid", None).is_err());
        assert_eq!(store.get(&second.id).unwrap().state, "sending");
    }

    #[test]
    fn reordering_and_deleting_keep_queue_order_without_moving_history() {
        let directory = Directory::new();
        let store = DraftStore::open(&directory.database()).unwrap();
        let target = target();
        let a = add(&store, &target, "a");
        let b = add(&store, &target, "b");
        let c = add(&store, &target, "c");
        store.move_draft(&a.id, -1).unwrap();
        store.move_draft(&c.id, 1).unwrap();
        store.move_draft(&c.id, -1).unwrap();
        assert_eq!(bodies(&store, &target, false), ["a", "c", "b"]);
        store.claim(&a.id).unwrap();
        store.finish(&a.id, "sent", None).unwrap();
        store.move_draft(&c.id, -1).unwrap();
        assert_eq!(bodies(&store, &target, false), ["c", "b"]);
        store.delete(&c.id).unwrap();
        assert_eq!(bodies(&store, &target, false), ["b"]);
        assert_eq!(bodies(&store, &target, true), ["a"]);
        store.claim(&b.id).unwrap();
        store.finish(&b.id, "error", Some("retry")).unwrap();
        store.delete(&b.id).unwrap();
        assert!(store.list_drafts(&target, false).unwrap().is_empty());
        assert!(store.delete("missing").is_err());
        assert!(store.move_draft("missing", 1).is_err());
    }

    #[test]
    fn first_native_session_adopts_queue_composer_and_restart_alias() {
        let directory = Directory::new();
        let current = target();
        let previous = Target {
            session: None,
            ..current.clone()
        };
        let draft_id;
        {
            let store = DraftStore::open(&directory.database()).unwrap();
            let draft = add(&store, &previous, "next prompt");
            draft_id = draft.id.clone();
            store
                .save_composer(&previous, "still writing", Some(&draft.id))
                .unwrap();
            store.adopt_session(&previous, &current).unwrap();
        }
        let store = DraftStore::open(&directory.database()).unwrap();
        assert_eq!(store.get(&draft_id).unwrap().target_key, current.key());
        assert_eq!(
            store.composer(&current).unwrap(),
            Composer {
                body: "still writing".into(),
                editing_id: Some(draft_id.clone())
            }
        );
        assert_eq!(store.composer(&previous).unwrap().body, "still writing");
        assert_eq!(store.resolve_target(&previous).unwrap(), current);
        assert_eq!(
            store.recorded_target(&previous.key()).unwrap(),
            Some(previous)
        );
        let other = Target {
            session: Some("different-conversation".into()),
            ..current.clone()
        };
        assert_eq!(store.resolve_target(&other).unwrap(), other);
        store.claim(&draft_id).unwrap();
        store
            .recover_inflight(&store.resolve_target(&current).unwrap())
            .unwrap();
        assert_eq!(store.get(&draft_id).unwrap().state, "uncertain");
    }

    #[test]
    fn adoption_conflict_rolls_back_queue_and_preserves_both_composers() {
        let directory = Directory::new();
        let store = DraftStore::open(&directory.database()).unwrap();
        let current = target();
        let previous = Target {
            session: None,
            ..current.clone()
        };
        let draft = add(&store, &previous, "old queue");
        store
            .save_composer(&previous, "old composer", None)
            .unwrap();
        store
            .save_composer(&current, "current composer", None)
            .unwrap();
        assert!(store.adopt_session(&previous, &current).is_err());
        assert_eq!(store.get(&draft.id).unwrap().target_key, previous.key());
        assert_eq!(store.composer(&previous).unwrap().body, "old composer");
        assert_eq!(store.composer(&current).unwrap().body, "current composer");
        assert_eq!(store.resolve_target(&previous).unwrap(), previous);
    }

    #[test]
    fn saving_through_original_environment_after_adoption_keeps_one_composer() {
        let directory = Directory::new();
        let store = DraftStore::open(&directory.database()).unwrap();
        let current = target();
        let previous = Target {
            session: None,
            ..current.clone()
        };
        let draft = add(&store, &previous, "initial prompt");
        store
            .save_composer(&previous, "before adoption", None)
            .unwrap();
        store.adopt_session(&previous, &current).unwrap();
        store
            .save_composer(&previous, "text typed during adoption", Some(&draft.id))
            .unwrap();
        assert_eq!(
            store.composer(&previous).unwrap().body,
            "text typed during adoption"
        );
        assert_eq!(
            store.composer(&current).unwrap().body,
            "text typed during adoption"
        );
        let updated = store.commit_composer(&previous).unwrap();
        assert_eq!(updated.target_key, current.key());
        assert_eq!(updated.body, "text typed during adoption");
        assert_eq!(updated.id, draft.id);
        assert_eq!(store.composer(&current).unwrap(), Composer::default());
        assert_eq!(
            bodies(&store, &previous, false),
            ["text typed during adoption"]
        );
        let database = Connection::open(directory.database()).unwrap();
        let count: i64 = database
            .query_row(
                "SELECT COUNT(*) FROM composers WHERE target_key=?",
                [previous.key()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn reads_and_updates_a_literal_python_v1_database() {
        let directory = Directory::new();
        fs::create_dir_all(&directory.0).unwrap();
        {
            let database = Connection::open(directory.database()).unwrap();
            database.execute_batch(r#"
                CREATE TABLE targets (key TEXT PRIMARY KEY, identity TEXT NOT NULL);
                CREATE TABLE composers (target_key TEXT PRIMARY KEY REFERENCES targets(key), body TEXT NOT NULL, editing_id TEXT);
                CREATE TABLE drafts (id TEXT PRIMARY KEY, target_key TEXT NOT NULL REFERENCES targets(key), body TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('draft','sending','sent','error','uncertain')), position INTEGER NOT NULL, error TEXT, updated TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')));
                CREATE INDEX drafts_target ON drafts(target_key, position);
                CREATE TABLE target_aliases (alias_key TEXT PRIMARY KEY, target_key TEXT NOT NULL REFERENCES targets(key));
                INSERT INTO targets VALUES ('eb00767f03e06312e4e6aff99a49e16cd0f801e19a02d15217a2805315e58570', '{"socket_path": "/tmp/herdr-test.sock", "pane_id": "w1:p1", "terminal_id": "term-1", "workspace_id": "w1", "agent": "claude", "cwd": "/project", "session": "s1"}');
                INSERT INTO drafts VALUES ('python-id', 'eb00767f03e06312e4e6aff99a49e16cd0f801e19a02d15217a2805315e58570', 'Ancien texte 🎯', 'error', 9, 'retry', '2026-10-01T12:00:00.000Z');
                INSERT INTO composers VALUES ('eb00767f03e06312e4e6aff99a49e16cd0f801e19a02d15217a2805315e58570', 'Écriture durable', 'python-id');
            "#).unwrap();
        }
        let store = DraftStore::open(&directory.database()).unwrap();
        let target = target();
        assert_eq!(bodies(&store, &target, false), ["Ancien texte 🎯"]);
        let updated = store.commit_composer(&target).unwrap();
        assert_eq!(updated.id, "python-id");
        assert_eq!(updated.body, "Écriture durable");
        assert_eq!(updated.position, 9);
        assert_eq!(updated.error, None);
        assert_eq!(store.recorded_target(&target.key()).unwrap(), Some(target));
    }

    #[test]
    fn private_files_and_terminal_lock_survive_session_change_and_release() {
        let directory = Directory::new();
        let target = target();
        let store = DraftStore::open(&directory.database()).unwrap();
        add(&store, &target, "private prompt");
        assert_eq!(
            fs::metadata(directory.database())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let first = PanelLock::acquire(&directory.0, &target).unwrap();
        let next_session = Target {
            session: None,
            ..target.clone()
        };
        assert!(PanelLock::acquire(&directory.0, &next_session).is_err());
        let another_terminal = Target {
            terminal_id: "other".into(),
            ..target.clone()
        };
        let _other = PanelLock::acquire(&directory.0, &another_terminal).unwrap();
        drop(first);
        let _reopened = PanelLock::acquire(&directory.0, &next_session).unwrap();
    }
}
