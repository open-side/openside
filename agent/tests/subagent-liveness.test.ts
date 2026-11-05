// PP-24 ②: random interleavings of child waits, approvals, cancels, steers and restarts
// never leave a parent `delegate` call waiting forever.
import { beforeEach, expect, it } from "bun:test";
import type { AgentState, JsonObject } from "../src/protocol/types.ts";
import { Session } from "../src/runtime/session.ts";
import { TERMINAL } from "../src/runtime/state.ts";
import type { RpcRouter } from "../src/transport/rpc.ts";
import {
	type FakeDaemon,
	hermeticEnv,
	installScript,
	type Wire as Message,
	textResponse,
	toolSpec,
	withFakeDaemon,
} from "./helpers/fake-daemon.ts";

type Action =
	| { kind: "resolve"; index: number; approve: boolean; crash: boolean }
	| { kind: "cancel"; index: number }
	| { kind: "steer"; index: number }
	| { kind: "restart" };

interface Ticket {
	call: string;
	subagent: string | undefined;
	state: "pending" | "approved" | "denied" | "cancelled";
	delivered: boolean;
	claimed: boolean;
}

const EXAMPLES = 40;
const SEED = 0x5eed_0024;

function delegateSpec() {
	return {
		name: "delegate",
		description: "Delegate a task to a subagent",
		parameters: {
			type: "object",
			properties: {
				task: { type: "string" },
				model: { type: "string" },
				tools: { type: "array", items: { type: "string" } },
				context: { type: "string", enum: ["fresh", "fork"] },
			},
			required: ["task"],
			additionalProperties: false,
		},
		approval: "read",
		deferrable: true,
	};
}

function call(name: string, id: string, args: JsonObject = {}) {
	return { type: "tool_call", name, args, id };
}

/** Deterministic stand-in for the former property-based generator (mulberry32). */
function random(seed: number): () => number {
	let state = seed >>> 0;
	return () => {
		state = (state + 0x6d2b79f5) >>> 0;
		let value = state;
		value = Math.imul(value ^ (value >>> 15), value | 1);
		value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
		return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
	};
}

function generate(next: () => number): { children: number[]; actions: Action[] } {
	const int = (low: number, high: number) => low + Math.floor(next() * (high - low + 1));
	const children = Array.from({ length: int(1, 2) }, () => int(1, 2));
	const actions: Action[] = Array.from({ length: int(0, 8) }, (): Action => {
		switch (int(0, 3)) {
			case 0:
				return { kind: "resolve", index: int(0, 7), approve: next() < 0.5, crash: next() < 0.5 };
			case 1:
				return { kind: "cancel", index: int(0, 3) };
			case 2:
				return { kind: "steer", index: int(0, 3) };
			default:
				return { kind: "restart" };
		}
	});
	return { children, actions };
}

function writeScript(children: number[]): void {
	const subagents: Record<string, Message[]> = {};
	children.forEach((steps, index) => {
		const id = `del-${index}`;
		subagents[id] = [
			...Array.from({ length: steps }, (_, step) => ({ parts: [call("probe", `${id}-call-${step}`)] })),
			textResponse(`${id} done`),
		];
	});
	installScript(
		[{ parts: Object.keys(subagents).map(id => call("delegate", id, { task: id })) }, textResponse("parent done")],
		{ subagents },
	);
}

/** Test stand-in for the daemon's ticket, subagent and recovery rules. */
class Daemon {
	tickets = new Map<string, Ticket>();
	subagents = new Map<string, string>();
	executions = new Map<string, number>();
	snapshot: AgentState | undefined;
	finals = 0;
	errors: Message[] = [];
	peer!: FakeDaemon;
	session!: Session;
	rpc!: RpcRouter;

	attach(peer: FakeDaemon, session: Session, rpc: RpcRouter): void {
		this.peer = peer;
		this.session = session;
		this.rpc = rpc;
	}

	async open(): Promise<Message> {
		return this.peer.open({ tools: [toolSpec(), delegateSpec()], resume_state: this.snapshot });
	}

