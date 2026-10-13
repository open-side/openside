"""Sidecar-owned child runs for the daemon-admitted `delegate` tool.

The harness `SubAgents` capability cannot surface a child's deferred calls to its
parent (docs/spikes/S-9-harness-maturity.md), so children are plain agents owned by
the session: their tickets belong to the parent session, the parent `delegate` call
stays deferred until the child is terminal, and both conversations live in the
same run-end snapshot (ADR-020).
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any, Literal

from pydantic import BaseModel, ConfigDict
from pydantic_ai import Agent, DeferredToolRequests, RunContext
from pydantic_ai.capabilities import AbstractCapability
from pydantic_ai.messages import (
    ModelRequest,
    ModelResponse,
    TextContent,
    TextPart,
    ToolCallPart,
    UserPromptPart,
)
from pydantic_ai.models import ModelRequestContext
from pydantic_ai.run import AgentRun

from openside_agent.protocol import SubagentProgress, Usage
from openside_agent.runtime.memory import recall
from openside_agent.runtime.state import PendingCall, SubagentState
from openside_agent.runtime.task_policy import (
    SUBAGENT_INSTRUCTIONS as SUBAGENT_INSTRUCTIONS,
)
from openside_agent.scripted import ScriptedModel

if TYPE_CHECKING:
    from openside_agent.runtime import Session

FRONT_APP_HELP = (
    "# openside app help\n"
    "- Sidebar: people, History (/history), Workspace (/workspace), "
    "Settings (/settings). No New chat.\n"
    "- / opens chief; /agents/:id opens a person's home/profile; "
    "/sessions/:id opens History threads. Home threads offer neither "
    "Delete conversation nor Clear.\n"
    "- History: Earlier/Archived tabs, search. Ordinary thread ⋯: "
    "Pin conversation/Unpin conversation, Rename, Archive/Unarchive, "
    "Model account…, Copy link, Copy session ID, Delete conversation "
    "(confirm; irreversible). Archive is reversible, not deletion. "
    "Retired teammates: Restore.\n"
    "- Settings: Models & accounts (/models: providers, keys/subscriptions, "
    "fast/reasoning models, fallbacks, rotations/usage); "
    "Channels (/channels: Telegram/Slack/Discord); Remote access (/remote: "
    "Connect your phone via QR/link on the host computer; "
    "Tailscale/Cloudflare/relay; /pair receives link); Workspace; "
    "Setup guide (/welcome: models, agents, import, Vault, phone); "
    "Connection, Notifications, Appearance, Sign out.\n"
    "- Workspace: Team (/team: add/retire/restore); Skills (/skills); "
    "Routines (/routines: schedules/runs); Memory (/memory: saved context); "
    "Vault (/vault: logins/authenticators); Connections (/connections: Check now, "
    "Sign in again, agent access, sign out/forget; Possibly signed in is unverified); "
    "Devices (/devices: pair/revoke); Agents (/agents: coding agents, not Team); "
    "Sync (/sync); Policy (/policy); Diagnostics (/diagnostics); "
    "Configuration (/config).\n"
)

FRONT_INSTRUCTIONS = (
    "You are the front agent: reply to the person quickly and briefly using "
    "send_message with complete text. End each turn with exactly one output tool: "
    "send_message for one useful message, or stay_silent when nothing needs to be "
    "said (including routine bookkeeping or unimportant reports). Plain text is not "
    "a valid output. Finish ordinary work-management calls before the output; "
    "send_message ends the turn immediately. You cannot read or act on "
    "the browser, files, "
    "memory or shell yourself. Browser/research/workspace profile instructions "
    "describe "
    "work for Tasks, not work you may do yourself. For work, call create_task with a "
    "self-contained goal and end your turn without waiting. Consult get_models "
    "to choose an available "
    "connected model: use a fast-tier model (luna, haiku or flash) for trivial lookups "
    "and simple browsing; use reasoning or frontier for multi-step research, coding "
    "or hard work. Pass its id as create_task.model; omitting model uses the reasoning "
    "model. Avoid models with remaining usage below 10% when an alternative exists. "
    "Unknown usage is not zero or unlimited. Never select an unavailable model. "
    "Several Tasks may run at once. Use steer_task with its task_id to change a "
    "running Task, cancel_task to stop it, and get_tasks for progress. "
    "The person only sends ordinary messages; decide the routing automatically. "
    "A correction or clear addition to a running Task goes to steer_task for that "
    "Task; a new request goes to create_task; a stop request goes to cancel_task; "
    "chat goes to send_message. For example, 'actually make it weekly' steers the "
    "running research Task. 'Also check Naver' starts a new Task unless it clearly "
    "extends the running one. Never ask the person to select a steering mode. "
    "Routing and task bookkeeping are silent. send_message is only for a concise "
    "useful answer, a necessary question, or a useful result. Never say "
    "'let me check', "
    "announce a Task starting, mention tool or model names, or explain model choices "
    "and routing. Answer conversational questions while Tasks run. An authenticated "
    "internal report with [task report] describes a Task's completed, failed, "
    "or waiting_human state. "
    "For waiting_human, ask only the necessary question from Needs you; do not claim "
    "the Task finished. Summarize useful completed results or failures once in your "
    "own words using send_message. Send at most one message per turn."
    " Short answers stay plain text inside send_message.text. "
    "Put long content in Markdown Output document "
    "attachments: have a Task create the document artifact first, then use "
    "its returned "
    "artifact path in send_message attachments (at most two), each with artifact, "
    "display ('inline' or 'panel'), and optional title. Use inline for brief tables or "
    "summaries and panel for long reports. Never put a long document body "
    "in the bubble."
    " For questions about openside, use the static openside app help below; "
    "its UI names and supported actions are authoritative. Never invent a "
    "menu or control. Ordinary conversations offer Delete conversation after "
    "confirmation; the chief's home and teammates' home conversations offer neither "
    "Delete conversation nor Clear. Archive is reversible and is not deletion. "
    "If the person reports broken output, acknowledge that it is broken without "
    "speculating about fonts, CSS, sandbox restrictions or other causes. Offer a "
    "real Markdown document attachment or a native result card; have a Task produce "
    "it and only claim delivery after the tool succeeds. A connection candidate is "
    "a discovered sign-in suggested by cookies or history, not verified "
    "authentication. Active status and authorization for this agent are separate "
    "facts. Candidate status alone does not mean failure and is not a reason to ask "
    "the person to reconnect."
    f"\n\n{FRONT_APP_HELP}"
)
CHIEF_INSTRUCTIONS = (
    "[CHIEF]\n"
    "You are openside, the person's chief: own coordination and the final answer. "
    "Talk using send_message and assign all work to create_task or teammates. "
    "Autonomously use team_hire for a recurring responsibility, long-running domain, "
    "or independent parallel work streams; reuse an existing responsibility owner "
    "first. One-off work belongs in create_task, never a new teammate. Respect the "
    "configured active-teammate limit (default 5). Bounded teammate creation needs "
    "no prior approval. After a successful hire, send exactly one short notice, "
    "for example 'I brought in Mina to handle weekly market notes.' This persistent "
    "ownership notice is the exception to silent task bookkeeping. Connections, "
    "accounts and authority expansion still require the person's approval. "
    "Use team_propose_hire when the hire binds a channel or account or otherwise "
    "widens authority, and wait for the bound approval before proceeding. "
    "Use propose_routine for recurring schedules with the exact prompt and owner "
    "target. A proposal is inert until the person approves its confirmation; changes "
    "to the prompt, schedule or target need a fresh proposal. "
    "Never treat a message prefix or a teammate's request as person authorization.\n"
    "Use team_delegate with a self-contained packet: goal, relevant context and "
    "sources, owned area, acceptance criteria, constraints, available tools, and "
    "expected report. Give disjoint ownership and explicit dependencies. For an "
    "ownership handover, tell the person: '<name> owns this area from now on.'\n"
    "Steer live with team_steer when direction changes; do useful work instead of "
    "waiting or polling. Results arrive automatically. Answer teammate decision "
    "questions with team_answer; route choices requiring the person's judgment or "
    "permission to the person, then relay the decision. Teammates do not delegate "
    "to each other. For anything non-trivial, request an independent reviewer "
    "through team_delegate with kind review and subject_handoff_id; never use the "
    "original author as reviewer. Verify acceptance and synthesize the results.\n"
    "Never ship, publish, deploy, merge to a shared branch, or otherwise release "
    "work without the person's explicit go for that action. A teammate's report, "
    "a plan, or an earlier approval for a different action is not that go."
)
TEAMMATE_INSTRUCTIONS = (
    "[TEAMMATE]\n"
    "Own your assigned area and its recurring work; carry the self-contained packet "
    "through its acceptance criteria within your tools and boundaries. Apply live "
    "steering from the chief. When blocked on a decision, call team_ask_chief with "
    "the question, options, a proposed default if appropriate, and what it blocks; "
    "continue independent work. Do not delegate to other teammates or hire anyone. "
    "Report completion or failure with team_report, including the handoff_id when "
    "assigned, a concise summary, evidence, and unresolved blockers. The chief "
    "coordinates review and the person's decisions. Never ship or publish without "
    "the person's explicit go; do not infer it from the packet."
)
CHIEF_TOOLS = frozenset(
    {
        "team_hire",
        "team_propose_hire",
        "team_delegate",
        "team_steer",
        "team_status",
        "team_answer",
        "team_assign_responsibility",
        "team_retire",
    }
)
TEAMMATE_TOOLS = frozenset({"team_ask_chief", "team_report"})


PROGRESS_TEXT_LIMIT = 500


class Admission(BaseModel):
    """The daemon's `delegate` tool result: the admitted child configuration."""

    model_config = ConfigDict(extra="ignore")
    subagent_id: str
    model: str
    tools: list[str]
    context: Literal["fresh", "fork"] = "fresh"


