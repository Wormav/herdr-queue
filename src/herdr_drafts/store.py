"""Transactional local persistence and exclusive ownership of a panel's queue."""

from __future__ import annotations

import fcntl
import os
import sqlite3
from pathlib import Path
from typing import IO
from uuid import uuid4

from herdr_drafts.models import Composer, Draft, DraftConflict, DraftState, Target


class PanelLock:
    def __init__(self, directory: Path, target: Target) -> None:
        self.path = directory / f"panel-{target.lock_key}.lock"
        self._file: IO[str] | None = None

    def acquire(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        file = self.path.open("a+", encoding="utf-8")
        try:
            fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            file.close()
            raise DraftConflict("Un panneau est déjà ouvert pour cet agent.") from exc
        self._file = file

    def release(self) -> None:
        if self._file is not None:
            self._file.close()
            self._file = None


class DraftStore:
    def __init__(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
        os.close(fd)
        path.chmod(0o600)
        self._db = sqlite3.connect(path, timeout=3)
        self._db.row_factory = sqlite3.Row
        self._db.execute("PRAGMA foreign_keys = ON")
        self._db.execute("PRAGMA journal_mode = WAL")
        self._db.executescript("""
            CREATE TABLE IF NOT EXISTS targets (
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
            );
        """)

    def close(self) -> None:
        self._db.close()

    def _register(self, target: Target) -> None:
        self._db.execute(
            "INSERT OR IGNORE INTO targets VALUES (?, ?)", (target.key, target.to_json())
        )

    def recorded_target(self, key: str) -> Target | None:
        row = self._db.execute("SELECT identity FROM targets WHERE key=?", (key,)).fetchone()
        return Target.from_json(row["identity"]) if row else None

    def resolve_target(self, target: Target) -> Target:
        row = self._db.execute(
            "SELECT targets.identity FROM target_aliases "
            "JOIN targets ON targets.key=target_aliases.target_key WHERE alias_key=?",
            (target.key,),
        ).fetchone()
        return Target.from_json(row["identity"]) if row else target

    def composer(self, target: Target) -> Composer:
        row = self._db.execute(
            "SELECT body, editing_id FROM composers WHERE target_key = ?", (target.key,)
        ).fetchone()
        return Composer(row["body"], row["editing_id"]) if row else Composer()

    def save_composer(self, target: Target, body: str, editing_id: str | None = None) -> None:
        with self._db:
            self._register(target)
            self._db.execute(
                "INSERT INTO composers VALUES (?, ?, ?) ON CONFLICT(target_key) "
                "DO UPDATE SET body=excluded.body, editing_id=excluded.editing_id",
                (target.key, body, editing_id),
            )

    def commit_composer(self, target: Target) -> Draft:
        with self._db:
            composer = self.composer(target)
            if not composer.body.strip():
                raise DraftConflict("Le message est vide.")
            self._register(target)
            if composer.editing_id:
                draft_id = composer.editing_id
                cursor = self._db.execute(
                    "UPDATE drafts SET body=?, state='draft', error=NULL "
                    "WHERE id=? AND target_key=? AND state IN ('draft','error')",
                    (composer.body, draft_id, target.key),
                )
                if cursor.rowcount != 1:
                    raise DraftConflict(
                        "Ce message a changé. Conservez votre texte avant de continuer."
                    )
            else:
                draft_id = uuid4().hex
                position = self._db.execute(
                    "SELECT COALESCE(MAX(position), -1)+1 FROM drafts WHERE target_key=?",
                    (target.key,),
                ).fetchone()[0]
                self._db.execute(
                    "INSERT INTO drafts(id,target_key,body,state,position) VALUES(?,?,?,'draft',?)",
                    (draft_id, target.key, composer.body, position),
                )
            self._db.execute("DELETE FROM composers WHERE target_key=?", (target.key,))
        return self.get(draft_id)

    def get(self, draft_id: str) -> Draft:
        row = self._db.execute("SELECT * FROM drafts WHERE id=?", (draft_id,)).fetchone()
        if row is None:
            raise DraftConflict("Ce message n'existe plus.")
        return Draft(
            row["id"], row["target_key"], row["body"], row["state"], row["position"], row["error"]
        )

    def list_drafts(self, target: Target, *, history: bool = False) -> list[Draft]:
        comparison = "=" if history else "!="
        order = "updated DESC, position DESC" if history else "position, id"
        rows = self._db.execute(
            f"SELECT id FROM drafts WHERE target_key=? AND state {comparison} 'sent' "
            f"ORDER BY {order}",
            (target.key,),
        ).fetchall()
        return [self.get(row["id"]) for row in rows]

    def claim(self, draft_id: str) -> Draft:
        with self._db:
            cursor = self._db.execute(
                "UPDATE drafts SET state='sending', error=NULL "
                "WHERE id=? AND state IN ('draft','error')",
                (draft_id,),
            )
            if cursor.rowcount != 1:
                raise DraftConflict("Ce message est déjà transmis ou en cours d'envoi.")
        return self.get(draft_id)

    def finish(self, draft_id: str, state: DraftState, error: str | None = None) -> None:
        with self._db:
            cursor = self._db.execute(
                "UPDATE drafts SET state=?,error=?,updated=strftime('%Y-%m-%dT%H:%M:%fZ','now') "
                "WHERE id=? AND state='sending'",
                (state, error, draft_id),
            )
            if cursor.rowcount != 1:
                raise DraftConflict("L'état de ce message a changé.")

    def resolve(self, draft_id: str, *, delivered: bool) -> None:
        with self._db:
            cursor = self._db.execute(
                "UPDATE drafts SET state=?,error=NULL WHERE id=? AND state='uncertain'",
                ("sent" if delivered else "draft", draft_id),
            )
            if cursor.rowcount != 1:
                raise DraftConflict("Ce message n'attend pas de vérification.")

    def recover_inflight(self, target: Target) -> None:
        with self._db:
            self._db.execute(
                "UPDATE drafts SET state='uncertain',error=? "
                "WHERE target_key=? AND state='sending'",
                ("Envoi interrompu. Vérifiez le terminal avant de réessayer.", target.key),
            )

    def delete(self, draft_id: str) -> None:
        with self._db:
            cursor = self._db.execute(
                "DELETE FROM drafts WHERE id=? AND state IN ('draft','error')", (draft_id,)
            )
            if cursor.rowcount != 1:
                raise DraftConflict("Ce message ne peut pas être supprimé dans son état actuel.")

    def move(self, draft_id: str, direction: int) -> None:
        draft = self.get(draft_id)
        if draft.state not in ("draft", "error"):
            raise DraftConflict("Ce message ne peut pas être déplacé.")
        rows = self._db.execute(
            "SELECT id,position FROM drafts WHERE target_key=? AND state!='sent' "
            "ORDER BY position,id",
            (draft.target_key,),
        ).fetchall()
        index = next(i for i, row in enumerate(rows) if row["id"] == draft_id)
        neighbor = index + (-1 if direction < 0 else 1)
        if not 0 <= neighbor < len(rows):
            return
        with self._db:
            self._db.executemany(
                "UPDATE drafts SET position=? WHERE id=?",
                [(rows[neighbor]["position"], draft_id), (draft.position, rows[neighbor]["id"])],
            )

    def adopt_session(self, previous: Target, current: Target) -> None:
        with self._db:
            self._register(current)
            self._db.execute(
                "UPDATE drafts SET target_key=? WHERE target_key=?", (current.key, previous.key)
            )
            self._db.execute(
                "UPDATE composers SET target_key=? WHERE target_key=?", (current.key, previous.key)
            )
            self._db.execute(
                "INSERT INTO target_aliases VALUES (?, ?) "
                "ON CONFLICT(alias_key) DO UPDATE SET target_key=excluded.target_key",
                (previous.key, current.key),
            )
