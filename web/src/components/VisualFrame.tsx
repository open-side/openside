import { Check, CodeXml, Copy } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import type { Visual } from "../api/types";
import { useClient } from "../context";
import { copyText, ICON_STROKE, IconButton, useToast } from "../ui";
import { htmlDocument, htmlPicture, paintedHeight, svgPicture } from "./visualImage";
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

/**
 * The page's text colour and the column width, read from the element and followed through theme
 * and size changes: a visual's image has to be redrawn for either.
 */
function useInkAndWidth(element: RefObject<HTMLElement | null>): { ink: string; width: number } {
  const [measure, setMeasure] = useState({ ink: "", width: 0 });
  useEffect(() => {
    const node = element.current;
    if (!node) return;
    const read = () => {
      const next = { ink: getComputedStyle(node).color || "currentColor", width: Math.round(node.clientWidth) };
      setMeasure((current) => (current.ink === next.ink && current.width === next.width ? current : next));
    };
    read();
    const theme = new MutationObserver(read);
    theme.observe(document.documentElement, { attributes: true });
    const scheme = window.matchMedia?.("(prefers-color-scheme: dark)");
    scheme?.addEventListener("change", read);
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(read);
    resize?.observe(node);
    return () => {
      theme.disconnect();
      scheme?.removeEventListener("change", read);
      resize?.disconnect();
    };
  }, [element]);
  return measure;
}

type Picture = { url: string; width?: number; height?: number } | "failed" | null;

/** A visual block as an inert image, sized to its content; a clear note when it cannot be drawn. */
function VisualPicture({ visual, label }: { visual: Visual; label: string }) {
  const box = useRef<HTMLDivElement>(null);
  const { ink, width } = useInkAndWidth(box);
  const [picture, setPicture] = useState<Picture>(null);
  useEffect(() => {
    if (!ink) return;
    if (visual.kind === "svg") {
      setPicture(svgPicture(visual.source, ink) ?? "failed");
      return;
    }
    if (!width) return;
    let current = true;
    const xhtml = htmlDocument(visual.source, ink);
    void paintedHeight(xhtml, width)
      .then((painted) => {
        // Content sized to the viewport has no height of its own; it gets a 16:10 page.
        const height = painted ?? Math.round((width * 10) / 16);
        if (current) setPicture(height > 0 ? { url: htmlPicture(xhtml, width, height), width, height } : "failed");
      })
      .catch(() => {
        if (current) setPicture("failed");
      });
    return () => {
      current = false;
    };
  }, [visual.kind, visual.source, ink, width]);

  return (
    <div className="visual-picture" ref={box}>
      {picture === "failed" ? (
        <p className="visual-failed" role="note">
          Couldn’t show this visual. Its source is under Source.
        </p>
      ) : picture ? (
        <img
          className="visual-image"
          src={picture.url}
          alt={label}
          draggable={false}
          {...(picture.width ? { width: picture.width } : {})}
          {...(picture.height ? { height: picture.height } : {})}
          onError={() => setPicture("failed")}
        />
      ) : null}
    </div>
  );
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
        <VisualPicture visual={visual} label={label} />
      )}
    </figure>
  );
}
