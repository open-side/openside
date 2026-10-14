import { Check, Copy, FileText } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MessageDocument } from "../api/types";
import { useClient } from "../context";
import { Button, copyText, Dialog, ICON_STROKE, useToast } from "../ui";
import { Markdown } from "./Markdown";
import "./documents.css";

/** The document's Markdown, read once through the session's raw artifact route. */
function useDocumentText(sessionId: string, path: string): string | null | "failed" {
  const client = useClient();
  const [text, setText] = useState<{ path: string; value: string | "failed" } | null>(null);
  useEffect(() => {
    let current = true;
    void client
      .artifactBlob(sessionId, path)
      .then((blob) => blob.text())
      .then(
        (value) => current && setText({ path, value }),
        () => current && setText({ path, value: "failed" }),
      );
    return () => {
      current = false;
    };
  }, [client, sessionId, path]);
  return text?.path === path ? text.value : null;
}

/** First lines of Markdown as plain text, for a panel document's faded preview; the card already shows the title. */
function previewLines(text: string, title: string): string[] {
  return text
    .split("\n")
    .map((line) => line.replace(/^\s{0,3}(#{1,6}\s+|[-*+]\s+|\d+[.)]\s+|>\s?)/, "").replace(/[*_`~]+/g, "").trim())
    .filter((line) => line !== title)
    .filter(Boolean)
    .slice(0, 3);
}

const FAILED = "Couldn’t open this document.";

/** A document shown in place: rendered Markdown, clipped with Show more when it runs long. */
function InlineDocument({ sessionId, document }: { sessionId: string; document: MessageDocument }) {
  const text = useDocumentText(sessionId, document.path);
  const body = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [clipped, setClipped] = useState(false);
  useLayoutEffect(() => {
    const node = body.current;
    if (node && !open) setClipped(node.scrollHeight > node.clientHeight + 1);
  }, [text, open]);
  return (
    <section className="doc-inline" aria-label={document.title} data-open={open} data-clipped={clipped && !open}>
      {text === "failed" ? (
        <p className="doc-failed">{FAILED}</p>
      ) : (
        <div className="doc-inline-body" ref={body} aria-busy={text === null}>
          {text === null ? (
            <span className="doc-skeleton" role="status" aria-label="Loading document">
              <span className="doc-skeleton-bar" aria-hidden="true" />
              <span className="doc-skeleton-bar" aria-hidden="true" />
              <span className="doc-skeleton-bar" aria-hidden="true" />
            </span>
          ) : (
            <Markdown text={text} />
          )}
        </div>
      )}
      {clipped ? (
        <button type="button" className="doc-more" aria-expanded={open} onClick={() => setOpen(!open)}>
          {open ? "Show less" : "Show more"}
        </button>
      ) : null}
    </section>
  );
}

/** A document behind a card: its title and a faded start; it opens beside the thread or as a sheet. */
function PanelDocument({ sessionId, document }: { sessionId: string; document: MessageDocument }) {
  const text = useDocumentText(sessionId, document.path);
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const ready = typeof text === "string" && text !== "failed";

  async function copy() {
    if (ready && (await copyText(text))) {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } else toast.notify("Couldn’t copy the document.", "error");
  }

  return (
    <>
      <button type="button" className="doc-card" aria-haspopup="dialog" onClick={() => setOpen(true)}>
        <span className="doc-card-title">
          <FileText size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {document.title}
        </span>
        <span className="doc-card-preview" aria-hidden="true">
          {text === "failed" ? FAILED : ready ? previewLines(text, document.title).map((line, index) => <span key={index}>{line}</span>) : null}
        </span>
      </button>
      <Dialog open={open} onClose={() => setOpen(false)} size="wide" className="doc-panel" title={document.title}>
        <div className="doc-panel-actions">
          <Button size="compact" disabled={!ready} onClick={() => void copy()}>
            {copied ? <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> : <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />}
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
        <div className="doc-panel-body">{text === "failed" ? <p className="doc-failed">{FAILED}</p> : ready ? <Markdown text={text} /> : null}</div>
      </Dialog>
    </>
  );
}

/** Documents the agent attached to a message, at the top of its bubble. */
export function MessageDocuments({ sessionId, documents }: { sessionId: string; documents: readonly MessageDocument[] }) {
  return (
    <div className="message-documents">
      {documents.map((document) =>
        document.display === "inline" ? (
          <InlineDocument key={document.path} sessionId={sessionId} document={document} />
        ) : (
          <PanelDocument key={document.path} sessionId={sessionId} document={document} />
        ),
      )}
    </div>
  );
}
