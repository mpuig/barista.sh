"""The session's contract, pinned where it is cheap to check.

The scenario itself (nap-006 task 3.2) needs a real pause and resume and so waits
on the guest channel. These do not: they hold the workload to the properties that
make T7 *meaningful*, which is a different thing from making it pass.
"""

import importlib.util
import pathlib
import sys

_spec = importlib.util.spec_from_file_location(
    "acp_session", pathlib.Path(__file__).resolve().parents[1] / "acp_session.py"
)
acp_session = importlib.util.module_from_spec(_spec)
sys.modules["acp_session"] = acp_session
_spec.loader.exec_module(acp_session)

Session = acp_session.Session


def prompt(session, text):
    return session.handle(
        {"method": "session/prompt", "params": {"text": text}}
    )


def context(session):
    return session.handle({"method": "session/context"})


def test_turns_accumulate_in_order():
    session = Session()
    assert prompt(session, "one")["turns"] == 1
    assert prompt(session, "two")["turns"] == 2
    assert context(session)["turns"] == 2


def test_a_fresh_session_holds_nothing():
    """The property that makes T7 worth running.

    If the session persisted its context anywhere, T7 would pass while proving
    nothing — disk survives a *stop*, and the whole claim is that memory survives
    a *pause*. A new process must therefore start empty, and its digest must
    differ from one that has talked.
    """
    talked = Session()
    prompt(talked, "hello")
    fresh = Session()

    assert context(fresh)["turns"] == 0
    assert context(fresh)["digest"] != context(talked)["digest"]


def test_the_digest_depends_on_content_and_nothing_else():
    """Compared across a pause, so it must not move for reasons that are not the
    conversation — a timestamp or an id baked in would make T7 fail on a correct
    resume."""
    a, b = Session(), Session()
    for session in (a, b):
        prompt(session, "same")
        prompt(session, "words")
    assert context(a)["digest"] == context(b)["digest"]

    c = Session()
    prompt(c, "same")
    prompt(c, "different")
    assert context(c)["digest"] != context(a)["digest"]


def test_reconnect_is_reported_even_when_the_provider_is_down():
    """`post_restore_cmd` calls this (B26). A provider that cannot be reached is
    not a failed resume — the context is intact either way — so it reports the
    attempt honestly rather than raising."""
    session = Session()
    result = session.handle({"method": "session/reconnect"})
    assert result["reconnects"] == 1
    assert isinstance(result["connected"], bool)
    # And it is visible to the assertion the scenario makes after a resume.
    assert context(session)["reconnects"] == 1


def test_an_unknown_method_is_an_error_not_a_crash():
    """The session outlives every request; one bad frame must not end it."""
    session = Session()
    prompt(session, "before")
    try:
        session.handle({"method": "session/nope"})
        raise AssertionError("an unknown method must be rejected")
    except ValueError:
        pass
    assert context(session)["turns"] == 1, "state must survive a bad request"
