import { Check, Copy, FileText, Globe, Image as ImageIcon } from "lucide-react";
import { createContext, memo, useContext, useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import remarkGfm from "remark-gfm";
import { useFavicon } from "../hooks/useFavicon";
import { t } from "../i18n";
import { copyText, ICON_STROKE, IconButton } from "../ui";
import { ArtifactImage } from "./ArtifactImage";
import { BlockScanner, type OpenCode } from "./markdownBlocks";
import { imageChip, safeUrl } from "./markdownPolicy";
import { useSplitPane } from "./splitPaneState";

/** A page a web search or web read of this chat returned: the link in a reply that names it gets its icon. */
export interface Citation {
  title: string | null;
  /** The daemon's normalized PNG data URL, drawn as is. */
  favicon: string | null;
  /** An origin the daemon has the icon of, loaded through the client. */
  origin: string | null;
}

export interface MarkdownScope {
  /** The chat whose files relative links and images name; null where there is none (a shared copy). */
  sessionId: string | null;
  /** Keyed by {@link citationKey}. */
  citations: ReadonlyMap<string, Citation>;
}

export const MarkdownContext = createContext<MarkdownScope>({ sessionId: null, citations: new Map() });

/** One key for the same page however the model wrote it: no fragment, no trailing slash. */
export function citationKey(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    parsed.hash = "";
    return parsed.href.replace(/\/$/, "");
  } catch {
    return null;
  }
}

/**
 * A file of the chat named by a relative link ("chart.png", "out/report.md"): no scheme, not
 * rooted, no way out of the chat's artifact folder. Segments are checked after decoding, so an
 * encoded "%2e%2e" or "%2f" cannot smuggle a parent step past the check. Returns the decoded
 * path without a leading "./".
 */
