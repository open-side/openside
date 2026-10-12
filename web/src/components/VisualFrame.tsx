import { Check, CodeXml, Copy } from "lucide-react";
import { useState } from "react";
import type { Visual } from "../api/types";
import { useClient } from "../context";
import { copyText, ICON_STROKE, IconButton, useToast } from "../ui";
import "./browser.css";

/**
 * Renders a daemon-signed preview in an iframe with an empty sandbox: no scripts and an
 * opaque origin, on top of the daemon's own CSP sandbox for preview responses.
 */
export function SandboxFrame({ src, title }: { src: string; title: string }) {
  const client = useClient();
  return <iframe className="sandbox-frame" sandbox="" src={client.resolve(src)} title={title} referrerPolicy="no-referrer" />;
}

export function VisualFrame({ visual }: { visual: Visual }) {
  const toast = useToast();
  const [showSource, setShowSource] = useState(false);
  const [copied, setCopied] = useState(false);
  const label = `${visual.kind.toUpperCase()} visual`;

  async function copy() {
    if (await copyText(visual.source)) {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } else toast.notify("Could not copy the source.", "error");
  }

  return (
    <figure className="visual" data-visual={visual.id}>
      <figcaption>
        <button type="button" className="visual-toggle" aria-pressed={showSource} onClick={() => setShowSource(!showSource)}>
          <CodeXml size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {showSource ? "Preview" : "Source"}
        </button>
        <span className="visual-kind">{label}</span>
        <IconButton label={copied ? "Source copied" : "Copy source"} size="small" onClick={() => void copy()}>
          {copied ? (
            <Check size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          ) : (
            <Copy size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          )}
        </IconButton>
      </figcaption>
      {showSource ? <pre className="visual-source" tabIndex={0}>{visual.source}</pre> : <SandboxFrame src={visual.preview_url} title={label} />}
    </figure>
  );
}
