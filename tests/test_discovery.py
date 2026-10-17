import json
from subprocess import CompletedProcess

import pytest

from herdr_drafts.discovery import discover_sessions, load_agents
from herdr_drafts.herdr import HerdrClient
from herdr_drafts.models import HerdrSession


def test_external_terminal_discovers_sessions_without_herdr_environment(monkeypatch):
    payload = {
        "sessions": [
            {"name": "default", "socket_path": "/tmp/default.sock", "running": True},
            {"name": "work", "socket_path": "/tmp/work.sock", "running": True},
            {"name": "stopped", "socket_path": "/tmp/stopped.sock", "running": False},
        ]
    }
    monkeypatch.setattr(
        "herdr_drafts.discovery.subprocess.run",
        lambda *a, **k: CompletedProcess(a[0], 0, json.dumps(payload), ""),
    )
    assert discover_sessions({}) == [
        HerdrSession("default", "/tmp/default.sock"),
        HerdrSession("work", "/tmp/work.sock"),
    ]


def test_explicit_socket_never_falls_back_to_another_session():
    assert discover_sessions({}, socket_path="/tmp/chosen.sock") == [
        HerdrSession("socket explicite", "/tmp/chosen.sock")
    ]
    assert discover_sessions(
        {"HERDR_SOCKET_PATH": "/tmp/current.sock", "HERDR_SESSION": "work"}
    ) == [HerdrSession("work", "/tmp/current.sock")]


def test_named_session_is_resolved_by_the_cli_registry(monkeypatch):
    payload = {
        "sessions": [
            {"name": "default", "socket_path": "/tmp/default.sock", "running": True},
            {"name": "work", "socket_path": "/tmp/work.sock", "running": True},
        ]
    }
    monkeypatch.setattr(
        "herdr_drafts.discovery.subprocess.run",
        lambda *a, **k: CompletedProcess(a[0], 0, json.dumps(payload), ""),
    )
    assert discover_sessions({"HERDR_SOCKET_PATH": "/tmp/current.sock"}, session_name="work") == [
        HerdrSession("work", "/tmp/work.sock")
    ]
    with pytest.raises(ValueError, match="introuvable"):
        discover_sessions({}, session_name="missing")


def test_existing_socket_with_an_unreliable_running_flag_is_still_inspected(monkeypatch, tmp_path):
    path = tmp_path / "server.sock"
    path.touch()
    payload = {"sessions": [{"name": "default", "socket_path": str(path), "running": False}]}
    monkeypatch.setattr(
        "herdr_drafts.discovery.subprocess.run",
        lambda *a, **k: CompletedProcess(a[0], 0, json.dumps(payload), ""),
    )
    assert discover_sessions({}) == [HerdrSession("default", str(path))]


@pytest.mark.parametrize("stdout", ["invalid json", "{}", '{"sessions":{}}'])
def test_bad_session_registry_is_reported(monkeypatch, stdout):
    monkeypatch.setattr(
        "herdr_drafts.discovery.subprocess.run",
        lambda *a, **k: CompletedProcess(a[0], 0, stdout, ""),
    )
    with pytest.raises(ValueError):
        discover_sessions({})


def test_unavailable_sessions_do_not_hide_agents_from_a_working_one(fake_herdr, tmp_path):
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    sources = [
        (HerdrSession("working", fake_herdr.path), client),
        (
            HerdrSession("offline", str(tmp_path / "absent.sock")),
            HerdrClient(str(tmp_path / "absent.sock")),
        ),
    ]
    agents, errors = load_agents(sources)
    assert len(agents) == 1
    assert agents[0].session_label == "working"
    assert len(errors) == 1 and "offline" in errors[0]
    assert fake_herdr.prompts == []


def test_snapshot_enriches_names_projects_and_tabs(fake_herdr):
    fake_herdr.agent["name"] = None
    fake_herdr.agent["title"] = "Créer le plugin de messages"
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    agent = client.list_agents()[0]
    assert agent.workspace_label == "Projet test"
    assert agent.tab_label == "Développement"
    assert agent.name == "Créer le plugin de messages"
    assert agent.target.terminal_id == "term-1"
    assert fake_herdr.requests[-1]["method"] == "session.snapshot"
