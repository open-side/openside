import asyncio
import json

from runtime_helpers import fake_daemon, install_script, text_response

from openside_agent.runtime.state import SnapshotData


async def test_title_uses_a_separate_scripted_model_and_is_not_requested_after_resume(
    tmp_path, monkeypatch
):
    path = install_script(
        tmp_path,
        monkeypatch,
        [text_response("First reply"), text_response("Second reply")],
    )
    script = json.loads(path.read_text())
    script["title"] = "Compare Acme plans"
    path.write_text(json.dumps(script))
    async with fake_daemon() as (daemon, session):
        await daemon.open()
        await daemon.user(text="https://shop.example/plans?private=token")
        await daemon.until("settled")
        await session.title_task
        if not any(
            item["type"] == "event"
            and item["event"]["kind"] == "session.title_proposed"
            for item in daemon.outputs
        ):
            await daemon.until("event", kind="session.title_proposed")
        titles = [
            item["event"]["title"]
            for item in daemon.outputs
            if item["type"] == "event"
            and item["event"]["kind"] == "session.title_proposed"
        ]
        assert titles == ["Compare Acme plans"]
        snapshot = daemon.snapshots()[-1]
        saved = SnapshotData.model_validate_json(snapshot["data"])
        assert saved.title_attempted
        assert saved.script_cursor == 1
    async with fake_daemon() as (daemon, _):
        await daemon.open(resume_state=snapshot)
        await daemon.user(message_id="second", text="Keep my name")
        await daemon.until("settled")
        assert not any(
            item["type"] == "event"
            and item["event"]["kind"] == "session.title_proposed"
            for item in daemon.outputs
        )
        finals = [
            item["event"]["text"]
            for item in daemon.outputs
            if item["type"] == "event" and item["event"]["kind"] == "assistant.final"
        ]
        assert finals == ["Second reply"]


async def test_naming_failure_does_not_fail_a_completed_reply(tmp_path, monkeypatch):
    install_script(tmp_path, monkeypatch, [text_response("Done")])
    async with fake_daemon() as (daemon, session):
        await daemon.open()

        async def unavailable(*args, **kwargs):
            raise RuntimeError("private provider diagnostic")

        monkeypatch.setattr(session.title_model, "request", unavailable)
        await daemon.user()
        await daemon.until("settled")
        await session.title_task
        results = [item for item in daemon.outputs if item["type"] == "message_result"]
        assert results[0]["outcome"] == "completed"
        assert not any(item["type"] == "error" for item in daemon.outputs)
        assert "private provider diagnostic" not in json.dumps(daemon.outputs)
        assert SnapshotData.model_validate_json(
            daemon.snapshots()[-1]["data"]
        ).title_attempted


async def test_slow_naming_does_not_delay_settle_and_is_cancelled_on_shutdown(
    tmp_path, monkeypatch
):
    install_script(tmp_path, monkeypatch, [text_response("Done")])
    entered = asyncio.Event()
    cancelled = asyncio.Event()
    async with fake_daemon() as (daemon, session):
        await daemon.open()

        async def slow(*args, **kwargs):
            entered.set()
            try:
                await asyncio.Event().wait()
            finally:
                cancelled.set()

        monkeypatch.setattr(session.title_model, "request", slow)
        await daemon.user()
        async with asyncio.timeout(1):
            await entered.wait()
            await daemon.until("settled")
        assert session.title_task is not None and not session.title_task.done()
    assert cancelled.is_set()


async def test_named_conversations_do_not_spend_a_naming_request(tmp_path, monkeypatch):
    install_script(tmp_path, monkeypatch, [text_response("Done")])
    async with fake_daemon() as (daemon, session):
        await daemon.open(agent={"title_needed": False})
        await daemon.user()
        await daemon.until("settled")
        assert session.title_task is None
        assert not any(
            item["type"] == "event"
            and item["event"]["kind"] == "session.title_proposed"
            for item in daemon.outputs
        )
        assert SnapshotData.model_validate_json(
            daemon.snapshots()[-1]["data"]
        ).title_attempted


async def test_naming_keeps_the_goal_from_a_request_with_media(tmp_path, monkeypatch):
    install_script(tmp_path, monkeypatch, [text_response("I checked the chart")])
    async with fake_daemon() as (daemon, session):
        await daemon.open()
        original = session.title_model.request
        seen = []

        async def capture(messages, *args, **kwargs):
            seen.extend(messages)
            return await original(messages, *args, **kwargs)

        monkeypatch.setattr(session.title_model, "request", capture)
        await daemon.send(
            "user_message",
            message_id="with-chart",
            text="Compare the plans in this chart",
            mode="now",
            parts=[
                {
                    "type": "image",
                    "mime": "image/png",
                    "name": "chart.png",
                    "data_base64": (
                        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ"
                        "AAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
                    ),
                }
            ],
        )
        await daemon.until("settled")
        await session.title_task
        assert "Compare the plans in this chart" in str(seen)
