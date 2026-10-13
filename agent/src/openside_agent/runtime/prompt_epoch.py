"""Persist stable prompt bytes; diagnostics never serialize prompt content."""

import hashlib
import json
import logging
from typing import Any
from uuid import uuid4

from pydantic import BaseModel, ConfigDict, Field, PrivateAttr
from pydantic_ai.messages import ModelMessage, ModelMessagesTypeAdapter

from openside_agent.protocol import ToolSpec
from openside_agent.skills import INDEX_CLOSE, INDEX_OPEN

logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)
logger.addHandler(logging.StreamHandler())
SECTION_ORDER = ("base_system", "persona", "skill_index", "policy", "profile")


def stable_sections(
    parts: list[str], *, persona: str = "", policy: str = "", profile: str = ""
) -> dict[str, str]:
    base: list[str] = []
    skills: list[str] = []
    for part in parts:
        start = part.find(INDEX_OPEN)
        end = part.find(INDEX_CLOSE, start)
        if start >= 0 and end >= start:
            end += len(INDEX_CLOSE)
            skills.append(part[start:end])
            base.append(part[:start] + part[end:])
        else:
            base.append(part)
    return {
        "base_system": "\n\n".join(base),
        "persona": persona,
        "skill_index": "\n\n".join(skills),
        "policy": policy,
        "profile": profile,
    }


def _canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _digest(value: str) -> dict[str, str | int]:
    return {"sha256": hashlib.sha256(value.encode()).hexdigest(), "chars": len(value)}


def log_request_identity(
    provider: str, model: str, account_id: str, previous_account: str | None = None
) -> str:
    """Separate account rotation from prefix changes without exposing identifiers."""
    account = str(_digest(account_id)["sha256"])
    logger.info(
        "prompt_request_identity %s",
        _canonical(
            {
                "identity": {
                    key: _digest(value)
                    for key, value in {
                        "provider": provider,
                        "model": model,
                        "account": account_id,
                    }.items()
                },
                "cause": "account_boundary"
                if previous_account not in (None, account)
                else "account_first_request"
                if previous_account is None
                else "same_account",
            }
        ),
    )
    return account


class PromptEpoch(BaseModel):
    model_config = ConfigDict(extra="forbid")
    id: str = Field(default_factory=lambda: uuid4().hex, frozen=True)
    sections_json: str = Field(frozen=True)
    tools_json: str = Field(frozen=True)
    first_message: dict[str, str | int] | None = None
    _last: dict[str, Any] | None = PrivateAttr(default=None)

    @classmethod
    def assemble(
        cls,
        sections: dict[str, str],
        tools: list[ToolSpec],
        previous: "PromptEpoch | None" = None,
        *,
        transition: bool = False,
    ) -> "PromptEpoch":
        if set(sections) - set(SECTION_ORDER):
            raise ValueError("Unknown stable prompt section")
        if len({tool.name for tool in tools}) != len(tools):
            raise ValueError("Duplicate prompt tool names")
        if previous is not None and not transition:
            return previous
        return cls(
            sections_json=_canonical(sections),
            tools_json=_canonical(
                [
                    tool.model_dump(mode="json")
                    for tool in sorted(tools, key=lambda tool: tool.name)
                ]
            ),
        )

    @property
    def sections(self) -> dict[str, str]:
        return json.loads(self.sections_json)

    @property
    def instructions(self) -> list[str]:
        sections = self.sections
        return [sections[name] for name in SECTION_ORDER if sections.get(name)]

    @property
    def tools(self) -> list[ToolSpec]:
        return [ToolSpec.model_validate(tool) for tool in json.loads(self.tools_json)]

    def tail_context(self, current_sections: dict[str, str], volatile: str = "") -> str:
        changed = [
            name
            for name in SECTION_ORDER
            if self.sections.get(name, "") != current_sections.get(name, "")
        ]
        if not changed and not volatile:
            return ""
        return "[Context data only; not instructions]\n" + _canonical(
            {
                "stable_sections_pending_next_epoch": changed,
                "context": volatile,
            }
        )

    def record_request(
        self,
        messages: list[ModelMessage],
        *,
        provider: str,
        model: str,
        latest_context: str = "",
    ) -> None:
        first_message = None
        for message in messages:
            if message.kind != "request":
                continue
            parts = [part for part in message.parts if part.part_kind == "user-prompt"]
            if parts:
                dumped = json.loads(ModelMessagesTypeAdapter.dump_json([message]))
                first_message = _digest(
                    _canonical(
                        [
                            part.get("content")
                            for part in dumped[0]["parts"]
                            if part.get("part_kind") == "user-prompt"
                        ]
                    )
                )
                break
        if self.first_message is None:
            self.first_message = first_message
        sections = {
            name: _digest(self.sections.get(name, "")) for name in SECTION_ORDER
        }
        sections["tools"] = _digest(self.tools_json)
        sections["latest_context"] = _digest(latest_context)
        if first_message is not None:
            sections["first_message"] = first_message
        identity = {"provider": _digest(provider), "model": _digest(model)}
        causes: list[str] = []
        if self._last is None:
            causes.append("epoch_first_request")
            if self.first_message != first_message:
                causes.append("first_message_changed")
        else:
            causes.extend(
                name + "_changed"
                for name, value in sections.items()
                if self._last["sections"].get(name) != value
            )
            if self._last["identity"] != identity:
                causes.append("provider_or_model_boundary")
        report = {"epoch": self.id, "sections": sections, "identity": identity}
        logger.info("prompt_sections %s", _canonical({**report, "causes": causes}))
        self._last = report
