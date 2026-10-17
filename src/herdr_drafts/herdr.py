"""Bounded JSON transport for Herdr's local Unix socket API."""

from __future__ import annotations

import json
import socket
import time
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path
from uuid import uuid4

from herdr_drafts.models import Agent, Target

MAX_RESPONSE_BYTES = 1024 * 1024
KNOWN_PROMPT_REJECTIONS = frozenset(
    {
        "agent_blocked",
        "agent_not_found",
        "not_found",
        "invalid_params",
        "invalid_target",
        "agent_not_ready",
        "empty_agent_prompt",
    }
)


class HerdrError(RuntimeError):
    def __init__(self, message: str, *, code: str = "transport", uncertain: bool = False) -> None:
        super().__init__(message)
        self.code = code
        self.uncertain = uncertain


def connect_unix_socket(path: str, timeout: float) -> socket.socket:
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        connection.settimeout(timeout)
        connection.connect(path)
    except OSError:
        connection.close()
        raise
    return connection


def json_object(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise HerdrError("Format de réponse Herdr invalide.", code="invalid_response")
    return {key: item for key, item in value.items()}


def required_string(value: dict[str, object], key: str) -> str:
    item = value.get(key)
    if not isinstance(item, str) or not item:
        raise HerdrError(f"Champ manquant dans la réponse Herdr : {key}.", code="invalid_response")
    return item


class HerdrClient:
    def __init__(
        self,
        socket_path: str,
        *,
        timeout: float = 5,
        connect: Callable[[str, float], socket.socket] = connect_unix_socket,
    ) -> None:
        self.socket_path = str(Path(socket_path).expanduser().absolute())
        self.timeout = timeout
        self._connect = connect

    def request(self, method: str, params: dict[str, object]) -> dict[str, object]:
        request_id = uuid4().hex
        payload = (
            json.dumps({"id": request_id, "method": method, "params": params}, ensure_ascii=False)
            + "\n"
        ).encode()
        submission_started = False
        is_prompt = method == "agent.prompt"
        deadline = time.monotonic() + self.timeout
        try:
            with self._connect(self.socket_path, self.timeout) as connection:
                submission_started = True
                connection.sendall(payload)
                response_bytes = bytearray()
                while b"\n" not in response_bytes:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError("Herdr n'a pas répondu à temps.")
                    connection.settimeout(remaining)
                    chunk = connection.recv(8192)
                    if not chunk:
                        raise ConnectionError("Herdr a fermé la connexion avant sa réponse.")
                    response_bytes.extend(chunk)
                    if len(response_bytes) > MAX_RESPONSE_BYTES:
                        raise HerdrError("Réponse Herdr trop volumineuse.", code="invalid_response")
            decoded: object = json.loads(response_bytes.split(b"\n", 1)[0])
            response = json_object(decoded)
            if response.get("id") != request_id:
                raise HerdrError("Identifiant de réponse Herdr inattendu.", code="invalid_response")
            if "error" in response:
                error = json_object(response["error"])
                code = required_string(error, "code")
                message = required_string(error, "message")
                raise HerdrError(
                    message, code=code, uncertain=is_prompt and code not in KNOWN_PROMPT_REJECTIONS
                )
            return json_object(response.get("result"))
        except HerdrError as exc:
            if is_prompt and submission_started and exc.code == "invalid_response":
                exc.uncertain = True
            raise
        except (OSError, ConnectionError, UnicodeError, ValueError) as exc:
            raise HerdrError(
                "Connexion Herdr indisponible ou réponse interrompue.",
                uncertain=is_prompt and submission_started,
            ) from exc

    def _parse_agent(self, value: object) -> Agent:
        info = json_object(value)
        session = info.get("agent_session")
        native_session = None
        if session is not None:
            reference = json_object(session)
            native_session = json.dumps(
                [required_string(reference, key) for key in ("source", "kind", "value")]
            )
        cwd = info.get("foreground_cwd") or info.get("cwd") or ""
        name = (
            info.get("name")
            or info.get("title")
            or info.get("terminal_title_stripped")
            or info.get("display_agent")
            or info.get("agent")
        )
        if not isinstance(cwd, str) or not isinstance(name, str):
            raise HerdrError("Identité d'agent Herdr invalide.", code="invalid_response")
        target = Target(
            self.socket_path,
            required_string(info, "pane_id"),
            required_string(info, "terminal_id"),
            required_string(info, "workspace_id"),
            required_string(info, "agent"),
            cwd,
            native_session,
        )
        return Agent(target, required_string(info, "agent_status"), name)

    def get_agent(self, pane_id: str) -> Agent:
        result = self.request("agent.get", {"target": pane_id})
        return self._parse_agent(result.get("agent"))

    def list_agents(self) -> list[Agent]:
        snapshot = json_object(self.request("session.snapshot", {}).get("snapshot"))
        values = snapshot.get("agents")
        if not isinstance(values, list):
            raise HerdrError("Liste d'agents Herdr invalide.", code="invalid_response")
        labels: dict[str, dict[str, str]] = {}
        for collection, key in (("workspaces", "workspace_id"), ("tabs", "tab_id")):
            records = snapshot.get(collection)
            if not isinstance(records, list):
                raise HerdrError("Contexte des agents Herdr invalide.", code="invalid_response")
            labels[collection] = {}
            for value in records:
                record = json_object(value)
                label = record.get("label")
                if not isinstance(label, str):
                    raise HerdrError("Nom de projet Herdr invalide.", code="invalid_response")
                labels[collection][required_string(record, key)] = label
        agents = []
        for value in values:
            agent = self._parse_agent(value)
            info = json_object(value)
            agents.append(
                replace(
                    agent,
                    workspace_label=labels["workspaces"].get(agent.target.workspace_id, ""),
                    tab_label=labels["tabs"].get(required_string(info, "tab_id"), ""),
                )
            )
        return agents

    def prompt(self, pane_id: str, body: str) -> None:
        result = self.request("agent.prompt", {"target": pane_id, "text": body})
        if result.get("type") != "agent_prompted":
            raise HerdrError("Confirmation d'envoi Herdr inattendue.", uncertain=True)