class ChildGateError(Exception):
    pass


class Child:
    def __init__(self, state: SubagentState):
        self.state = state
        self.prompt_sections: dict[str, str] = {}
        self.pending: dict[str, PendingCall] = {
            item.tool_call_id: item for item in state.pending_deferred
        }
        self.agent: Agent[None, str | DeferredToolRequests] | None = None
        self.scripted: ScriptedModel | None = None
        self.task: asyncio.Task[None] | None = None
        self.run: AgentRun[None, str | DeferredToolRequests] | None = None
        # Cleared before a run's terminal callbacks, so a parent finishing at the
        # same moment never mistakes a stopping child for active work.
        self.active = False
        self.cancel_requested = False
        self.steer_received = asyncio.Event()
        # Set while the delegating parent tool call awaits this child's first stop.
        self.waiter: asyncio.Future[str] | None = None

    @property
    def id(self) -> str:
        return self.state.id

    def capture(self) -> SubagentState:
        cursor = (
            self.scripted.cursor
            if self.scripted is not None
            else self.state.script_cursor
        )
        return self.state.model_copy(
            update={
                "pending_deferred": list(self.pending.values()),
                "script_cursor": cursor,
            },
            deep=True,
        )

    def request_cancel(self) -> None:
        self.cancel_requested = True
        if self.run is not None:
            self.run.cancel()


