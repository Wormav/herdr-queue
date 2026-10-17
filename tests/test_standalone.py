from dataclasses import replace

from herdr_drafts.__main__ import main, run_terminal, state_directory
from herdr_drafts.app import DraftsApp
from herdr_drafts.herdr import HerdrClient
from herdr_drafts.picker import AgentPicker
from herdr_drafts.service import DraftService
from herdr_drafts.store import DraftStore


def test_default_command_chooses_an_agent_and_runs_locally(fake_herdr, monkeypatch, tmp_path):
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    monkeypatch.setattr("herdr_drafts.__main__.HerdrClient", lambda *a, **k: client)
    monkeypatch.setenv("HERDR_SOCKET_PATH", fake_herdr.path)
    monkeypatch.setenv("HERDR_PLUGIN_STATE_DIR", str(tmp_path))
    monkeypatch.delenv("HERDR_ENV", raising=False)
    monkeypatch.delenv("HERDR_PANE_ID", raising=False)
    opened = []

    def choose(self):
        agents, errors = self.load()
        assert not errors
        return agents[0]

    def run(self):
        opened.append(self.service.target)
        assert self.panel_id is None
        return False

    monkeypatch.setattr(AgentPicker, "run", choose)
    monkeypatch.setattr(DraftsApp, "run", run)
    assert main([]) == 0
    assert len(opened) == 1
    assert opened[0].terminal_id == "term-1"
    assert not any(r["method"].startswith(("plugin.", "pane.")) for r in fake_herdr.requests)
    assert fake_herdr.prompts == []


def test_switching_agents_keeps_each_composer_and_never_closes_the_launch_terminal(
    fake_herdr, monkeypatch, tmp_path
):
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    first = client.get_agent("w1:p1")
    second = replace(first, target=replace(first.target, pane_id="w1:p2", terminal_id="term-2"))
    chosen = iter((first, second, None))
    monkeypatch.setattr(AgentPicker, "run", lambda self: next(chosen))
    monkeypatch.setattr("herdr_drafts.__main__.HerdrClient", lambda *a, **k: client)

    def run(self):
        self.service.store.save_composer(
            self.service.target, "texte pour " + self.service.target.pane_id
        )
        return True

    monkeypatch.setattr(DraftsApp, "run", run)
    run_terminal(
        {
            "HERDR_SOCKET_PATH": fake_herdr.path,
            "HERDR_PANE_ID": "w1:p99",
            "HERDR_PLUGIN_STATE_DIR": str(tmp_path),
        }
    )
    store = DraftStore(tmp_path / "drafts.sqlite3")
    assert store.composer(first.target).body == "texte pour w1:p1"
    assert store.composer(second.target).body == "texte pour w1:p2"
    store.close()
    assert not any(r["method"].startswith(("plugin.", "pane.")) for r in fake_herdr.requests)


def test_default_state_directory_is_shared_with_the_integrated_plugin():
    assert str(state_directory({"XDG_STATE_HOME": "/tmp/state"})) == (
        "/tmp/state/herdr/plugins/herdr-drafts"
    )


async def test_agents_button_saves_the_composer_before_returning_to_the_picker(
    fake_herdr, tmp_path
):
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    store = DraftStore(tmp_path / "drafts.sqlite3")
    service = DraftService(store, client, client.get_agent("w1:p1").target)
    app = DraftsApp(service, allow_agent_switch=True)
    async with app.run_test(size=(48, 32)) as pilot:
        await pilot.press("e", "n", "c", "o", "u", "r", "s")
        await pilot.click("#switch-agent")
        await pilot.pause()
        assert app.return_value is True
        assert store.composer(service.target).body == "encours"
    store.close()
