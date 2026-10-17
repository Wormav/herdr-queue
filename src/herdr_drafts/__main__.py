"""Herdr plugin entrypoints and an isolated, network-free demonstration."""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import subprocess
import sys
from collections.abc import Mapping, Sequence
from contextlib import suppress
from importlib.metadata import version
from pathlib import Path
from tempfile import TemporaryDirectory

from herdr_drafts import __version__
from herdr_drafts.app import DraftsApp
from herdr_drafts.discovery import discover_sessions, load_agents
from herdr_drafts.herdr import HerdrClient, HerdrError, json_object, required_string
from herdr_drafts.models import Agent, DraftConflict, Target
from herdr_drafts.picker import AgentPicker
from herdr_drafts.service import DraftService
from herdr_drafts.store import DraftStore, PanelLock

PLUGIN_ID = "herdr-drafts"


def source_pane(environment: Mapping[str, str]) -> str:
    if environment.get("HERDR_ENV") != "1":
        raise ValueError("Ouvrez ce panneau depuis un agent dans Herdr, ou utilisez « demo ».")
    raw = environment.get("HERDR_PLUGIN_CONTEXT_JSON")
    if raw:
        context = json_object(json.loads(raw))
        pane = context.get("focused_pane_id")
        if isinstance(pane, str) and pane:
            return pane
    pane = environment.get("HERDR_PANE_ID")
    if not pane:
        raise ValueError("Le panneau source Herdr n'est pas identifié.")
    return pane


def state_directory(environment: Mapping[str, str]) -> Path:
    plugin_state = environment.get("HERDR_PLUGIN_STATE_DIR")
    if plugin_state:
        return Path(plugin_state).expanduser()
    state_home = environment.get("XDG_STATE_HOME")
    return (
        (Path(state_home).expanduser() if state_home else Path.home() / ".local/state")
        / "herdr"
        / "plugins"
        / PLUGIN_ID
    )


def open_panel(client: HerdrClient, source: str) -> str:
    source_info = json_object(client.request("pane.get", {"pane_id": source}).get("pane"))
    tokens = json_object(source_info.get("tokens") or {})
    if tokens.get("drafts_owner"):
        client.request("plugin.pane.focus", {"pane_id": source})
        return source
    target = client.get_agent(source).target
    panes = client.request("pane.list", {"workspace_id": target.workspace_id}).get("panes")
    if not isinstance(panes, list):
        raise HerdrError("Liste de panneaux Herdr invalide.")
    for value in panes:
        pane = json_object(value)
        metadata = json_object(pane.get("tokens") or {})
        if metadata.get("drafts_owner") == target.lock_key:
            if metadata.get("drafts_target") != target.key:
                raise DraftConflict(
                    "Un panneau est lié à une autre conversation. "
                    "Fermez-le avec Ctrl+Q, puis rouvrez les brouillons."
                )
            pane_id = required_string(pane, "pane_id")
            client.request("plugin.pane.focus", {"pane_id": pane_id})
            return pane_id
    result = client.request(
        "plugin.pane.open",
        {
            "plugin_id": PLUGIN_ID,
            "entrypoint": "drafts",
            "placement": "split",
            "target_pane_id": target.pane_id,
            "direction": "right",
            "focus": True,
            "env": {"HERDR_DRAFTS_TARGET": target.to_json()},
        },
    )
    opened = json_object(result.get("plugin_pane"))
    return required_string(json_object(opened.get("pane")), "pane_id")


class DemoClient(HerdrClient):
    """Simulate submission locally without touching any running Herdr agent."""

    def __init__(self, target: Target) -> None:
        super().__init__(target.socket_path)
        self.target = target

    def get_agent(self, pane_id: str) -> Agent:
        return Agent(self.target, "working", "Agent de démonstration")

    def prompt(self, pane_id: str, body: str) -> None:
        pass


def run_demo() -> None:
    target = Target("/demo", "démo", "demo-terminal", "demo", "codex", "~/mon-projet", "demo")
    with TemporaryDirectory(prefix="herdr-drafts-demo-") as directory:
        store = DraftStore(Path(directory) / "drafts.sqlite3")
        try:
            service = DraftService(store, DemoClient(target), target)
            for body in (
                "Quand les tests seront terminés, vérifie aussi le comportement sur mobile.",
                "Prépare un résumé des changements\net des points à tester ensemble.",
            ):
                store.save_composer(target, body)
                service.save()
            DraftsApp(service, demo=True).run()
        finally:
            store.close()


