// Incremental block splitter for streamed Markdown. A growing assistant message is re-rendered
// on every delta; parsing all of it each time is O(n^2). The scanner cuts the text at top-level
// block boundaries so finished segments keep their identity (and their React.memo) while only the
// unfinished tail is parsed again.

/** Finished segments are cut no finer than this, so memoized components stay few. */
const SEGMENT_MIN = 2048;
/** An open code fence larger than this is streamed as plain text chunks instead of re-parsed. */
export const STREAM_CODE_MIN = 16 * 1024;
const CODE_CHUNK = 4096;

// Link/footnote definitions can resolve references anywhere in the document, and HTML blocks of
// kinds 1-5 swallow blank lines: either makes a cut unsafe, so splitting is abandoned.
const GLOBAL_CONTEXT = /^ {0,3}(?:\[[^\]]+\]:|<(?:!|\?|\/?(?:pre|script|style|textarea)\b))/i;
const LIST_MARKER = /^(?:[-+*]|\d{1,9}[.)])(?:[ \t]|$)/;
const FENCE_OPEN = /^( *)(`{3,}|~{3,})(.*)$/;

export interface OpenCode {
  language: string | undefined;
  /** Finished chunks of the code body, in order. */
  chunks: string[];
  /** Body text after the last chunk, including a partial line. */
  rest: string;
}

export interface MarkdownView {
  /** Finished segments; each is parsed once. */
  segments: string[];
  /** Markdown after the last finished segment (before a streamed code block, when `code`). */
  tail: string;
  /** Set while a large top-level fence is still open. */
  code: OpenCode | null;
}

interface Fence {
  char: string;
  length: number;
  topLevel: boolean;
  /** Offset of the opening fence line. */
  start: number;
  language: string | undefined;
}

export class BlockScanner {
  private source = "";
  private pos = 0;
  private segments: string[] = [];
  private segStart = 0;
  private blankAt = -1;
  private fence: Fence | null = null;
  private chunks: string[] = [];
  private chunkStart = 0;
  private whole = false;

  view(text: string): MarkdownView {
    this.advance(text);
    if (this.whole) return { segments: [], tail: text, code: null };
    const fence = this.fence;
    if (fence?.topLevel && text.length - this.chunkStart + this.chunks.length * CODE_CHUNK >= STREAM_CODE_MIN) {
      return {
        segments: this.segments,
        tail: text.slice(this.segStart, fence.start),
        code: { language: fence.language, chunks: this.chunks, rest: text.slice(this.chunkStart) },
      };
    }
    return { segments: this.segments, tail: text.slice(this.segStart), code: null };
  }

  private reset() {
    this.source = "";
    this.pos = 0;
    this.segments = [];
    this.segStart = 0;
    this.blankAt = -1;
    this.fence = null;
    this.chunks = [];
    this.chunkStart = 0;
    this.whole = false;
  }

  private advance(text: string) {
    const known = this.source;
    // Deltas only append; a cheap window check stands in for a full prefix comparison.
    const window = Math.min(known.length, 64);
    if (
      text.length < known.length ||
      (window > 0 && text.slice(known.length - window, known.length) !== known.slice(known.length - window))
    ) {
      this.reset();
    }
    this.source = text;
    if (this.whole) return;
    for (;;) {
      const newline = text.indexOf("\n", this.pos);
      if (newline < 0) return;
      const start = this.pos;
      this.pos = newline + 1;
      this.line(text, start, newline);
      if (this.whole) return;
    }
  }

  private line(text: string, start: number, end: number) {
    const fence = this.fence;
    if (fence) {
      const raw = text.slice(start, end);
      const closing = raw.trimStart();
      if (
        closing.startsWith(fence.char.repeat(fence.length)) &&
        closing.trimEnd().replaceAll(fence.char, "") === ""
      ) {
        this.fence = null;
        this.chunks = [];
        // A top-level fence ends its block, so a large finished code block never rides along
        // with the text streaming after it.
        if (fence.topLevel && end + 1 - this.segStart >= SEGMENT_MIN) this.cut(text, end + 1, end + 1);
      } else if (end + 1 - this.chunkStart >= CODE_CHUNK) {
        this.chunks.push(text.slice(this.chunkStart, end + 1));
        this.chunkStart = end + 1;
      }
      return;
    }
    if (/^\s*$/.test(text.slice(start, end))) {
      if (this.blankAt < 0) this.blankAt = start;
      return;
    }
    const first = text.charCodeAt(start);
    const body = text.slice(start, end);
    if (GLOBAL_CONTEXT.test(body)) {
      this.whole = true;
      return;
    }
    if (this.blankAt >= 0) {
      const indented = first === 32 || first === 9;
      if (!indented && !LIST_MARKER.test(body) && this.blankAt - this.segStart >= SEGMENT_MIN) {
        this.cut(text, this.blankAt, start);
      }
      this.blankAt = -1;
    }
    const open = FENCE_OPEN.exec(body);
    const [, indent = "", marker = "", info = ""] = open ?? [];
    if (marker && !(marker.startsWith("`") && info.includes("`"))) {
      this.fence = {
        char: marker.charAt(0),
        length: marker.length,
        topLevel: indent.length === 0,
        start,
        language: info.trim().split(/\s+/)[0] || undefined,
      };
      this.chunks = [];
      this.chunkStart = end + 1;
    }
  }

  private cut(text: string, segmentEnd: number, next: number) {
    this.segments = [...this.segments, text.slice(this.segStart, segmentEnd)];
    this.segStart = next;
    this.blankAt = -1;
  }
}
