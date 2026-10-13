"""Terminal front outputs retain daemon-owned message validation and publication."""

from typing import Any

from pydantic_ai import StructuredDict, ToolOutput

from openside_agent.protocol import ToolSpec

OUTPUT_NAMES = frozenset({"send_message", "stay_silent"})


def output_types(tools: list[ToolSpec]) -> list[ToolOutput[dict[str, Any]]]:
    outputs = [
        ToolOutput(
            StructuredDict(
                {"type": "object", "properties": {}, "additionalProperties": False}
            ),
            name="stay_silent",
            description="End this turn without sending a message to the person.",
        )
    ]
    send = next((tool for tool in tools if tool.name == "send_message"), None)
    if send is not None:
        outputs.insert(
            0,
            ToolOutput(
                StructuredDict(send.parameters),
                name="send_message",
                description=send.description
                + " Ends the turn; send at most one message.",
            ),
        )
    return outputs
