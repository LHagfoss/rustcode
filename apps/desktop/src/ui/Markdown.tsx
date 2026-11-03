// Reply Markdown, styled after MarkdownText.swift: body text, bold headings,
// mono inline code on a quiet rounded fill, code blocks on a quiet fill with a Copy
// button and Xcode colours, tables and quotes on rounded-xs fills.

import { Check, Copy } from "lucide-react";
import {
  type ComponentProps,
  memo,
  type ReactElement,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import ReactMarkdown, { type Components, defaultUrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";
import { type FileRef, parseInlineRef, parseLinkRef, resolveRef } from "../lib/fileRefs";
import { highlight, peekHighlight } from "../lib/highlight";
import { markdownBlocks } from "../lib/markdownBlocks";
import { openExternal, openInEditor } from "../lib/native";
import { rehypeReveal, revealedBefore } from "../lib/reveal";
import { hasShell } from "../lib/shell";
import { toast, useApp } from "../state/store";
import { LinkChip } from "./LinkChip";
import { MarkdownImage, MermaidBlock } from "./MarkdownMedia";
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
    toast("Opening files needs the desktop app.", "info");
    return;
  }
  openInEditor(resolveRef(ref, cwd), ref.line, ref.column).catch((e) =>
    toast(e instanceof Error ? e.message : String(e), "error"),
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
        className="-mx-1 cursor-pointer rounded-full px-1 text-accent underline decoration-accent/30 underline-offset-2 transition-colors duration-150 hover:bg-accent/12 hover:decoration-transparent"
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
  img: ({ src, alt }) => <MarkdownImage src={typeof src === "string" ? src : undefined} alt={alt} />,
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
    if (block && lang === "mermaid") {
      const source = text.replace(/\n$/, "");
      return <MermaidBlock code={source} fallback={<CodeBlock code={source} lang={lang} />} />;
    }
    if (block) return <CodeBlock code={text.replace(/\n$/, "")} lang={lang} />;
    const ref = parseInlineRef(text);
    if (ref)
      return (
        <button
          type="button"
          title={`Open ${ref.path}${ref.line ? ` at line ${ref.line}` : ""}`}
          onClick={() => openRef(ref)}
          className="group inline rounded-full text-accent"
        >
          <code className="rounded-full bg-accent/10 px-1.5 py-px font-mono text-[0.9em] transition-colors duration-150 group-hover:bg-accent/20">
            {children}
          </code>
        </button>
      );
    return (
      <code className="rounded-xs bg-fill px-1 py-px font-mono text-[0.9em] [box-decoration-break:clone]">
        {children}
      </code>
    );
  },
};

const plugins = [remarkGfm];

/** react-markdown's safe URLs, plus `file://` and `data:image/` for pictures (MarkdownMedia.tsx checks them). */
export function urlTransform(url: string, key: string): string {
  if (key === "src" && /^(file:\/\/|data:image\/)/i.test(url)) return url;
  return defaultUrlTransform(url);
}

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
  const element = ReactMarkdown({ children: text, remarkPlugins: plugins, components, urlTransform });
  rendered.set(text, element);
  if (rendered.size > RENDERED_MAX) rendered.delete(rendered.keys().next().value as string);
  return element;
}

const Block = memo(function Block({ text }: { text: string }) {
  return renderBlock(text);
});

/** The block still being written, with the words that just arrived set to fade in. */
function renderRevealed(text: string, revealed: number): { element: ReactElement; chars: number } {
  let chars = revealed;
  const element = ReactMarkdown({
    children: text,
    remarkPlugins: plugins,
    rehypePlugins: [
      [
        rehypeReveal,
        {
          revealed,
          onRendered: (n: number) => {
            chars = n;
          },
        },
      ],
    ],
    components,
    urlTransform,
  });
  return { element, chars };
}

/**
 * The last block of a reply while it streams: only the words the chunk added
 * fade in, and the words already on screen are plain text. The block is not
 * cached (it changes with every chunk) and stops being rendered this way as
 * soon as the reply ends.
 */
const StreamingTail = memo(function StreamingTail({ text }: { text: string }) {
  const host = useRef<HTMLDivElement>(null);
  // The block and its rendered length as the last frame left them.
  const shown = useRef({ text: "", chars: 0 });
  const fresh = useRef(true);
  const revealed = revealedBefore(shown.current.text, shown.current.chars, text);
  const { element, chars } = renderRevealed(text, revealed);
  useLayoutEffect(() => {
    shown.current = { text, chars };
    const spans = host.current?.querySelectorAll<HTMLElement>(".rc-reveal");
    if (fresh.current) {
      fresh.current = false;
      // A block with no words to reveal (code, a table, a picture) fades in whole, once.
      const block = host.current?.firstElementChild;
      if (!spans?.length && block instanceof HTMLElement) block.style.animation = "rc-fade 180ms ease-out";
    }
    // React reuses the span the previous word had when it sits at the same
    // place, and that span's animation already ran. Rewind this chunk's spans
    // from the top (delay included) before the frame paints.
    for (const span of spans ?? [])
      for (const animation of span.getAnimations()) {
        animation.cancel();
        animation.play();
      }
  }, [text, chars]);
  return (
    // Out of the layout (display: contents): the block stays a child of the reply.
    <div ref={host} className="contents">
      {element}
    </div>
  );
});

export const Markdown = memo(function Markdown({
  text,
  className,
  streaming,
  reveal = false,
}: {
  text: string;
  className?: string;
  /** Growing: parse it block by block, so a chunk re-parses only the last block. */
  streaming?: boolean;
  /** Optional: fade in the words that arrive while it streams (a reply, not a rolling preview). */
  reveal?: boolean;
}) {
  // A finished text is one block (one parse, cached by its text); a streaming one is split so the
  // blocks above the one being written keep their elements. Splitting costs a parser per block.
  const blocks = useMemo(() => (streaming ? markdownBlocks(text) : [text]), [text, streaming]);
  const tail = blocks.length - 1;
  return (
    <div className={cx("selectable flex flex-col gap-2.5 text-body break-words", className)}>
      {blocks.map((block, i) =>
        reveal && streaming && i === tail ? (
          <StreamingTail key={i} text={block} />
        ) : (
          <Block key={i} text={block} />
        ),
      )}
    </div>
  );
});
