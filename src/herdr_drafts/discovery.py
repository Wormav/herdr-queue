"""Discover local Herdr servers and read their agent inventories."""

from __future__ import annotations

import json
import subprocess
from collections.abc import Mapping, Sequence
from dataclasses import replace
from pathlib import Path

from herdr_drafts.herdr import HerdrClient, HerdrError, json_object, required_string
from herdr_drafts.models import Agent, HerdrSession


def discover_sessions(
    environment: Mapping[str, str],
    *,
    session_name: str | None = None,
    socket_path: str | None = None,
) -> list[HerdrSession]:
    if socket_path:
        return [HerdrSession("socket explicite", str(Path(socket_path).expanduser().absolute()))]
    inherited = environment.get("HERDR_SOCKET_PATH")
    if inherited and not session_name:
        return [HerdrSession(environment.get("HERDR_SESSION", "session courante"), inherited)]
    binary = environment.get("HERDR_BIN_PATH", "herdr")
    try:
        result = subprocess.run(
            [binary, "session", "list", "--json"], capture_output=True, text=True, timeout=5
        )
    except FileNotFoundError as exc:
        raise ValueError(
            "Herdr est introuvable. Vérifiez qu'il est installé dans votre PATH."
        ) from exc
    except subprocess.TimeoutExpired as exc:
        raise ValueError("Herdr ne répond pas. Réessayez avec Ctrl+R.") from exc
    if result.returncode:
        raise ValueError("Impossible de lire les sessions Herdr. Vérifiez que Herdr fonctionne.")
    try:
        registry = json_object(json.loads(result.stdout))
        records = registry.get("sessions")
        if not isinstance(records, list):
            raise ValueError("La liste des sessions Herdr est invalide.")
        sessions = []
        for value in records:
            record = json_object(value)
            name = required_string(record, "name")
            path = required_string(record, "socket_path")
            if session_name is not None:
                if name == session_name:
                    return [HerdrSession(name, path)]
            elif record.get("running") is True or Path(path).exists():
                sessions.append(HerdrSession(name, path))
    except (HerdrError, json.JSONDecodeError) as exc:
        raise ValueError("La liste des sessions Herdr est invalide.") from exc
    if session_name is not None:
        raise ValueError(f"Session Herdr introuvable : {session_name}.")
    return sessions


def load_agents(
    sources: Sequence[tuple[HerdrSession, HerdrClient]],
) -> tuple[list[Agent], list[str]]:
    agents: list[Agent] = []
    errors: list[str] = []
    for session, client in sources:
        try:
            agents.extend(
                replace(agent, session_label=session.name) for agent in client.list_agents()
            )
        except HerdrError as exc:
            errors.append(f"{session.name} : {exc}")
    return agents, errors
