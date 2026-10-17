"""Mouse and keyboard interface for composing and manually sending prompts."""

from __future__ import annotations

import asyncio
import sqlite3
import sys
from collections.abc import Sequence

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.events import Resize
from textual.filter import LineFilter
from textual.screen import ModalScreen
from textual.widgets import Button, Footer, Static, TextArea

from herdr_drafts.herdr import HerdrError
from herdr_drafts.models import Draft, DraftConflict
from herdr_drafts.service import DraftService
from herdr_drafts.terminal_colors import TerminalDefaultColors

STATUS_LABELS = {
    "working": "En cours",
    "idle": "Disponible",
    "done": "Disponible",
    "blocked": "Réponse attendue dans le terminal",
    "unknown": "État inconnu",
}


class DraftComposer(TextArea):
    BINDINGS = [
        Binding("enter", "app.save", "Ajouter", show=False, priority=True),
        Binding("shift+enter,ctrl+j", "newline", "Nouvelle ligne", show=False, priority=True),
    ]

    def action_newline(self) -> None:
        self.replace("\n", *self.selection, maintain_selection_offset=False)


class Confirm(ModalScreen[bool]):
    BINDINGS = [Binding("escape", "cancel", show=False)]

    def __init__(self, question: str) -> None:
        super().__init__()
        self.question = question

    def compose(self) -> ComposeResult:
        with Vertical(id="confirm-dialog"):
            yield Static(self.question, markup=False)
            with Horizontal(classes="actions"):
                yield Button("Annuler", id="confirm-no", compact=True)
                yield Button("Confirmer", variant="warning", id="confirm-yes", compact=True)

    def on_mount(self) -> None:
        self.query_one("#confirm-no", Button).focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        event.stop()
        self.dismiss(event.button.id == "confirm-yes")

    def action_cancel(self) -> None:
        self.dismiss(False)


class DraftCard(Vertical):
    def __init__(self, draft: Draft, number: int, *, editing: bool, busy: bool) -> None:
        super().__init__(id=f"card-{draft.id}", classes=f"draft-card {draft.state}")
        self.draft = draft
        self.number = number
        self.editing = editing
        self.busy = busy

    def compose(self) -> ComposeResult:
        draft = self.draft
        label = {
            "draft": "Prêt",
            "sending": "Envoi en cours…",
            "sent": "Transmis au terminal",
            "error": "À réessayer",
            "uncertain": "Envoi à vérifier",
        }[draft.state]
        if self.editing:
            label = "En cours de modification"
        yield Static(f"{self.number}  {label}", classes="card-heading", markup=False)
        lines = draft.body.splitlines()
        preview = "\n".join(lines[:4])[:360]
        if len(lines) > 4 or len(draft.body) > 360:
            preview += "\n…"
        yield Static(preview, classes="preview", markup=False)
        if draft.error:
            yield Static(draft.error, classes="message-error", markup=False)
        with Horizontal(classes="actions"):
            if draft.state in ("draft", "error"):
                yield Button(
                    "Envoyer",
                    id=f"send-{draft.id}",
                    variant="primary",
                    compact=True,
                    disabled=self.busy or self.editing,
                )
                yield Button(
                    "Modifier",
                    id=f"edit-{draft.id}",
                    compact=True,
                    disabled=self.busy or self.editing,
                )
            elif draft.state == "uncertain":
                yield Button("Remettre en brouillon", id=f"resolve-{draft.id}", compact=True)
            elif draft.state == "sent":
                yield Button("Reprendre", id=f"reuse-{draft.id}", compact=True)
        if draft.state in ("draft", "error", "uncertain"):
            with Horizontal(classes="actions secondary-actions"):
                if draft.state == "uncertain":
                    yield Button("Déjà transmis", id=f"delivered-{draft.id}", compact=True)
                else:
                    yield Button(
                        "↑",
                        id=f"up-{draft.id}",
                        compact=True,
                        tooltip="Monter dans la liste",
                        disabled=self.busy,
                    )
                    yield Button(
                        "↓",
                        id=f"down-{draft.id}",
                        compact=True,
                        tooltip="Descendre dans la liste",
                        disabled=self.busy,
                    )
                    yield Button(
                        "Supprimer",
                        id=f"delete-{draft.id}",
                        compact=True,
                        disabled=self.busy or self.editing,
                    )


