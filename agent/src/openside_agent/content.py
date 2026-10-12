"""Map wire content parts (PP-14) to Pydantic AI user content."""

from __future__ import annotations

import base64
import binascii
from collections.abc import Sequence

from pydantic_ai.messages import BinaryContent, UserContent

from openside_agent.protocol import ContentPart, UserMessage

# Kinds every multimodal provider adapter in Pydantic AI accepts as BinaryContent. Other
# documents become a note, so an unsupported media type cannot fail the whole request.
_BINARY_MIMES = frozenset({"application/pdf"})


def _note(part: ContentPart, why: str) -> str:
    label = part.name or part.artifact_ref or part.type
    where = f" (artifact {part.artifact_ref})" if part.artifact_ref else ""
    return f"[attachment {label}: {part.mime}{where}; {why}]"


def user_content(parts: Sequence[ContentPart]) -> list[UserContent]:
    content: list[UserContent] = []
    for part in parts:
        if part.data_base64 is None:
            content.append(_note(part, "not inlined"))
            continue
        try:
            data = base64.b64decode(part.data_base64, validate=True)
        except binascii.Error:
            content.append(_note(part, "unreadable"))
            continue
        if part.type == "text":
            label = part.name or part.artifact_ref or "text"
            content.append(f"Attachment {label}:\n{data.decode('utf-8', 'replace')}")
        elif part.type in ("image", "audio") or part.mime in _BINARY_MIMES:
            content.append(
                BinaryContent(data, media_type=part.mime, identifier=part.artifact_ref)
            )
        else:
            content.append(_note(part, "not supported as model input"))
    return content


def user_prompt(message: UserMessage) -> str | list[UserContent]:
    if not message.parts:
        return message.text
    return [message.text, *user_content(message.parts)]
