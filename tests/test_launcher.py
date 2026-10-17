import json
import tomllib
from pathlib import Path

import pytest

from herdr_drafts.__main__ import PLUGIN_ID, open_panel, source_pane
from herdr_drafts.herdr import HerdrClient


def test_source_is_explicit_and_does_not_follow_global_focus():
    assert source_pane({"HERDR_ENV": "1", "HERDR_PANE_ID": "w1:p1"}) == "w1:p1"
    assert (
        source_pane(
            {
                "HERDR_ENV": "1",
                "HERDR_PANE_ID": "old-context",
                "HERDR_PLUGIN_CONTEXT_JSON": json.dumps({"focused_pane_id": "w2:p4"}),
            }
        )
        == "w2:p4"
    )
    with pytest.raises(ValueError, match="Herdr"):
        source_pane({})
    with pytest.raises(ValueError):
        source_pane({"HERDR_ENV": "1"})


def test_launcher_opens_to_the_right_and_passes_the_pinned_target(fake_herdr):
    base = fake_herdr.respond

    def respond(request):
        if request["method"] == "pane.list":
            return {"result": {"type": "pane_list", "panes": []}}
        if request["method"] == "pane.get":
            return {"result": {"type": "pane_info", "pane": fake_herdr.agent}}
        if request["method"] == "plugin.pane.open":
            return {
                "result": {
                    "type": "plugin_pane_opened",
                    "plugin_pane": {
                        "plugin_id": PLUGIN_ID,
                        "entrypoint": "drafts",
                        "pane": {"pane_id": "w1:p2"},
                    },
                }
            }
        return base(request)

    fake_herdr.handler = respond
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    assert open_panel(client, "w1:p1") == "w1:p2"
    request = next(r for r in fake_herdr.requests if r["method"] == "plugin.pane.open")
    params = request["params"]
    assert params["placement"] == "split"
    assert params["direction"] == "right"
    assert params["target_pane_id"] == "w1:p1"
    assert params["focus"] is True
    target = json.loads(params["env"]["HERDR_DRAFTS_TARGET"])
    assert target["terminal_id"] == "term-1"
    assert target["session"] == '["test", "id", "s1"]'


def test_an_existing_plugin_pane_is_focused_instead_of_duplicated(fake_herdr):
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    target = client.get_agent("w1:p1").target
    base = fake_herdr.respond

    def respond(request):
        if request["method"] == "pane.get":
            return {"result": {"type": "pane_info", "pane": fake_herdr.agent}}
        if request["method"] == "pane.list":
            return {
                "result": {
                    "type": "pane_list",
                    "panes": [
                        {
                            "pane_id": "w1:p9",
                            "tokens": {
                                "drafts_owner": target.lock_key,
                                "drafts_target": target.key,
                            },
                        }
                    ],
                }
            }
        return base(request)

    fake_herdr.handler = respond
    assert open_panel(client, "w1:p1") == "w1:p9"
    assert not any(r["method"] == "plugin.pane.open" for r in fake_herdr.requests)
    assert fake_herdr.requests[-1]["method"] == "plugin.pane.focus"


def test_reopen_does_not_refocus_a_panel_bound_to_a_replaced_conversation(fake_herdr):
    from dataclasses import replace

    from herdr_drafts.models import DraftConflict

    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    target = client.get_agent("w1:p1").target
    old = replace(target, session="old-session")
    base = fake_herdr.respond

    def respond(request):
        if request["method"] == "pane.get":
            return {"result": {"type": "pane_info", "pane": fake_herdr.agent}}
        if request["method"] == "pane.list":
            return {
                "result": {
                    "type": "pane_list",
                    "panes": [
                        {
                            "pane_id": "w1:p9",
                            "tokens": {"drafts_owner": old.lock_key, "drafts_target": old.key},
                        }
                    ],
                }
            }
        return base(request)

    fake_herdr.handler = respond
    with pytest.raises(DraftConflict, match="Fermez"):
        open_panel(client, "w1:p1")
    assert not any(r["method"] == "plugin.pane.focus" for r in fake_herdr.requests)


def test_plugin_manifest_uses_only_supported_fields_and_owned_commands():
    root = Path(__file__).resolve().parents[1]
    manifest = tomllib.loads((root / "herdr-plugin.toml").read_text())
    assert manifest["id"] == PLUGIN_ID
    assert manifest["min_herdr_version"] == "0.9.3"
    assert manifest["actions"][0]["command"] == ["sh", "run.sh", "open"]
    assert manifest["panes"][0]["placement"] == "split"
    assert manifest["panes"][0]["command"] == ["sh", "run.sh", "tui"]
