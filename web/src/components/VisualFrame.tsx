import { Check, CodeXml, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import type { Visual } from "../api/types";
import { useClient } from "../context";
import { copyText, ICON_STROKE, IconButton, useToast } from "../ui";
import "./browser.css";

/** Blob URLs keep previews on the same transport as API calls, including an encrypted relay. */
export function SandboxFrame({ src, title }: { src: string; title: string }) {
  const client = useClient();
  const [preview, setPreview] = useState<{ src: string; url: string } | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    if (!client.relay) return;
    const controller = new AbortController();
    let url: string | undefined;
    setError(false);
    void client.sandboxPreview(src, controller.signal).then((blob) => {
      if (controller.signal.aborted) return;
      url = URL.createObjectURL(blob);
      setPreview({ src, url });
    }).catch(() => {
      if (!controller.signal.aborted) setError(true);
    });
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, src]);
  if (client.relay && error) return <p role="alert">Could not load the preview. Try opening it again.</p>;
  return <iframe className="sandbox-frame" sandbox="" src={client.relay ? (preview?.src === src ? preview.url : undefined) : client.resolve(src)} title={title} referrerPolicy="no-referrer" />;
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
    <figure className="visual" data-visual={visual.id} data-kind={visual.kind}>
      <figcaption>
        <span className="visual-kind">{label}</span>
        <button type="button" className="visual-toggle" aria-pressed={showSource} onClick={() => setShowSource(!showSource)}>
          <CodeXml size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {showSource ? "Preview" : "Source"}
        </button>
        <IconButton label={copied ? "Source copied" : "Copy source"} size="small" onClick={() => void copy()}>
          {copied ? (
            <Check size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          ) : (
            <Copy size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          )}
        </IconButton>
      </figcaption>
      {showSource ? (
        <pre className="visual-source" tabIndex={0}>
          {visual.source}
        </pre>
      ) : (
        <SandboxFrame src={visual.preview_url} title={label} />
      )}
    </figure>
  );
}
