import type { Socket, UnixSocketOptions } from "bun";
import { type Inbound, MAX_LINE_BYTES, type OutboundBody } from "../protocol/types";
import { validateInbound } from "../protocol/validate";

export type UnixConnector = (options: UnixSocketOptions<undefined>) => Promise<Socket<undefined>>;

export class LineTooLarge extends Error {
	constructor() {
		super("Outgoing protocol line exceeds the size limit");
		this.name = "LineTooLarge";
	}
}

export class ConnectionClosed extends Error {
	constructor() {
		super("Sidecar connection closed");
		this.name = "ConnectionClosed";
	}
}

/** What the reader hands to `receive`, in arrival order. */
type Frame = { kind: "line"; bytes: Uint8Array } | { kind: "oversize" } | { kind: "eof"; partial: boolean };

interface PendingWrite {
	data: Uint8Array;
	offset: number;
	resolve: () => void;
	reject: (error: Error) => void;
}

// Reading pauses once this many complete frames wait for `receive`, so a flooding peer
// is pushed back by the kernel socket buffer instead of growing our heap. Bytes already
// delivered past that point are parked unparsed until `receive` drains the queue.
const READ_HIGH_WATER = 64;
const NEWLINE = 10;
const decoder = new TextDecoder("utf-8", { fatal: true });
const encoder = new TextEncoder();

/**
 * JSONL framing over the daemon's Unix socket (docs/specs/sidecar-protocol.md §2):
 * one UTF-8 JSON object per line, at most MAX_LINE_BYTES excluding the newline.
 * Consumed line buffers are zeroed because credential leases travel on this socket.
 */
export class JsonlConnection {
	#socket: Socket<undefined> | undefined;
	#chunks: Uint8Array[] = [];
	#buffered = 0;
	#discarding = false;
	#frames: Frame[] = [];
	#frameWaiter: (() => void) | undefined;
	#writes: PendingWrite[] = [];
	#lastWrite: Promise<void> = Promise.resolve();
	#closed = false;
	#eof = false;
	#parked: Uint8Array | undefined;
	#endParked = false;

	private constructor() {}

