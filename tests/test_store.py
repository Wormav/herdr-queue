from pathlib import Path

import pytest

from herdr_drafts.models import DraftConflict, Target
from herdr_drafts.store import DraftStore, PanelLock


@pytest.fixture
def target() -> Target:
    return Target("/tmp/herdr-test.sock", "w1:p1", "term-1", "w1", "claude", "/project", "s1")


@pytest.fixture
def store(tmp_path: Path) -> DraftStore:
    db = DraftStore(tmp_path / "drafts.sqlite3")
    yield db
    db.close()


def add(store: DraftStore, target: Target, body: str):
    store.save_composer(target, body)
    return store.commit_composer(target)


def test_composer_and_several_drafts_survive_reopen(tmp_path, target):
    path = tmp_path / "drafts.sqlite3"
    store = DraftStore(path)
    first = add(store, target, "Première ligne\nDeuxième ligne 🎯")
    add(store, target, "Ensuite, vérifie les tests.")
    store.save_composer(target, "Une idée encore incomplète…")
    store.close()
    store = DraftStore(path)
    assert [d.body for d in store.list_drafts(target)] == [
        first.body,
        "Ensuite, vérifie les tests.",
    ]
    assert store.composer(target).body == "Une idée encore incomplète…"
    store.close()


def test_edit_does_not_remove_the_original_until_saved(store, target):
    draft = add(store, target, "original")
    store.save_composer(target, "modifié", editing_id=draft.id)
    assert store.get(draft.id).body == "original"
    assert store.composer(target).editing_id == draft.id
    updated = store.commit_composer(target)
    assert updated.id == draft.id
    assert updated.body == "modifié"
    assert store.composer(target).body == ""


def test_targets_and_native_conversations_are_isolated(store, target):
    other = Target("/tmp/herdr-test.sock", "w1:p1", "term-1", "w1", "claude", "/project", "s2")
    add(store, target, "first conversation")
    assert store.list_drafts(other) == []
    assert store.composer(other).body == ""


def test_reordering_and_delete_keep_the_remaining_order(store, target):
    a, b, c = [add(store, target, name) for name in ("a", "b", "c")]
    store.move(c.id, -1)
    assert [d.id for d in store.list_drafts(target)] == [a.id, c.id, b.id]
    store.delete(a.id)
    assert [d.body for d in store.list_drafts(target)] == ["c", "b"]


def test_claim_is_atomic_across_connections(tmp_path, target):
    path = tmp_path / "drafts.sqlite3"
    one, two = DraftStore(path), DraftStore(path)
    draft = add(one, target, "only once")
    assert one.claim(draft.id).state == "sending"
    with pytest.raises(DraftConflict):
        two.claim(draft.id)
    one.finish(draft.id, "sent")
    assert one.list_drafts(target) == []
    assert one.list_drafts(target, history=True)[0].body == "only once"
    one.close()
    two.close()


def test_uncertain_send_needs_explicit_resolution(store, target):
    draft = add(store, target, "preserve me")
    store.claim(draft.id)
    store.finish(draft.id, "uncertain", "Connexion perdue")
    with pytest.raises(DraftConflict):
        store.claim(draft.id)
    store.resolve(draft.id, delivered=False)
    assert store.claim(draft.id).body == "preserve me"


def test_recovery_is_scoped_to_the_locked_target(store, target):
    other = Target("/tmp/another.sock", "w1:p1", "other-term", "w1", "codex", "", None)
    first, second = add(store, target, "first"), add(store, other, "second")
    store.claim(first.id)
    store.claim(second.id)
    store.recover_inflight(target)
    assert store.get(first.id).state == "uncertain"
    assert store.get(second.id).state == "sending"


def test_a_second_panel_cannot_own_the_same_terminal(tmp_path, target):
    first, second = PanelLock(tmp_path, target), PanelLock(tmp_path, target)
    first.acquire()
    with pytest.raises(DraftConflict):
        second.acquire()
    first.release()
    second.acquire()
    second.release()


def test_first_native_session_can_adopt_existing_unsent_drafts(store, target):
    unknown = Target(
        target.socket_path,
        target.pane_id,
        target.terminal_id,
        target.workspace_id,
        target.agent,
        target.cwd,
        None,
    )
    draft = add(store, unknown, "next prompt")
    store.save_composer(unknown, "still writing")
    store.adopt_session(unknown, target)
    assert store.get(draft.id).target_key == target.key
    assert store.composer(target).body == "still writing"


def test_permissions_do_not_expose_prompt_text(tmp_path, target):
    path = tmp_path / "private" / "drafts.sqlite3"
    store = DraftStore(path)
    add(store, target, "local text")
    assert path.stat().st_mode & 0o077 == 0
    store.close()
