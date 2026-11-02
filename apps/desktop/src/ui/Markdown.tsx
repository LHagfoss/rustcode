// Reply Markdown, styled after MarkdownText.swift: 13px body, bold headings,
// mono inline code without a box, code blocks on a quiet fill with a Copy
// button and Xcode colours, tables and quotes on rounded-xs fills.

import { Check, Copy } from "lucide-react";
import {
  type ComponentProps,
  memo,
  type ReactElement,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { type FileRef, parseInlineRef, parseLinkRef, resolveRef } from "../lib/fileRefs";
import { highlight, peekHighlight } from "../lib/highlight";
import { markdownBlocks } from "../lib/markdownBlocks";
import { openExternal, openInEditor } from "../lib/native";
import { hasShell } from "../lib/shell";
import { toast, useApp } from "../state/store";
import { LinkChip } from "./LinkChip";
import { copyText, cx } from "./primitives";

export function CopyButton({
  text,
  label = "Copy",
  iconOnly,
}: {
  text: string;
  label?: string;
  iconOnly?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <button
      type="button"
      aria-label={copied ? "Copied" : label}
      onClick={() => void copyText(text).then(() => setCopied(true))}
      className={cx(
        "inline-flex h-[22px] items-center gap-1 rounded-md text-caption text-fg-muted hover:bg-fill hover:text-fg",
        iconOnly ? "w-[22px] justify-center" : "px-2",
      )}
    >
      {copied ? <Check size={12} className="text-success" /> : <Copy size={12} />}
      {!iconOnly && <span>{copied ? "Copied" : label}</span>}
    </button>
  );
}

export function CodeBlock({ code, lang }: { code: string; lang?: string }) {
  // Highlighted before (the row remounted, or the same snippet): show it in the first frame.
  const [html, setHtml] = useState<string | null>(() => peekHighlight(code, lang) ?? null);
  const highlighted = useRef(html !== null);
  useEffect(() => {
    const cached = peekHighlight(code, lang);
    if (cached !== undefined) {
      highlighted.current = cached !== null;
      setHtml(cached);
      return;
    }
    let live = true;
    // Streaming code re-highlights at most a few times a second.
    const t = setTimeout(
      () => {
        void highlight(code, lang).then(
          (out) => {
            if (!live) return;
            highlighted.current = out !== null;
            setHtml(out);
          },
          () => undefined,
        );
      },
      highlighted.current ? 150 : 0,
    );
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [code, lang]);
  return (
    <div className="my-0.5 overflow-hidden rounded-lg bg-fill-subtle">
      <div className="flex items-center justify-between pt-1.5 pr-1.5 pl-3">
        <span className="font-mono text-caption text-fg-muted">{lang || "code"}</span>
        <CopyButton text={code} />
      </div>
      {html ? (
        <div
          className="selectable overflow-x-auto px-3 pt-1.5 pb-3 font-mono text-callout leading-[18px] [&_pre]:m-0"
          // Shiki escapes the code; the HTML holds only spans with colour variables.
          // biome-ignore lint/security/noDangerouslySetInnerHtml: highlighted, escaped output
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre className="selectable m-0 overflow-x-auto px-3 pt-1.5 pb-3 font-mono text-callout leading-[18px]">
          {code}
        </pre>
      )}
    </div>
  );
}

/** Opens a file a reply names, resolved against the open thread's folder. */
function openRef(ref: FileRef) {
  const s = useApp.getState();
  const cwd = (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd;
  if (!hasShell()) {
    toast("Opening files needs the desktop app.", "warning");
    return;
  }
  openInEditor(resolveRef(ref, cwd), ref.line, ref.column).catch((e) =>
    toast(e instanceof Error ? e.message : String(e), "warning"),
  );
}

function shortUrl(href: string, children: ReactNode): ReactNode {
  if (typeof children !== "string" || children !== href) return children;
  try {
    const url = new URL(href);
    return `${url.host}${url.pathname === "/" ? "" : url.pathname}`;
  } catch {
    return children;
  }
}

const components: Components = {
  h1: ({ children }) => <h1 className="mt-1 font-bold text-title2">{children}</h1>,
  h2: ({ children }) => <h2 className="mt-1 font-bold text-title3">{children}</h2>,
  h3: ({ children }) => <h3 className="font-semibold text-body">{children}</h3>,
  h4: ({ children }) => <h4 className="font-semibold text-body">{children}</h4>,
  p: ({ children }) => <p className="leading-[20px]">{children}</p>,
  ul: ({ children }) => <ul className="ml-[1.1em] list-disc space-y-0.5 marker:text-fg-muted">{children}</ul>,
  ol: ({ children }) => (
    <ol className="ml-[1.4em] list-decimal space-y-0.5 marker:text-fg-muted">{children}</ol>
  ),
  li: ({ children }) => <li className="pl-0.5 leading-[20px] [&>ol]:mt-0.5 [&>ul]:mt-0.5">{children}</li>,
  a: ({ href, children }) =>
    // A bare link (autolinked, or `<url>`) reads as a chip; a link with its own words stays a link.
    href && /^https?:\/\//i.test(href) && typeof children === "string" && children === href ? (
      <LinkChip url={href} />
    ) : (
      <a
        href={href}
        onClick={(e) => {
          e.preventDefault();
          if (!href) return;
          // A relative link names a file in the thread's folder, not a page.
          const ref = parseLinkRef(href);
          if (ref) openRef(ref);
          else openExternal(href);
        }}
        className="cursor-pointer text-accent underline decoration-accent/45 underline-offset-2"
      >
        {href ? shortUrl(href, children) : children}
      </a>
    ),
  blockquote: ({ children }) => (
    <blockquote className="relative rounded-md bg-fill-subtle py-2 pr-3 pl-[21px] text-fg/80 before:absolute before:top-2 before:bottom-2 before:left-2 before:w-[3px] before:rounded-full before:bg-fg-muted/45">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-1 border-border" />,
  table: ({ children }) => (
    <div className="overflow-x-auto rounded-lg bg-fill-subtle py-1.5">
      <table className="w-full border-collapse text-left">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border-b border-border px-2.5 py-1.5 font-semibold">{children}</th>,
  td: ({ children }) => <td className="px-2.5 py-1.5 align-top">{children}</td>,
  pre: ({ children }) => <>{children}</>,
  code: ({ className, children, node }: ComponentProps<"code"> & { node?: { position?: unknown } }) => {
    const text = String(children ?? "");
    const lang = /language-([\w+#.-]+)/.exec(className ?? "")?.[1];
    // A fenced block has a language class or spans lines; inline code has neither.
    const block = lang !== undefined || text.includes("\n");
    void node;
    if (block) return <CodeBlock code={text.replace(/\n$/, "")} lang={lang} />;
    const ref = parseInlineRef(text);
    if (ref)
      return (
        <button
          type="button"
          title={`Open ${ref.path}${ref.line ? ` at line ${ref.line}` : ""}`}
          onClick={() => openRef(ref)}
          className="inline rounded-xs text-accent decoration-accent/45 underline-offset-2 hover:underline"
        >
          <code className="font-mono text-[0.95em]">{children}</code>
        </button>
      );
    return <code className="font-mono text-[0.95em]">{children}</code>;
  },
};

const plugins = [remarkGfm];

// Rendered blocks by their source. react-markdown's `Markdown` is a plain
// function (no hooks), so its element tree can be kept: a row scrolled back
// into view, or a finished block above a streaming one, is not parsed again.
const rendered = new Map<string, ReactElement>();
const RENDERED_MAX = 800;

export function renderBlock(text: string): ReactElement {
  const hit = rendered.get(text);
  if (hit) {
    // Most recently used goes last; the oldest is evicted first.
    rendered.delete(text);
    rendered.set(text, hit);
    return hit;
  }
  const element = ReactMarkdown({ children: text, remarkPlugins: plugins, components });
  rendered.set(text, element);
  if (rendered.size > RENDERED_MAX) rendered.delete(rendered.keys().next().value as string);
  return element;
}

const Block = memo(function Block({ text }: { text: string }) {
  return renderBlock(text);
});

export const Markdown = memo(function Markdown({
  text,
  className,
  streaming,
}: {
  text: string;
  className?: string;
  /** Growing: parse it block by block, so a chunk re-parses only the last block. */
  streaming?: boolean;
}) {
  // A finished text is one block (one parse, cached by its text); a streaming one is split so the
  // blocks above the one being written keep their elements. Splitting costs a parser per block.
  const blocks = useMemo(() => (streaming ? markdownBlocks(text) : [text]), [text, streaming]);
  return (
    <div className={cx("selectable flex flex-col gap-2.5 text-body break-words", className)}>
      {blocks.map((block, i) => (
        <Block key={i} text={block} />
      ))}
    </div>
  );
});
