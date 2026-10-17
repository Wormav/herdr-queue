"""Concrete values shared by the queue, storage, and Herdr transport."""

from __future__ import annotations

import hashlib
import json
from dataclasses import asdict, dataclass
from typing import Literal

DraftState = Literal["draft", "sending", "sent", "error", "uncertain"]


class DraftConflict(ValueError):
    """A message changed or is unavailable for the requested operation."""


@dataclass(frozen=True)
class Target:
    socket_path: str
    pane_id: str
    terminal_id: str
    workspace_id: str
    agent: str
    cwd: str
    session: str | None

    @property
    def key(self) -> str:
        identity = [self.socket_path, self.terminal_id, self.agent, self.session]
        return hashlib.sha256(json.dumps(identity).encode()).hexdigest()

    @property
    def lock_key(self) -> str:
        identity = [self.socket_path, self.terminal_id]
        return hashlib.sha256(json.dumps(identity).encode()).hexdigest()

    def to_json(self) -> str:
        return json.dumps(asdict(self), ensure_ascii=False)

    @classmethod
    def from_json(cls, raw: str) -> Target:
        value: object = json.loads(raw)
        if not isinstance(value, dict):
            raise ValueError("Destination invalide.")
        names = ("socket_path", "pane_id", "terminal_id", "workspace_id", "agent", "cwd")
        strings = []
        for name in names:
            item = value.get(name)
            if not isinstance(item, str) or (not item and name != "cwd"):
                raise ValueError(f"Destination invalide : {name}.")
            strings.append(item)
        session = value.get("session")
        if session is not None and not isinstance(session, str):
            raise ValueError("Référence de conversation invalide.")
        return cls(
            socket_path=strings[0],
            pane_id=strings[1],
            terminal_id=strings[2],
            workspace_id=strings[3],
            agent=strings[4],
            cwd=strings[5],
            session=session,
        )


@dataclass(frozen=True)
class Agent:
    target: Target
    status: str
    name: str
    workspace_label: str = ""
    tab_label: str = ""
    session_label: str = ""


@dataclass(frozen=True)
class HerdrSession:
    name: str
    socket_path: str


@dataclass(frozen=True)
class Composer:
    body: str = ""
    editing_id: str | None = None


@dataclass(frozen=True)
class Draft:
    id: str
    target_key: str
    body: str
    state: DraftState
    position: int
    error: str | None
