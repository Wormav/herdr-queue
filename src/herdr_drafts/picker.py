"""Choose a live agent without creating or moving a Herdr pane."""

from __future__ import annotations

import asyncio
from collections.abc import Callable

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal
from textual.widgets import Button, Footer, Input, OptionList, Static
from textual.widgets.option_list import Option

from herdr_drafts.app import STATUS_LABELS
from herdr_drafts.herdr import HerdrError
from herdr_drafts.models import Agent


class AgentPicker(App[Agent | None]):
    TITLE = "Choisir un agent Herdr"
    CSS_PATH = ["app.tcss", "picker.tcss"]
    BINDINGS = [
        Binding("ctrl+q,escape", "quit", "Fermer", priority=True),
        Binding("ctrl+r", "refresh", "Actualiser"),
        Binding("ctrl+f", "search", "Rechercher"),
        Binding("down", "focus_agents", show=False),
    ]

    def __init__(
        self,
        load: Callable[[], tuple[list[Agent], list[str]]],
        *,
        notice: str = "",
    ) -> None:
        super().__init__()
        self.load = load
        self.notice = notice
        self._agents: list[Agent] = []
        self._visible: dict[str, Agent] = {}
        self._errors: list[str] = []
        self._loading = False

    def compose(self) -> ComposeResult:
        yield Static("Agents Herdr", id="title")
        yield Static(
            "Choisissez un agent pour préparer ses messages dans ce terminal.", id="picker-help"
        )
        yield Static(self.notice, id="picker-notice", markup=False)
        yield Input(placeholder="Rechercher un projet, un nom ou un agent…", id="search")
        yield Static("Recherche des agents…", id="picker-status", markup=False)
        yield OptionList(id="agents", markup=False)
        with Horizontal(id="picker-actions", classes="actions"):
            yield Button(
                "Ouvrir", id="choose-agent", variant="primary", disabled=True, compact=True
            )
            yield Button("Actualiser", id="refresh-agents", compact=True)
        yield Footer()

    def on_mount(self) -> None:
        self.theme = "textual-dark"
        self.query_one("#picker-notice", Static).display = bool(self.notice)
        self.query_one("#search", Input).focus()
        self.set_interval(5, self.action_refresh)
        self.action_refresh()

    def action_refresh(self) -> None:
        if self._loading:
            return
        self._loading = True
        self.query_one("#refresh-agents", Button).disabled = True
        self.run_worker(self._refresh_agents(), group="inventory")

    async def _refresh_agents(self) -> None:
        try:
            self._agents, self._errors = await asyncio.to_thread(self.load)
        except (HerdrError, ValueError, OSError) as exc:
            self._agents = []
            self._errors = [str(exc)]
        finally:
            self._loading = False
            self._render_agents()
            self.query_one("#refresh-agents", Button).disabled = False

    def _render_agents(self) -> None:
        choices = self.query_one("#agents", OptionList)
        selected = (
            choices.get_option_at_index(choices.highlighted).id
            if choices.highlighted is not None
            else None
        )
        query = self.query_one("#search", Input).value.casefold().strip()
        agents = [
            agent
            for agent in self._agents
            if query
            in " ".join(
                (
                    agent.name,
                    agent.target.agent,
                    agent.workspace_label,
                    agent.tab_label,
                    agent.session_label,
                    agent.target.cwd,
                    agent.target.pane_id,
                )
            ).casefold()
        ]
        self._visible = {agent.target.key: agent for agent in agents}
        choices.clear_options()
        options = []
        for agent in agents:
            project = agent.workspace_label or agent.target.workspace_id
            state = STATUS_LABELS.get(agent.status, "État inconnu")
            session = f"  /  {agent.session_label}" if agent.session_label else ""
            prompt = (
                f"{project}  /  {agent.target.agent}  /  {state}{session}\n"
                f"{agent.name}\n{agent.target.cwd}  /  {agent.target.pane_id}"
            )
            options.append(Option(prompt, id=agent.target.key))
        choices.add_options(options)
        if agents:
            keys = list(self._visible)
            choices.highlighted = keys.index(selected) if selected in keys else 0
        self.query_one("#choose-agent", Button).disabled = not agents
        if agents:
            message = f"{len(agents)} agent(s) disponible(s). Cliquez ou appuyez sur Entrée."
        elif self._agents:
            message = "Aucun agent ne correspond à la recherche."
        elif self._errors:
            message = "Connexion à Herdr impossible."
        else:
            message = "Aucun agent trouvé. Lancez un agent dans Herdr, puis actualisez."
        if self._errors:
            message += "\n" + "\n".join(self._errors[:2])
        self.query_one("#picker-status", Static).update(message)

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "search":
            self._render_agents()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "search":
            self._choose_highlighted()

    def on_option_list_option_selected(self, event: OptionList.OptionSelected) -> None:
        if event.option_id in self._visible:
            self.exit(self._visible[event.option_id])

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "choose-agent":
            self._choose_highlighted()
        elif event.button.id == "refresh-agents":
            self.action_refresh()

    def _choose_highlighted(self) -> None:
        choices = self.query_one("#agents", OptionList)
        if choices.highlighted is not None:
            key = choices.get_option_at_index(choices.highlighted).id
            if key in self._visible:
                self.exit(self._visible[key])

    def action_focus_agents(self) -> None:
        self.query_one("#agents", OptionList).focus()

    def action_search(self) -> None:
        self.query_one("#search", Input).focus()

    async def action_quit(self) -> None:
        self.exit(None)
