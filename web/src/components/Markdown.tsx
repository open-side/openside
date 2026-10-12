import { Check, Copy, Image as ImageIcon } from "lucide-react";
import { memo, useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import remarkGfm from "remark-gfm";
import { copyText, ICON_STROKE, IconButton } from "../ui";
import { BlockScanner, type OpenCode } from "./markdownBlocks";
import { imageChip, safeUrl } from "./markdownPolicy";

const REMARK_PLUGINS = [remarkGfm];

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
      label={copied ? "Copied" : "Copy code"}
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
    return <a {...props} target="_blank" rel="noopener noreferrer" />;
  },
  // Images never load: see imageChip.
  img({ node: _node, src, alt }: ComponentProps<"img"> & { node?: unknown }) {
    const { label, href } = imageChip(typeof src === "string" ? src : undefined, alt, window.location.origin);
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
  },
  table({ node: _node, ...props }: ComponentProps<"table"> & { node?: unknown }) {
    return (
      // Focusable so a keyboard user can scroll a table wider than the bubble.
      <div className="table-scroll" tabIndex={0} role="group" aria-label="Table">
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