class DraftsApp(App[bool]):
    TITLE = "Herdr Drafts"
    CSS_PATH = "app.tcss"
    BINDINGS = [
        Binding(
            "super+u",
            "save",
            "Ajouter",
            key_display="⌘U",
            show=sys.platform == "darwin",
            priority=True,
        ),
        Binding(
            "super+y",
            "send_next",
            "Suivant",
            key_display="⌘Y",
            show=sys.platform == "darwin",
            priority=True,
        ),
        Binding(
            "super+l",
            "quit",
            "Fermer",
            key_display="⌘L",
            show=sys.platform == "darwin",
            priority=True,
        ),
        Binding("super+p", "new", "Nouveau", show=False, priority=True),
        Binding("super+o", "agents", "Agents", show=False, priority=True),
        Binding("super+r", "refresh", "Actualiser", show=False, priority=True),
        Binding("f2", "save", "Ajouter", show=sys.platform != "darwin", priority=True),
        Binding("f8", "send_next", "Suivant", show=sys.platform != "darwin", priority=True),
        Binding("f10", "quit", "Fermer", show=sys.platform != "darwin", priority=True),
        Binding("ctrl+s", "save", "Ajouter", show=False, priority=True),
        Binding("ctrl+n,f4", "new", "Nouveau", show=False, priority=True),
        Binding("ctrl+r,f5", "refresh", "Actualiser", show=False, priority=True),
        Binding("ctrl+g,f3", "agents", "Agents", show=False, priority=True),
        Binding("ctrl+t", "send_next", "Suivant", show=False, priority=True),
        Binding("ctrl+q", "quit", "Fermer", show=False, priority=True),
    ]

    def __init__(
        self,
        service: DraftService,
        *,
        demo: bool = False,
        panel_id: str | None = None,
        allow_agent_switch: bool = False,
    ) -> None:
        super().__init__(ansi_color=True)
        self._terminal_colors = TerminalDefaultColors()
        self.theme = "textual-ansi"
        self.service = service
        self.demo = demo
        self.panel_id = panel_id
        self.allow_agent_switch = allow_agent_switch
        self._editing_id = service.store.composer(service.target).editing_id
        self._history = False
        self._busy = False
        self._refreshing = False

    def get_line_filters(self) -> Sequence[LineFilter]:
        filters = super().get_line_filters()
        return [*filters, self._terminal_colors] if self.ansi_color else filters

    def compose(self) -> ComposeResult:
        target = self.service.target
        yield Static("Brouillons", id="title")
        label = f"{target.agent}  /  {target.pane_id}"
        with Horizontal(id="target-line"):
            yield Static(label, id="destination", markup=False)
            if self.allow_agent_switch:
                yield Button("Agents", id="switch-agent", compact=True)
        yield Static(target.cwd or "Agent sélectionné", id="directory", markup=False)
        yield Static("Démo : aucun agent connecté" if self.demo else "Connexion…", id="agent-state")
        yield Static("Écrire un message", id="composer-title")
        yield DraftComposer(
            self.service.store.composer(target).body,
            id="composer",
            soft_wrap=True,
            show_line_numbers=False,
            tab_behavior="focus",
            placeholder="Préparez votre prochain message…",
        )
        with Horizontal(id="compose-actions", classes="actions"):
            yield Button("Ajouter à la liste", id="save", variant="primary", compact=True)
            yield Button("Nouveau", id="new", compact=True)
        yield Static("", id="composer-hint", markup=False)
        with Horizontal(id="queue-header"):
            yield Static("À envoyer", id="queue-title")
            yield Button("Historique", id="history", compact=True)
        yield VerticalScroll(id="queue")
        yield Footer()

    async def on_mount(self) -> None:
        self._update_composer_labels()
        await self.render_queue()
        self.query_one("#composer", TextArea).focus()
        self.set_interval(3, self.action_refresh)
        self.action_refresh()

    def on_resize(self, event: Resize) -> None:
        for editor in self.query(TextArea):
            editor.styles.height = 5 if event.size.height < 28 else 8

    def _update_composer_labels(self) -> None:
        body = self.query_one("#composer", TextArea).text
        editing = self._editing_id is not None
        self.query_one("#composer-title", Static).update(
            "Modifier le message" if editing else "Écrire un message"
        )
        button = self.query_one("#save", Button)
        button.label = "Enregistrer" if editing else "Ajouter à la liste"
        button.disabled = not body.strip()
        self.query_one("#composer-hint", Static).update(
            f"{len(body.encode('utf-8')):,} / 65 536 octets  ·  sauvegardé"
            if body
            else "Entrée : ajouter   Maj+Entrée : nouvelle ligne"
        )

    def _persist_composer(self) -> bool:
        try:
            self.service.store.save_composer(
                self.service.target, self.query_one("#composer", TextArea).text, self._editing_id
            )
        except sqlite3.Error:
            self.query_one("#composer-hint", Static).update(
                "Sauvegarde impossible. Gardez le panneau ouvert et copiez votre texte."
            )
            return False
        self._update_composer_labels()
        return True

    def on_text_area_changed(self, event: TextArea.Changed) -> None:
        if event.text_area.id == "composer":
            self._persist_composer()

    async def render_queue(self) -> None:
        drafts = self.service.store.list_drafts(self.service.target, history=self._history)
        queue = self.query_one("#queue", VerticalScroll)
        scroll_y = queue.scroll_y
        with self.batch_update():
            await queue.remove_children()
            if not drafts:
                await queue.mount(
                    Static(
                        "Les messages transmis apparaîtront ici."
                        if self._history
                        else "Votre liste est vide.\n"
                        "Écrivez un message, puis ajoutez-le à la liste.",
                        id="empty",
                        markup=False,
                    )
                )
            else:
                await queue.mount(
                    *[
                        DraftCard(
                            draft, index, editing=draft.id == self._editing_id, busy=self._busy
                        )
                        for index, draft in enumerate(drafts, 1)
                    ]
                )
        queue.scroll_to(y=scroll_y, animate=False)
        title = "Transmis" if self._history else "À envoyer"
        self.query_one("#queue-title", Static).update(f"{title} ({len(drafts)})")
        self.query_one("#history", Button).label = "Brouillons" if self._history else "Historique"

    async def action_save(self) -> None:
        if not self._persist_composer():
            return
        try:
            self.service.save()
        except (ValueError, sqlite3.Error) as exc:
            self.notify(str(exc), severity="error")
            return
        self._editing_id = None
        self.query_one("#composer", TextArea).load_text("")
        self._history = False
        await self.render_queue()
        self.query_one("#composer", TextArea).focus()

    async def action_send_next(self) -> None:
        if self._busy or self._history or isinstance(self.screen, ModalScreen):
            return
        try:
            drafts = self.service.store.list_drafts(self.service.target)
            if not drafts:
                self.notify("La liste est vide.")
                return
            first = drafts[0]
            if first.id == self._editing_id:
                self.notify(
                    "Enregistrez d'abord la modification du premier message.", severity="warning"
                )
                return
            if first.state not in ("draft", "error"):
                self.notify("Vérifiez d'abord l'envoi du premier message.", severity="warning")
                return
            await self._draft_action("send", first.id)
        except (ValueError, sqlite3.Error) as exc:
            self.notify(str(exc), severity="error")

    async def action_new(self) -> None:
        if self.query_one("#composer", TextArea).text:
            self.push_screen(
                Confirm("Effacer le texte en cours pour préparer un nouveau message ?"),
                self._new_confirmed,
            )
        else:
            await self._new_confirmed(True)

    async def _new_confirmed(self, confirmed: bool | None) -> None:
        if confirmed:
            self._editing_id = None
            self.query_one("#composer", TextArea).load_text("")
            self._persist_composer()
            await self.render_queue()
            self.query_one("#composer", TextArea).focus()

    async def on_button_pressed(self, event: Button.Pressed) -> None:
        button_id = event.button.id or ""
        try:
            if button_id == "save":
                await self.action_save()
            elif button_id == "new":
                await self.action_new()
            elif button_id == "history":
                self._history = not self._history
                await self.render_queue()
            elif button_id == "switch-agent":
                await self.action_agents()
            elif "-" in button_id:
                action, draft_id = button_id.split("-", 1)
                await self._draft_action(action, draft_id)
        except (ValueError, sqlite3.Error) as exc:
            self.notify(str(exc), severity="error")

    async def _draft_action(self, action: str, draft_id: str) -> None:
        if action == "send":
            if self._busy:
                return
            self._busy = True
            await self.render_queue()
            self.run_worker(self._send(draft_id), group="send")
        elif action in ("edit", "reuse"):
            if self.query_one("#composer", TextArea).text:
                self.notify("Ajoutez ou terminez d'abord le message en cours.", severity="warning")
                return
            draft = self.service.store.get(draft_id)
            self._editing_id = draft.id if action == "edit" else None
            self.query_one("#composer", TextArea).load_text(draft.body)
            self._persist_composer()
            await self.render_queue()
            self.query_one("#composer", TextArea).focus()
        elif action in ("up", "down"):
            self.service.store.move(draft_id, -1 if action == "up" else 1)
            await self.render_queue()
        elif action in ("delete", "resolve", "delivered"):
            questions = {
                "delete": "Supprimer ce brouillon ?",
                "resolve": "Vérifiez le terminal : ce message n'a pas été transmis ?\n"
                "Confirmer le remet en brouillon, sans l'envoyer.",
                "delivered": "Vous avez retrouvé ce message dans le terminal ?\n"
                "Confirmer le classe dans l'historique.",
            }

            async def confirmed(accepted: bool | None) -> None:
                if not accepted:
                    return
                if action == "delete":
                    self.service.store.delete(draft_id)
                else:
                    self.service.store.resolve(draft_id, delivered=action == "delivered")
                await self.render_queue()

            self.push_screen(Confirm(questions[action]), confirmed)

    async def _send(self, draft_id: str) -> None:
        try:
            draft = await self.service.send(draft_id)
            if draft.state == "sent":
                self.notify("Message transmis au terminal.")
            elif draft.state == "uncertain":
                self.notify(
                    "Envoi à vérifier dans le terminal. Aucun renvoi automatique.",
                    severity="warning",
                    timeout=8,
                )
            else:
                self.notify(draft.error or "Message conservé.", severity="error")
        except (ValueError, sqlite3.Error) as exc:
            self.notify(str(exc), severity="error")
        finally:
            self._busy = False
            await self.render_queue()
            self.action_refresh()

    def action_refresh(self) -> None:
        if self._busy or self._refreshing:
            return
        self._refreshing = True
        self.run_worker(self._refresh_status(), group="refresh")

    async def _refresh_status(self) -> None:
        try:
            if self.panel_id:
                await asyncio.to_thread(
                    self.service.client.request,
                    "pane.report_metadata",
                    {
                        "pane_id": self.panel_id,
                        "source": "herdr-drafts",
                        "tokens": {
                            "drafts_owner": self.service.target.lock_key,
                            "drafts_target": self.service.target.key,
                        },
                        "ttl_ms": 10000,
                    },
                )
            live = await self.service.refresh()
            label = STATUS_LABELS.get(live.status, "État inconnu")
            if self.demo:
                label = "Démo : les envois sont simulés"
            self.query_one("#agent-state", Static).update(label)
        except (HerdrError, DraftConflict) as exc:
            self.query_one("#agent-state", Static).update(str(exc))
        finally:
            self._refreshing = False

    async def action_quit(self) -> None:
        if self._busy:
            self.notify("Attendez la confirmation de l'envoi avant de fermer.", severity="warning")
        elif self._persist_composer():
            self.exit()

    async def action_agents(self) -> None:
        if not self.allow_agent_switch:
            return
        if self._busy:
            self.notify(
                "Attendez la confirmation de l'envoi avant de changer d'agent.", severity="warning"
            )
        elif self._persist_composer():
            self.exit(True)