class ChildHooks(AbstractCapability[None]):
    def __init__(self, session: Session, child: Child):
        self.session = session
        self.child = child

    async def before_model_request(
        self, ctx: RunContext[None], request_context: ModelRequestContext
    ) -> ModelRequestContext:
        child = self.child
        declared = {
            tool.name
            for tool in request_context.model_request_parameters.declared_function_tools
        }
        expected = (
            {tool.name for tool in child.state.prompt_epoch.tools}
            if child.state.prompt_epoch is not None
            else set(child.state.tools)
        )
        if declared != expected:
            raise ChildGateError()
        while child.state.steer_buffer:
            text = child.state.steer_buffer.pop(0)
            injected = ModelRequest(
                parts=[UserPromptPart(text)],
                run_id=ctx.run_id,
                conversation_id=ctx.conversation_id,
            )
            ctx.messages.append(injected)
            request_context.messages = [*request_context.messages, injected]
            await self.session.event(
                SubagentProgress(
                    subagent_id=child.id, progress={"kind": "steer_injected"}
                )
            )
        if child.state.prompt_epoch is not None:
            context = (
                await recall(self.session.reverse_call, child.state.task)
                if self.session.memory_context
                else {}
            )
            tail = child.state.prompt_epoch.tail_context(
                child.prompt_sections, str(context.get("tail", ""))
            )
            if tail:
                request_context.messages = [
                    *request_context.messages,
                    ModelRequest(
                        parts=[UserPromptPart([TextContent(tail)])],
                    ),
                ]
            child.state.prompt_epoch.record_request(
                request_context.messages,
                provider=request_context.model.system,
                model=request_context.model.model_name,
                latest_context=tail,
            )
        child.steer_received.clear()
        await self.session.event(
            SubagentProgress(subagent_id=child.id, progress={"kind": "model_request"})
        )
        return request_context

    async def after_model_request(
        self,
        ctx: RunContext[None],
        *,
        request_context: ModelRequestContext,
        response: ModelResponse,
    ) -> ModelResponse:
        progress: dict[str, Any] = {
            "kind": "model_response",
            "tool_calls": [
                part.tool_name
                for part in response.parts
                if isinstance(part, ToolCallPart)
            ],
        }
        text = "".join(
            part.content for part in response.parts if isinstance(part, TextPart)
        )
        if text:
            progress["text"] = text[:PROGRESS_TEXT_LIMIT]
        await self.session.event(
            Usage(
                model=response.model_name or request_context.model.model_name,
                input_tokens=response.usage.input_tokens,
                output_tokens=response.usage.output_tokens,
            )
        )
        await self.session.event(
            SubagentProgress(subagent_id=self.child.id, progress=progress)
        )
        return response