	/** Everything the sidecar emits after the kill point is lost, like SIGKILL. */
	async stop(): Promise<void> {
		await this.peer.send("shutdown", {});
		await this.peer.until("bye");
	}

	/** The daemon's recovery after a restart: re-cancel, then redeliver or interrupt tickets. */
	async recover(ready: Message): Promise<void> {
		for (const [subagentId, status] of this.subagents) {
			if (status === "cancelled") await this.peer.send("subagent_cancel", { subagent_id: subagentId });
		}
		for (const pending of ready.pending_deferred as Message[]) {
			const ticket = this.tickets.get(pending.ticket_id)!;
			if (ticket.state === "pending" || ticket.state === "cancelled") continue;
			let resolution: JsonObject;
			if (ticket.delivered) {
				resolution = { status: "denied", reason: "Interrupted" };
			} else {
				resolution = Daemon.resolution(ticket);
				ticket.delivered = true;
			}
			await this.deliver(pending.ticket_id, ticket, resolution);
		}
	}

	static resolution(ticket: Ticket): JsonObject {
		return ticket.state === "approved" ? { status: "approved" } : { status: "denied", reason: "Rejected" };
	}

	async deliver(ticketId: string, ticket: Ticket, resolution: JsonObject): Promise<void> {
		await this.peer.send("ticket_resolved", { ticket_id: ticketId, tool_call_id: ticket.call, resolution });
	}

	idle(): boolean {
		return !this.session.parentRunning && !this.session.subs.foregroundActive();
	}

	async pump(quietTicks = 2): Promise<void> {
		let ticks = 0;
		for (let round = 0; round < 2000; round++) {
			const message = (await this.peer.tryReceive(20)) as Message | undefined;
			if (message === undefined) {
				ticks = this.idle() && this.rpc.pendingCount === 0 ? ticks + 1 : 0;
				if (ticks >= quietTicks) return;
				continue;
			}
			ticks = 0;
			await this.handle(message);
		}
		throw new Error("Sidecar never went quiet");
	}

	async handle(message: Message): Promise<void> {
		if (message.type === "error") {
			this.errors.push(message);
		} else if (message.type === "state_snapshot") {
			this.snapshot = message.state;
		} else if (message.type === "event") {
			const event = message.event;
			if (event.kind === "assistant.final" && event.text === "parent done") {
				this.finals += 1;
			} else if (event.kind === "subagent.lifecycle") {
				const current = this.subagents.get(event.subagent_id);
				if (current === undefined || !TERMINAL.has(current)) this.subagents.set(event.subagent_id, event.status);
			}
		} else if (message.type === "tool_call") {
			await this.peer.result(message, this.toolOutcome(message));
		}
	}

	toolOutcome(message: Message): JsonObject {
		if (message.name === "delegate") {
			const subagentId = `sub-${message.tool_call_id}`;
			this.subagents.set(subagentId, "running");
			return {
				status: "done",
				for_model: { subagent_id: subagentId, model: "test:child", tools: ["probe"], context: "fresh" },
			};
		}
		const subagentId: string | undefined = message.subagent;
		if (subagentId !== undefined && this.subagents.get(subagentId) === "cancelled") {
			return { status: "denied", reason: "Subagent cancelled" };
		}
		const claim: string | undefined = message.approval_ticket;
		if (claim !== undefined) {
			const ticket = this.tickets.get(claim)!;
			if (ticket.state !== "approved" || ticket.claimed) return { status: "denied", reason: "Not approved" };
			ticket.claimed = true;
			this.executions.set(claim, (this.executions.get(claim) ?? 0) + 1);
			return { status: "done", for_model: "probed" };
		}
		const ticketId = `t${this.tickets.size}`;
		this.tickets.set(ticketId, {
			call: message.tool_call_id,
			subagent: subagentId,
			state: "pending",
			delivered: false,
			claimed: false,
		});
		return { status: "deferred", kind: "approval", ticket_id: ticketId };
	}

	pendingTickets(): string[] {
		return [...this.tickets]
			.filter(([, ticket]) => ticket.state === "pending")
			.map(([id]) => id)
			.sort();
	}