	/** `connector` is a test seam for injecting socket failures; production uses Bun.connect. */
	static async connect(socketPath: string, connector: UnixConnector = Bun.connect): Promise<JsonlConnection> {
		const connection = new JsonlConnection();
		connection.#socket = await connector({
			unix: socketPath,
			socket: {
				binaryType: "uint8array",
				data: (_socket, chunk) => connection.#onData(chunk),
				drain: () => connection.#flush(),
				end: () => connection.#onEnd(),
				close: () => connection.#onEnd(),
				error: () => connection.#onEnd(),
			},
		});
		return connection;
	}

	/** Serialize, frame and write one message; resolves once the socket accepted every byte. */
	send(message: OutboundBody): Promise<void> {
		const payload = encoder.encode(JSON.stringify({ v: 1, ...message }));
		if (payload.byteLength > MAX_LINE_BYTES) {
			payload.fill(0);
			return Promise.reject(new LineTooLarge());
		}
		if (this.#closed) {
			payload.fill(0);
			return Promise.reject(new ConnectionClosed());
		}
		const data = new Uint8Array(payload.byteLength + 1);
		data.set(payload);
		data[payload.byteLength] = NEWLINE;
		payload.fill(0);
		const { promise, resolve, reject } = Promise.withResolvers<void>();
		this.#writes.push({ data, offset: 0, resolve, reject });
		this.#lastWrite = promise.catch(() => undefined);
		if (this.#writes.length === 1) this.#flush();
		return promise;
	}

	/**
	 * Next valid daemon message, or null at end of stream. Invalid or oversized lines are
	 * answered with a non-fatal `protocol` error and skipped, as the protocol requires.
	 */
	async receive(): Promise<Inbound | null> {
		for (;;) {
			const frame = await this.#nextFrame();
			if (frame.kind === "eof") {
				if (frame.partial) await this.#protocolError();
				return null;
			}
			if (frame.kind === "oversize") {
				await this.#protocolError();
				continue;
			}
			const message = parseLine(frame.bytes);
			if (message === undefined) {
				await this.#protocolError();
				continue;
			}
			return message;
		}
	}

	async close(): Promise<void> {
		for (const chunk of this.#chunks) chunk.fill(0);
		this.#chunks = [];
		this.#buffered = 0;
		this.#parked?.fill(0);
		this.#parked = undefined;
		for (const frame of this.#frames) if (frame.kind === "line") frame.bytes.fill(0);
		this.#frames = [];
		if (this.#closed) return;
		// Let queued lines (bye, final snapshot) reach the daemon before closing.
		await this.#lastWrite;
		this.#closed = true;
		this.#socket?.end();
	}

	#protocolError(): Promise<void> {
		return this.send({
			type: "error",
			code: "protocol",
			message: "Invalid protocol message discarded",
			fatal: false,
		}).catch(() => undefined);
	}

	#nextFrame(): Promise<Frame> {
		const frame = this.#frames.shift();
		if (frame !== undefined) {
			if (this.#frames.length < READ_HIGH_WATER) this.#unpark();
			return Promise.resolve(frame);
		}
		const { promise, resolve } = Promise.withResolvers<Frame>();
		this.#frameWaiter = () => {
			this.#frameWaiter = undefined;
			resolve(this.#nextFrame());
		};
		return promise;
	}

	#push(frame: Frame): void {
		this.#frames.push(frame);
		if (this.#frames.length >= READ_HIGH_WATER) this.#socket?.pause();
		this.#frameWaiter?.();
	}

	#onData(chunk: Uint8Array): void {
		if (this.#parked !== undefined) {
			// Chunks the kernel delivered after the pause queue behind the parked bytes.
			const joined = new Uint8Array(this.#parked.byteLength + chunk.byteLength);
			joined.set(this.#parked);
			joined.set(chunk, this.#parked.byteLength);
			this.#parked.fill(0);
			chunk.fill(0);
			this.#parked = joined;
			return;
		}
		this.#consume(chunk);
	}

	#consume(chunk: Uint8Array): void {
		let start = 0;
		for (;;) {
			const newline = chunk.indexOf(NEWLINE, start);
			if (newline < 0) break;
			if (this.#frames.length >= READ_HIGH_WATER) {
				this.#parked = chunk.slice(start);
				chunk.fill(0);
				this.#socket?.pause();
				return;
			}
			const piece = chunk.subarray(start, newline);
			if (this.#discarding) {
				this.#discarding = false;
			} else if (this.#buffered + piece.byteLength > MAX_LINE_BYTES) {
				this.#push({ kind: "oversize" });
			} else {
				this.#push({ kind: "line", bytes: this.#take(piece) });
			}
			this.#dropBuffered();
			start = newline + 1;
		}
		if (start < chunk.byteLength && !this.#discarding) {
			const rest = chunk.slice(start);
			this.#chunks.push(rest);
			this.#buffered += rest.byteLength;
			if (this.#buffered > MAX_LINE_BYTES) {
				this.#push({ kind: "oversize" });
				this.#dropBuffered();
				this.#discarding = true;
			}
		}
		chunk.fill(0);
	}

	/** Parse parked bytes once the queue has room; resume the socket when none remain. */
	#unpark(): void {
		const parked = this.#parked;
		if (parked !== undefined) {
			this.#parked = undefined;
			this.#consume(parked);
			if (this.#parked !== undefined) return;
			if (this.#endParked) {
				this.#endParked = false;
				this.#pushEof();
				return;
			}
		}
		if (this.#frames.length < READ_HIGH_WATER) this.#socket?.resume();
	}

	#take(last: Uint8Array): Uint8Array {
		const line = new Uint8Array(this.#buffered + last.byteLength);
		let offset = 0;
		for (const chunk of this.#chunks) {
			line.set(chunk, offset);
			offset += chunk.byteLength;
		}
		line.set(last, offset);
		return line;
	}

	#dropBuffered(): void {
		for (const chunk of this.#chunks) chunk.fill(0);
		this.#chunks = [];
		this.#buffered = 0;
	}

	#flush(): void {
		const socket = this.#socket;
		while (socket !== undefined && this.#writes.length > 0) {
			const write = this.#writes[0] as PendingWrite;
			let written: number;
			try {
				written = socket.write(write.data.subarray(write.offset));
			} catch {
				written = -1;
			}
			// A negative count means the socket is closed; zero means wait for `drain`.
			if (written < 0) {
				this.#failWrites();
				return;
			}
			if (written === 0) return;
			write.offset += written;
			if (write.offset < write.data.byteLength) return;
			this.#writes.shift();
			write.data.fill(0);
			write.resolve();
		}
	}

	#onEnd(): void {
		if (!this.#eof) {
			this.#eof = true;
			// End of stream queues behind parked lines so none of them is lost.
			if (this.#parked !== undefined) this.#endParked = true;
			else this.#pushEof();
		}
		this.#failWrites();
	}

	#pushEof(): void {
		const partial = this.#buffered > 0 && !this.#discarding;
		this.#dropBuffered();
		this.#push({ kind: "eof", partial });
	}

	/** Reject every queued write; later sends fail fast with ConnectionClosed. */
	#failWrites(): void {
		if (this.#closed) return;
		this.#closed = true;
		for (const write of this.#writes.splice(0)) {
			write.data.fill(0);
			write.reject(new ConnectionClosed());
		}
	}
}

function parseLine(bytes: Uint8Array): Inbound | undefined {
	let value: unknown;
	try {
		value = JSON.parse(decoder.decode(bytes));
	} catch {
		return undefined;
	} finally {
		bytes.fill(0);
	}
	const result = validateInbound(value);
	return result.ok ? result.value : undefined;
}
