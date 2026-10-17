"""Rules for deliberate prompt submission to one pinned agent conversation."""

from __future__ import annotations

import asyncio
from dataclasses import replace

from herdr_drafts.herdr import HerdrClient, HerdrError
from herdr_drafts.models import Agent, Draft, DraftConflict, Target
from herdr_drafts.store import DraftStore

MAX_PROMPT_BYTES = 64 * 1024


def validate_prompt(body: str) -> None:
    if not body.strip():
        raise ValueError("Écrivez un message avant de l'ajouter.")
    if len(body.encode("utf-8")) > MAX_PROMPT_BYTES:
        raise ValueError("Le message dépasse 64 Kio. Divisez-le en plusieurs messages.")
    if any((ord(char) < 32 and char not in "\n\t") or 127 <= ord(char) < 160 for char in body):
        raise ValueError("Le message contient un caractère de contrôle du terminal.")


class DraftService:
    def __init__(self, store: DraftStore, client: HerdrClient, target: Target) -> None:
        self.store = store
        self.client = client
        self.target = store.resolve_target(target)
        if target.session is not None:
            initial = store.recorded_target(replace(target, session=None).key)
            if initial is not None and store.resolve_target(initial).session is None:
                self.target = initial
                self.verify_target(target)

    def save(self) -> Draft:
        validate_prompt(self.store.composer(self.target).body)
        return self.store.commit_composer(self.target)

    def verify_target(self, current: Target) -> None:
        bound = self.target
        if (
            current.socket_path,
            current.pane_id,
            current.terminal_id,
            current.workspace_id,
            current.agent,
            current.cwd,
        ) != (
            bound.socket_path,
            bound.pane_id,
            bound.terminal_id,
            bound.workspace_id,
            bound.agent,
            bound.cwd,
        ) or (bound.session is not None and current.session != bound.session):
            raise DraftConflict(
                "La conversation cible a changé. Fermez ce panneau (Ctrl+Q), "
                "puis rouvrez-le depuis l'agent souhaité."
            )
        if bound.session is None and current.session is not None:
            self.store.adopt_session(bound, current)
            self.target = current

    async def refresh(self) -> Agent:
        live = await asyncio.to_thread(self.client.get_agent, self.target.pane_id)
        self.verify_target(live.target)
        return live

    async def send(self, draft_id: str) -> Draft:
        draft = self.store.get(draft_id)
        if draft.target_key != self.target.key:
            raise DraftConflict("Ce message appartient à un autre agent.")
        validate_prompt(draft.body)
        self.store.claim(draft_id)
        submission_started = False
        try:
            live = await self.refresh()
            if live.status not in ("idle", "done", "working"):
                raise DraftConflict(
                    "L'agent attend une réponse interactive."
                    if live.status == "blocked"
                    else "L'état de l'agent est inconnu. Vérifiez son terminal."
                )
            submission_started = True
            await asyncio.to_thread(self.client.prompt, self.target.pane_id, draft.body)
        except (DraftConflict, HerdrError) as exc:
            uncertain = isinstance(exc, HerdrError) and exc.uncertain
            self.store.finish(draft_id, "uncertain" if uncertain else "error", str(exc))
        except asyncio.CancelledError:
            self.store.finish(
                draft_id,
                "uncertain" if submission_started else "error",
                "Envoi interrompu. Vérifiez le terminal avant de réessayer.",
            )
            raise
        else:
            self.store.finish(draft_id, "sent")
        return self.store.get(draft_id)