export function artifactPath(href: string | undefined): string | null {
  if (!href || /^[a-z][a-z0-9+.-]*:/i.test(href)) return null;
  let path: string;
  try {
    path = decodeURIComponent(href.split(/[?#]/)[0] ?? "");
  } catch {
    return null;
  }
  path = path.replace(/^\.\//, "");
  if (!path || /^[/\\]/.test(path) || path.includes("\\") || path.includes("\0")) return null;
  if (path.split("/").some((part) => part === "" || part === "." || part === "..")) return null;
  return path;
}

function CitationIcon({ citation }: { citation: Citation }) {
  const fetched = useFavicon(citation.favicon ? null : citation.origin);
  const src = citation.favicon ?? fetched;
  return src ? <img className="citation-icon" src={src} alt="" width={13} height={13} /> : <Globe className="citation-icon" size={13} strokeWidth={ICON_STROKE} aria-hidden="true" />;
}

function MarkdownLink({ href, children, ...props }: ComponentProps<"a">) {
  const scope = useContext(MarkdownContext);
  const split = useSplitPane();
  const file = scope.sessionId ? artifactPath(href) : null;
  if (file) {
    // A file of this chat opens beside it; without the pane (a phone, a shared copy) it is plain text.
    if (!split) return <span className="md-file">{children}</span>;
    return (
      <a
        {...props}
        href={href}
        className="md-file"
        onClick={(event) => {
          event.preventDefault();
          split.show({ kind: "file", path: file });
        }}
      >
        <FileText size={13} strokeWidth={ICON_STROKE} aria-hidden="true" />
        {children}
      </a>
    );
  }
  const key = href ? citationKey(href) : null;
  const citation = key ? scope.citations.get(key) : undefined;
  if (citation) {
    return (
      <a {...props} href={href} className="citation" target="_blank" rel="noopener noreferrer" title={citation.title ?? href}>
        <CitationIcon citation={citation} />
        {children}
      </a>
    );
  }
  return (
    <a {...props} href={href} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

function MarkdownImage({ src, alt }: { src: string | undefined; alt: string | undefined }) {
  const { sessionId } = useContext(MarkdownContext);
  const file = sessionId ? artifactPath(src) : null;
  // An image the agent made or was given in this chat loads through the client, like attachments.
  if (file && sessionId) {
    return (
      <span className="md-artifact-image">
        <ArtifactImage sessionId={sessionId} path={file} alt={alt?.trim() || file.split("/").pop() || file} />
      </span>
    );
  }
  // Everything else never loads: see imageChip.
  const { label, href } = imageChip(src, alt, window.location.origin, t("thread.markdown.image"));
  const content = (
    <>
      <ImageIcon size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span>{label}</span>
    </>
  );
  return href ? (
    <a className="md-image" href={href} target="_blank" rel="noopener noreferrer" title={href}>
      {content}
    </a>
  ) : (
    <span className="md-image" role="img" aria-label={label}>
      {content}
    </span>
  );
}

const REMARK_PLUGINS = [remarkGfm];
/** "1,234", "-3.5", "$12", "42%": a cell that is only a number, perhaps with a currency or percent sign. */
const NUMERIC_CELL = /^[+\-−]?[$€£¥₩]?\d[\d,]*(?:\.\d+)?%?$/;

interface HastNode {
  type: string;
  value?: string;
  tagName?: string;
  properties?: { className?: unknown };
  children?: HastNode[];
}

function textOf(node: HastNode | undefined): string {
  if (!node) return "";
  if (node.type === "text") return node.value ?? "";
  return (node.children ?? []).map(textOf).join("");
}

function CopyCode({ text }: { text: () => string }) {
  const [copied, setCopied] = useState(false);
  return (
    <IconButton
      label={copied ? t("common.copied") : t("thread.markdown.copyCode")}
      size="small"
      onClick={() =>
        void copyText(text().replace(/\n$/, "")).then((ok) => {
          if (!ok) return;
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        })
      }
    >
      {copied ? <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> : <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />}
    </IconButton>
  );
}

// `text` is a getter so a streaming block never joins its body just to render.
function CodeBlock({ language, text, children }: { language: string | undefined; text: () => string; children: ReactNode }) {
  return (
    <div className="code-block">
      <div className="code-block-head">
        <span>{language ?? "text"}</span>
        <CopyCode text={text} />
      </div>
      <pre tabIndex={0}>{children}</pre>
    </div>
  );
}

const components: Components = {
  pre({ node, children }) {
    const code = node?.children.find((child) => child.type === "element" && child.tagName === "code");
    const classes = code?.type === "element" ? code.properties.className : undefined;
    const language = (Array.isArray(classes) ? classes : [])
      .map(String)
      .find((name) => name.startsWith("language-"))
      ?.slice("language-".length);
    return (
      <CodeBlock language={language} text={() => textOf(code)}>
        {children}
      </CodeBlock>
    );
  },
  a({ node: _node, ...props }: ComponentProps<"a"> & { node?: unknown }) {
    return <MarkdownLink {...props} />;
  },
  img({ node: _node, src, alt }: ComponentProps<"img"> & { node?: unknown }) {
    return <MarkdownImage src={typeof src === "string" ? src : undefined} alt={alt} />;
  },
  table({ node: _node, ...props }: ComponentProps<"table"> & { node?: unknown }) {
    return (
      // Focusable so a keyboard user can scroll a table wider than the bubble.
      <div className="table-scroll" tabIndex={0} role="group" aria-label={t("thread.markdown.table")}>
        <table {...props} />
      </div>
    );
  },
  // A column the author did not align keeps numbers flush right so their digits line up.
  td({ node, ...props }: ComponentProps<"td"> & { node?: unknown }) {
    const numeric = !props.style?.textAlign && NUMERIC_CELL.test(textOf(node as HastNode | undefined).trim());
    return <td {...props} {...(numeric ? { "data-numeric": "" } : {})} />;
  },
};

type RehypePlugin = NonNullable<Options["rehypePlugins"]>[number];

// highlight.js grammars are a third of the bundle. Prose renders at once without them; code
// blocks gain colours when the chunk arrives (text and layout do not change).
let highlight: RehypePlugin[] = [];
const highlightReady = import("rehype-highlight").then((module) => {
  highlight = [module.default];
});

function useHighlight(): RehypePlugin[] {
  const [, setReady] = useState(highlight.length > 0);
  useEffect(() => {
    if (highlight.length) return;
    let live = true;
    void highlightReady.then(() => live && setReady(true));
    return () => {
      live = false;
    };
  }, []);
  return highlight;
}

/**
 * Assistant prose. Raw HTML in the text is not rendered (react-markdown's default), so model
 * output can never inject markup; visuals go through sandboxed frames instead.
 */
export function Markdown({ text }: { text: string }) {
  const rehypePlugins = useHighlight();
  const scanner = useRef<BlockScanner | null>(null);
  scanner.current ??= new BlockScanner();
  const { segments, tail, code } = scanner.current.view(text);
  return (
    <div className="markdown">
      {segments.map((segment, index) => (
        <MarkdownChunk key={index} text={segment} plugins={rehypePlugins} />
      ))}
      {tail ? <MarkdownChunk text={tail} plugins={rehypePlugins} /> : null}
      {code ? <StreamingCode code={code} /> : null}
    </div>
  );
}

// Finished chunks are memoized by their text: a delta re-parses only the open tail.
const MarkdownChunk = memo(function MarkdownChunk({ text, plugins }: { text: string; plugins: RehypePlugin[] }) {
  return (
    <ReactMarkdown remarkPlugins={REMARK_PLUGINS} rehypePlugins={plugins} components={components} urlTransform={safeUrl}>
      {text}
    </ReactMarkdown>
  );
});

// A very large code block still being written: plain text in fixed-size memoized chunks, since
// only the last chunk changes per delta. It is parsed and highlighted once the fence closes.
function StreamingCode({ code }: { code: OpenCode }) {
  return (
    <CodeBlock language={code.language} text={() => code.chunks.join("") + code.rest}>
      <code>
        {code.chunks.map((chunk, index) => (
          <CodeText key={index} text={chunk} />
        ))}
        {code.rest}
      </code>
    </CodeBlock>
  );
}

const CodeText = memo(function CodeText({ text }: { text: string }) {
  return <>{text}</>;
});
