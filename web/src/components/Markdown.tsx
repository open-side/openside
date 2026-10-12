import { Check, Copy, Image as ImageIcon } from "lucide-react";
import { useEffect, useState, type ComponentProps } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import remarkGfm from "remark-gfm";
import { copyText, ICON_STROKE, IconButton } from "../ui";

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

function CopyCode({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <IconButton
      label={copied ? "Copied" : "Copy code"}
      size="small"
      onClick={() =>
        void copyText(text).then((ok) => {
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

const components: Components = {
  pre({ node, children, ...props }) {
    const code = node?.children.find((child) => child.type === "element" && child.tagName === "code");
    const classes = code?.type === "element" ? code.properties.className : undefined;
    const language = (Array.isArray(classes) ? classes : [])
      .map(String)
      .find((name) => name.startsWith("language-"))
      ?.slice("language-".length);
    return (
      <div className="code-block">
        <div className="code-block-head">
          <span>{language ?? "text"}</span>
          <CopyCode text={textOf(code).replace(/\n$/, "")} />
        </div>
        <pre {...props} tabIndex={0}>
          {children}
        </pre>
      </div>
    );
  },
  a({ node: _node, ...props }: ComponentProps<"a"> & { node?: unknown }) {
    return <a {...props} target="_blank" rel="noopener noreferrer" />;
  },
  // Model output never makes the page fetch: an image would bypass web/src/api (no auth, no
  // 401 handling) and external hosts violate img-src. Show its alt text instead; an absolute
  // http(s) image stays one tap away as a plain link the reader opens deliberately.
  img({ node: _node, src, alt }: ComponentProps<"img"> & { node?: unknown }) {
    const label = alt?.trim() || "Image";
    const content = (
      <>
        <ImageIcon size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span>{label}</span>
      </>
    );
    const external = typeof src === "string" && /^https?:\/\//i.test(src) && !src.startsWith(`${window.location.origin}/`);
    return external ? (
      <a className="md-image" href={src} target="_blank" rel="noopener noreferrer" title={src}>
        {content}
      </a>
    ) : (
      <span className="md-image" role="img" aria-label={label}>
        {content}
      </span>
    );
  },
  table({ node: _node, ...props }: ComponentProps<"table"> & { node?: unknown }) {
    return (
      <div className="table-scroll">
        <table {...props} />
      </div>
    );
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
  return (
    <div className="markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={rehypePlugins} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
