from textual.widgets import Button, Input, OptionList, Static

from herdr_drafts.models import Agent, Target
from herdr_drafts.picker import AgentPicker


def agent(name, workspace, pane, kind="codex", state="working", socket="/tmp/h.sock"):
    target = Target(
        socket, pane, "term-" + pane, workspace, kind, "/projects/" + workspace, "session-" + pane
    )
    return Agent(target, state, name, workspace_label=workspace, tab_label="main")


async def test_choosing_an_agent_returns_its_identity_without_opening_a_herdr_pane():
    first = agent("Corriger les tests", "Forge", "w1:p1")
    second = agent("Préparer les matchs", "OpenGambit", "w2:p1", "claude", "idle")
    app = AgentPicker(lambda: ([first, second], []))
    async with app.run_test(size=(80, 32)) as pilot:
        await app.workers.wait_for_complete()
        await pilot.pause()
        choices = app.query_one("#agents", OptionList)
        assert choices.option_count == 2
        choices.focus()
        await pilot.press("down", "enter")
        await pilot.pause()
        assert app.return_value == second


async def test_search_matches_project_name_and_agent_type():
    first = agent("Corriger les tests", "Forge", "w1:p1")
    second = agent("Préparer les matchs", "OpenGambit", "w2:p1", "claude")
    app = AgentPicker(lambda: ([first, second], []))
    async with app.run_test(size=(80, 32)) as pilot:
        await app.workers.wait_for_complete()
        app.query_one("#search", Input).value = "opengambit"
        await pilot.pause()
        assert app.query_one("#agents", OptionList).option_count == 1
        await pilot.click("#choose-agent")
        await pilot.pause()
        assert app.return_value == second


async def test_agents_with_the_same_pane_id_on_different_servers_remain_distinct():
    first = agent("Premier", "Forge", "w1:p1", socket="/tmp/a.sock")
    second = agent("Second", "Forge", "w1:p1", socket="/tmp/b.sock")
    app = AgentPicker(lambda: ([first, second], []))
    async with app.run_test() as pilot:
        await app.workers.wait_for_complete()
        choices = app.query_one("#agents", OptionList)
        assert choices.get_option_at_index(0).id != choices.get_option_at_index(1).id
        choices.focus()
        await pilot.press("down", "enter")
        await pilot.pause()
        assert app.return_value.target.socket_path == "/tmp/b.sock"


async def test_refresh_preserves_the_highlighted_identity_when_order_changes():
    first = agent("Premier", "Forge", "w1:p1")
    second = agent("Second", "Forge", "w1:p2")
    inventory = [first, second]
    app = AgentPicker(lambda: (list(inventory), []))
    async with app.run_test() as pilot:
        await app.workers.wait_for_complete()
        choices = app.query_one("#agents", OptionList)
        choices.highlighted = 1
        inventory[:] = [second, first]
        app.action_refresh()
        await app.workers.wait_for_complete()
        await pilot.pause()
        assert choices.get_option_at_index(choices.highlighted).id == second.target.key


async def test_disappeared_agents_cannot_be_selected():
    inventory = [agent("Premier", "Forge", "w1:p1")]
    app = AgentPicker(lambda: (list(inventory), []))
    async with app.run_test() as pilot:
        await app.workers.wait_for_complete()
        inventory.clear()
        app.action_refresh()
        await app.workers.wait_for_complete()
        await pilot.pause()
        assert app.query_one("#choose-agent", Button).disabled
        assert app.query_one("#agents", OptionList).option_count == 0
        assert app.return_value is None


async def test_errors_are_visible_and_do_not_prevent_a_retry():
    def failing():
        raise ValueError("Herdr est indisponible")

    app = AgentPicker(failing)
    async with app.run_test() as pilot:
        await app.workers.wait_for_complete()
        await pilot.pause()
        assert "indisponible" in str(app.query_one("#picker-status", Static).render())
        assert app.query_one("#choose-agent", Button).disabled


async def test_enter_in_search_opens_the_filtered_agent():
    first = agent("Premier", "Forge", "w1:p1")
    app = AgentPicker(lambda: ([first], []))
    async with app.run_test() as pilot:
        await app.workers.wait_for_complete()
        await pilot.pause()
        assert app.return_value is None
        app.query_one("#search", Input).focus()
        await pilot.press("enter")
        await pilot.pause()
        assert app.return_value == first
