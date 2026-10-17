import asyncio
from pathlib import Path

import pytest

from herdr_drafts.herdr import HerdrClient
from herdr_drafts.models import DraftConflict
from herdr_drafts.service import DraftService, validate_prompt
from herdr_drafts.store import DraftStore


@pytest.fixture
def service(fake_herdr, tmp_path: Path):
    client = HerdrClient(fake_herdr.path, timeout=1, connect=fake_herdr.connect)
    store = DraftStore(tmp_path / "drafts.sqlite3")
    service = DraftService(store, client, client.get_agent("w1:p1").target)
    yield service
    store.close()


def add(service, text="message"):
    service.store.save_composer(service.target, text)
    return service.save()


async def test_manual_send_to_a_working_agent(service, fake_herdr):
    draft = add(service, "À faire ensuite\nmerci 🎯")
    result = await service.send(draft.id)
    assert result.state == "sent"
    assert fake_herdr.prompts == [draft.body]
    assert service.store.get(draft.id).body == draft.body


@pytest.mark.parametrize("status", ["blocked", "unknown"])
async def test_unready_agent_keeps_the_draft_without_input(service, fake_herdr, status):
    fake_herdr.agent["agent_status"] = status
    draft = add(service)
    result = await service.send(draft.id)
    assert result.state == "error"
    assert result.body == draft.body
    assert fake_herdr.prompts == []


@pytest.mark.parametrize(
    "field,value", [("terminal_id", "replacement"), ("agent", "codex"), ("cwd", "/other-project")]
)
async def test_changed_target_is_rejected(service, fake_herdr, field, value):
    fake_herdr.agent[field] = value
    result = await service.send(add(service).id)
    assert result.state == "error"
    assert fake_herdr.prompts == []


async def test_native_conversation_replacement_is_rejected(service, fake_herdr):
    fake_herdr.agent["agent_session"]["value"] = "different-conversation"
    result = await service.send(add(service).id)
    assert result.state == "error"
    assert fake_herdr.prompts == []


async def test_double_click_submits_at_most_once(service, fake_herdr):
    draft = add(service)
    results = await asyncio.gather(
        service.send(draft.id), service.send(draft.id), return_exceptions=True
    )
    assert fake_herdr.prompts == ["message"]
    assert any(isinstance(result, DraftConflict) for result in results)


async def test_uncertain_result_is_not_automatically_retried(service, fake_herdr):
    fake_herdr.drop_prompt = True
    draft = add(service)
    result = await service.send(draft.id)
    assert result.state == "uncertain"
    with pytest.raises(DraftConflict):
        await service.send(draft.id)
    assert fake_herdr.prompts == ["message"]


async def test_another_targets_draft_is_never_sent(service, fake_herdr):
    from dataclasses import replace

    other = replace(service.target, terminal_id="other")
    service.store.save_composer(other, "another target")
    draft = service.store.commit_composer(other)
    with pytest.raises(DraftConflict):
        await service.send(draft.id)
    assert fake_herdr.prompts == []
    assert service.store.get(draft.id).state == "draft"


@pytest.mark.parametrize(
    "body", ["", "  \n ", "escape\x1b[31m", "a\x00b", "a\rb", "a\x7fb", "é" * 32769]
)
def test_invalid_prompt_cannot_enter_the_queue(body):
    with pytest.raises(ValueError):
        validate_prompt(body)


def test_prompt_size_is_measured_in_utf8_bytes():
    validate_prompt("é" * 32768)
    validate_prompt("plusieurs\nlignes\t🎯")


def test_reopen_after_the_first_native_session_recovers_unsent_text(service):
    from dataclasses import replace

    initial = replace(service.target, session=None)
    service.store.save_composer(initial, "À garder dans la liste")
    draft = service.store.commit_composer(initial)
    service.store.save_composer(initial, "Écriture encore en cours")
    reopened = DraftService(service.store, service.client, service.target)
    assert [d.id for d in reopened.store.list_drafts(reopened.target)] == [draft.id]
    assert reopened.store.composer(reopened.target).body == "Écriture encore en cours"


def test_restarted_panel_resolves_its_original_environment_before_recovery(service):
    from dataclasses import replace

    initial = replace(service.target, session=None)
    original = DraftService(service.store, service.client, initial)
    draft = add(original)
    original.verify_target(service.client.get_agent(initial.pane_id).target)
    original.store.save_composer(original.target, "Texte durable après adoption", draft.id)
    original.store.claim(draft.id)
    restored = DraftService(service.store, service.client, initial)
    restored.store.recover_inflight(restored.target)
    assert restored.target.session == service.target.session
    assert restored.store.composer(restored.target).body == "Texte durable après adoption"
    assert restored.store.composer(restored.target).editing_id == draft.id
    assert restored.store.get(draft.id).state == "uncertain"


def test_a_known_conversation_is_not_rebound_to_a_different_one(service):
    from dataclasses import replace

    initial = replace(service.target, session=None)
    original = DraftService(service.store, service.client, initial)
    draft = add(original)
    original.verify_target(service.client.get_agent(initial.pane_id).target)
    different = replace(service.target, session="conversation-b")
    reopened = DraftService(service.store, service.client, different)
    assert reopened.target == different
    assert reopened.store.list_drafts(different) == []
    assert reopened.store.get(draft.id).target_key == service.target.key
