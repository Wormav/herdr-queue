import pytest

from herdr_drafts.herdr import HerdrClient, HerdrError


def test_parses_the_installed_schema_shape(fake_herdr):
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    agent = client.get_agent("w1:p1")
    assert agent.target.pane_id == "w1:p1"
    assert agent.target.session == '["test", "id", "s1"]'
    assert agent.status == "working"
    assert client.list_agents() == [agent]


def test_unicode_multiline_prompt_remains_literal(fake_herdr):
    fake_herdr.fragment = True
    client = HerdrClient(fake_herdr.path, connect=fake_herdr.connect)
    body = "Vérifie les tests 🎯\n`rm -rf /` et $(echo hello) restent du texte.\n\tfin"
    client.prompt("w1:p1", body)
    assert fake_herdr.prompts == [body]
    assert fake_herdr.requests[0]["params"] == {"target": "w1:p1", "text": body}


def test_rejects_an_unrelated_response(fake_herdr):
    fake_herdr.wrong_id = True
    with pytest.raises(HerdrError, match="réponse"):
        HerdrClient(fake_herdr.path, connect=fake_herdr.connect).get_agent("w1:p1")


def test_disconnected_submission_is_uncertain(fake_herdr):
    fake_herdr.drop_prompt = True
    with pytest.raises(HerdrError) as error:
        HerdrClient(fake_herdr.path, connect=fake_herdr.connect).prompt("w1:p1", "message")
    assert error.value.uncertain
    assert fake_herdr.prompts == ["message"]


def test_blocked_rejection_is_known_not_to_have_sent(fake_herdr):
    fake_herdr.prompt_error = {"code": "agent_blocked", "message": "blocked"}
    with pytest.raises(HerdrError) as error:
        HerdrClient(fake_herdr.path, connect=fake_herdr.connect).prompt("w1:p1", "message")
    assert not error.value.uncertain
    assert error.value.code == "agent_blocked"


def test_unexpected_prompt_error_is_conservatively_uncertain(fake_herdr):
    fake_herdr.prompt_error = {"code": "timeout", "message": "too late"}
    with pytest.raises(HerdrError) as error:
        HerdrClient(fake_herdr.path, connect=fake_herdr.connect).prompt("w1:p1", "message")
    assert error.value.uncertain


def test_absent_socket_has_no_submission_side_effect(tmp_path):
    with pytest.raises(HerdrError) as error:
        HerdrClient(str(tmp_path / "missing.sock")).prompt("w1:p1", "message")
    assert not error.value.uncertain


def test_missing_agent_identity_fails_before_input(fake_herdr):
    del fake_herdr.agent["terminal_id"]
    with pytest.raises(HerdrError):
        HerdrClient(fake_herdr.path, connect=fake_herdr.connect).get_agent("w1:p1")