	liveSubagents(): string[] {
		return [...this.subagents]
			.filter(([, status]) => !TERMINAL.has(status))
			.map(([id]) => id)
			.sort();
	}

	async resolve(ticketId: string, approve: boolean): Promise<void> {
		const ticket = this.tickets.get(ticketId)!;
		ticket.state = approve ? "approved" : "denied";
		ticket.delivered = true;
		await this.deliver(ticketId, ticket, Daemon.resolution(ticket));
	}

	/** Applies one action; true when the sidecar must be restarted next. */
	async act(action: Action): Promise<boolean> {
		if (action.kind === "resolve") {
			const pending = this.pendingTickets();
			if (pending.length === 0) return false;
			await this.resolve(pending[action.index % pending.length]!, action.approve);
			return action.crash;
		}
		if (action.kind === "restart") return true;
		const live = this.liveSubagents();
		if (live.length === 0) return false;
		const subagentId = live[action.index % live.length]!;
		if (action.kind === "cancel") {
			this.subagents.set(subagentId, "cancelled");
			for (const ticket of this.tickets.values()) {
				if (ticket.subagent === subagentId && ticket.state === "pending") {
					ticket.state = "cancelled";
					ticket.delivered = true;
				}
			}
			await this.peer.send("subagent_cancel", { subagent_id: subagentId });
		} else {
			await this.peer.send("subagent_steer", { subagent_id: subagentId, text: "steer", control_id: "liveness" });
		}
		return false;
	}
}

async function scenario(children: number[], actions: Action[]): Promise<void> {
	const daemon = new Daemon();
	const queue = [...actions];
	let first = true;
	let finished = false;
	while (!finished) {
		await withFakeDaemon(
			(conn, rpc) => {
				const session = new Session(conn, rpc);
				daemon.attach(daemon.peer, session, rpc);
				return { session, done: session.serve() };
			},
			async (peer, session) => {
				daemon.attach(peer, session, daemon.rpc);
				const ready = await daemon.open();
				if (first) {
					first = false;
					await peer.user();
				} else {
					await daemon.recover(ready);
				}
				await daemon.pump();
				while (queue.length > 0) {
					const restart = await daemon.act(queue.shift()!);
					if (restart) {
						await daemon.stop();
						return;
					}
					await daemon.pump();
				}
				for (let round = 0; round < 20; round++) {
					const pending = daemon.pendingTickets();
					if (pending.length === 0 && daemon.idle() && daemon.finals > 0) break;
					for (const ticketId of pending) await daemon.resolve(ticketId, true);
					await daemon.pump(3);
				}
				await daemon.stop();
				finished = true;
			},
		);
	}
	expect(daemon.errors).toEqual([]);
	expect(daemon.finals).toBe(1);
	const state = JSON.parse(daemon.snapshot!.data) as Message;
	expect(state.pending_delegates ?? []).toEqual([]);
	expect(state.pending_deferred ?? []).toEqual([]);
	for (const child of state.subagents as Message[]) expect(TERMINAL.has(child.status)).toBe(true);
	const returns = new Map<string, number>();
	for (const message of state.messages as Message[]) {
		if (message.role === "toolResult" && message.toolCallId.startsWith("del-")) {
			returns.set(message.toolCallId, (returns.get(message.toolCallId) ?? 0) + 1);
		}
	}
	expect(Object.fromEntries(returns)).toEqual(Object.fromEntries(children.map((_, index) => [`del-${index}`, 1])));
	for (const count of daemon.executions.values()) expect(count).toBe(1);
}

beforeEach(() => {
	hermeticEnv();
});

it(
	"test_random_interleavings_never_deadlock",
	async () => {
		const next = random(SEED);
		for (let example = 0; example < EXAMPLES; example++) {
			const { children, actions } = generate(next);
			writeScript(children);
			try {
				await scenario(children, actions);
			} catch (error) {
				throw new Error(`example ${example}: ${JSON.stringify({ children, actions })}`, { cause: error });
			}
		}
	},
	{ timeout: 300_000 },
);
