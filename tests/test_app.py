from pathlib import Path

import pytest
from textual.widgets import TextArea

from herdr_drafts.app import DraftsApp
from herdr_drafts.herdr import HerdrClient
from herdr_drafts.service import DraftService
from herdr_drafts.store import DraftStore


@pytest.fixture
def app(fake_herdr, tmp_path: Path):
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    store = DraftStore(tmp_path / "drafts.sqlite3")
    service = DraftService(store, client, client.get_agent("w1:p1").target)
    app = DraftsApp(service)
    yield app
    store.close()


async def add_message(app, pilot, text):
    app.query_one("#composer", TextArea).load_text(text)
    await pilot.pause()
    await pilot.click("#save")
    await pilot.pause()


async def test_prepare_several_messages_without_sending(app, fake_herdr):
    async with app.run_test(size=(48, 38)) as pilot:
        await add_message(app, pilot, "Premier message\navec un accent é")
        await add_message(app, pilot, "Deuxième message 🎯")
        assert len(app.service.store.list_drafts(app.service.target)) == 2
        assert app.query_one("#composer", TextArea).text == ""
        assert fake_herdr.prompts == []


async def test_click_sends_only_the_chosen_message(app, fake_herdr):
    async with app.run_test(size=(60, 50)) as pilot:
        await add_message(app, pilot, "premier")
        await add_message(app, pilot, "deuxième")
        drafts = app.service.store.list_drafts(app.service.target)
        await pilot.click(f"#send-{drafts[1].id}")
        await app.workers.wait_for_complete()
        await pilot.pause()
        assert fake_herdr.prompts == ["deuxième"]
        assert [d.body for d in app.service.store.list_drafts(app.service.target)] == ["premier"]


async def test_editor_autosaves_even_before_adding(app):
    async with app.run_test() as pilot:
        await pilot.press("b", "o", "n", "j", "o", "u", "r", "enter", "a")
        await pilot.pause()
        assert app.service.store.composer(app.service.target).body == "bonjour\na"


async def test_edit_and_reorder_with_mouse(app):
    async with app.run_test(size=(60, 50)) as pilot:
        await add_message(app, pilot, "premier")
        await add_message(app, pilot, "deuxième")
        first, second = app.service.store.list_drafts(app.service.target)
        await pilot.click(f"#up-{second.id}")
        await pilot.pause()
        assert app.service.store.list_drafts(app.service.target)[0].id == second.id
        await pilot.click(f"#edit-{first.id}")
        await pilot.pause()
        assert app.query_one("#composer", TextArea).text == "premier"
        await add_message(app, pilot, "premier modifié")
        assert app.service.store.get(first.id).body == "premier modifié"


async def test_delete_requires_confirmation_and_keeps_other_text(app):
    async with app.run_test(size=(60, 40)) as pilot:
        await add_message(app, pilot, "à supprimer")
        draft = app.service.store.list_drafts(app.service.target)[0]
        await pilot.click(f"#delete-{draft.id}")
        await pilot.pause()
        await pilot.click("#confirm-no")
        assert app.service.store.get(draft.id).body == "à supprimer"
        await pilot.click(f"#delete-{draft.id}")
        await pilot.pause()
        await pilot.click("#confirm-yes")
        await pilot.pause()
        assert app.service.store.list_drafts(app.service.target) == []


async def test_narrow_panel_stays_usable(app):
    async with app.run_test(size=(36, 24)) as pilot:
        await add_message(app, pilot, "un message court")
        draft = app.service.store.list_drafts(app.service.target)[0]
        button = app.query_one(f"#send-{draft.id}")
        assert button.region.right <= 36
        await pilot.click(button)
        await app.workers.wait_for_complete()
        assert app.service.store.get(draft.id).state == "sent"


async def test_uncertain_send_stays_in_the_list_for_review(app, fake_herdr):
    fake_herdr.drop_prompt = True
    async with app.run_test(size=(60, 42)) as pilot:
        await add_message(app, pilot, "important")
        draft = app.service.store.list_drafts(app.service.target)[0]
        await pilot.click(f"#send-{draft.id}")
        await app.workers.wait_for_complete()
        await pilot.pause()
        assert app.service.store.get(draft.id).state == "uncertain"
        await pilot.click(f"#resolve-{draft.id}")
        await pilot.pause()
        await pilot.click("#confirm-no")
        assert app.service.store.get(draft.id).state == "uncertain"
        assert fake_herdr.prompts == ["important"]


async def test_new_message_does_not_silently_erase_composer(app):
    async with app.run_test(size=(48, 32)) as pilot:
        app.query_one("#composer", TextArea).load_text("en cours")
        await pilot.pause()
        await pilot.press("ctrl+n")
        await pilot.pause()
        await pilot.click("#confirm-no")
        assert app.query_one("#composer", TextArea).text == "en cours"


async def test_new_shortcut_focuses_the_empty_editor(app):
    async with app.run_test(size=(48, 32)) as pilot:
        app.query_one("#new").focus()
        await pilot.press("ctrl+n")
        await pilot.pause()
        assert app.query_one("#composer", TextArea).has_focus


async def test_restored_initial_identity_loads_the_adopted_composer_before_typing(app):
    from dataclasses import replace

    initial = replace(app.service.target, session=None)
    original = DraftService(app.service.store, app.service.client, initial)
    original.store.save_composer(initial, "message dans la liste")
    draft = original.save()
    original.verify_target(app.service.target)
    original.store.save_composer(original.target, "texte durable après adoption", draft.id)
    restored = DraftsApp(DraftService(original.store, original.client, initial))
    async with restored.run_test(size=(48, 36)) as pilot:
        assert restored.query_one("#composer", TextArea).text == "texte durable après adoption"
        await pilot.press("end", "x")
        await pilot.pause()
        composer = original.store.composer(original.target)
        assert composer.body == "texte durable après adoptionx"
        assert composer.editing_id == draft.id