def run_drafts(
    client: HerdrClient,
    target: Target,
    directory: Path,
    *,
    panel_id: str | None = None,
    allow_agent_switch: bool = False,
) -> bool:
    lock = PanelLock(directory, target)
    lock.acquire()
    try:
        store = DraftStore(directory / "drafts.sqlite3")
        try:
            service = DraftService(store, client, target)
            store.recover_inflight(service.target)
            app = DraftsApp(service, panel_id=panel_id, allow_agent_switch=allow_agent_switch)
            return app.run() is True
        finally:
            store.close()
    finally:
        lock.release()


def run_panel(environment: Mapping[str, str]) -> None:
    raw = environment.get("HERDR_DRAFTS_TARGET")
    if not raw:
        raise ValueError(
            "Destination manquante. Lancez l'action « Ouvrir les brouillons » dans Herdr."
        )
    target = Target.from_json(raw)
    socket_path = environment.get("HERDR_SOCKET_PATH")
    if socket_path != target.socket_path:
        raise ValueError("Le socket Herdr ne correspond pas à la destination enregistrée.")
    panel_id = environment.get("HERDR_PANE_ID")
    client = HerdrClient(socket_path)
    run_drafts(client, target, state_directory(environment), panel_id=panel_id)
    if panel_id and panel_id != target.pane_id:
        with suppress(HerdrError):
            client.request("plugin.pane.close", {"pane_id": panel_id})


def run_terminal(
    environment: Mapping[str, str],
    *,
    session_name: str | None = None,
    socket_path: str | None = None,
) -> None:
    def available_agents() -> tuple[list[Agent], list[str]]:
        sessions = discover_sessions(
            environment, session_name=session_name, socket_path=socket_path
        )
        sources = [(session, HerdrClient(session.socket_path, timeout=3)) for session in sessions]
        return load_agents(sources)

    notice = ""
    while True:
        chosen = AgentPicker(available_agents, notice=notice).run()
        if chosen is None:
            return
        try:
            switching = run_drafts(
                HerdrClient(chosen.target.socket_path),
                chosen.target,
                state_directory(environment),
                allow_agent_switch=True,
            )
        except DraftConflict as exc:
            notice = str(exc)
            continue
        if not switching:
            return
        notice = ""


def doctor(environment: Mapping[str, str]) -> None:
    binary = environment.get("HERDR_BIN_PATH", "herdr")
    result = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=5)
    if result.returncode:
        raise ValueError("Herdr n'a pas pu communiquer sa version.")
    print(f"Herdr Drafts {__version__}")
    print(result.stdout.strip())
    print(f"Python {sys.version.split()[0]} / Textual {version('textual')}")
    print(f"Contexte Herdr : {'oui' if environment.get('HERDR_ENV') == '1' else 'non'}")
    print(f"Données locales : {state_directory(environment)}")
    print("Diagnostic local uniquement ; aucun message envoyé.")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Préparez plusieurs messages pour votre agent Herdr."
    )
    parser.add_argument("--version", action="version", version=__version__)
    parser.add_argument(
        "command",
        choices=("connect", "open", "tui", "demo", "doctor"),
        nargs="?",
        default="connect",
    )
    connection = parser.add_mutually_exclusive_group()
    connection.add_argument("--session", help="Limiter le choix à une session Herdr locale")
    connection.add_argument("--socket", help="Utiliser un socket Herdr précis")
    args = parser.parse_args(argv)
    if args.command != "connect" and (args.session or args.socket):
        parser.error("--session et --socket s'utilisent avec connect.")
    try:
        if args.command == "connect":
            run_terminal(os.environ, session_name=args.session, socket_path=args.socket)
        elif args.command == "demo":
            run_demo()
        elif args.command == "doctor":
            doctor(os.environ)
        elif args.command == "tui":
            run_panel(os.environ)
        else:
            source = source_pane(os.environ)
            socket_path = os.environ.get("HERDR_SOCKET_PATH")
            if not socket_path:
                raise ValueError("Le contexte Herdr ne fournit pas de socket.")
            pane_id = open_panel(HerdrClient(socket_path), source)
            print(f"Brouillons ouverts : {pane_id}")
    except (OSError, ValueError, HerdrError, sqlite3.Error, subprocess.SubprocessError) as exc:
        print(f"Herdr Drafts : {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
