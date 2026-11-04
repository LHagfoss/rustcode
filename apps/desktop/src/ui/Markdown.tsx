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
import { remarkMathAsCode, remarkMoneySafeMath } from "../lib/markdownMath";
import { openExternal, openInEditor } from "../lib/native";
import { advance, newRevealState, type RevealState, rehypeReveal } from "../lib/reveal";
import { hasShell } from "../lib/shell";
import { toast, useApp } from "../state/store";
import { LinkChip } from "./LinkChip";
import { MarkdownImage, MermaidBlock } from "./MarkdownMedia";
import { copyText, cx, Pill } from "./primitives";

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
  // The label it says, on the button and for a screen reader: "Copied" for a moment.
  const said = copied ? "Copied" : label;
  const copy = () => void copyText(text).then(() => setCopied(true));
  const icon = copied ? <Check size={12} className="text-success" /> : <Copy size={12} />;
  // The icon-only square in a row of 22px actions stays as it is; the labelled one is a pill.
  if (iconOnly)
    return (
      <button
        type="button"
        aria-label={said}
        onClick={copy}
        className="inline-flex h-[22px] w-[22px] items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
      >
        {icon}
      </button>
    );
  return (
    <Pill quiet size="sm" aria-label={said} onClick={copy}>
      {icon}
      {said}
    </Pill>
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

/** Starts the inline code that stands for a sent message's leading command: drawn as a chip. */
export const COMMAND_MARK = "\u2063";

// ---------------------------------------------------------------- math

let katex: Promise<typeof import("katex")> | undefined;

/**
 * KaTeX and its stylesheet, the first time a reply holds math: neither is in the app's
 * startup bundle, and a thread without formulas never downloads them.
 */
function loadKatex(): Promise<typeof import("katex")> {
  katex ??= (async () => {
    const [mod] = await Promise.all([import("katex"), import("katex/dist/katex.min.css")]);
    return mod;
  })();
  return katex;
}

/** A formula: TeX until KaTeX arrives (or forever, if it cannot parse it). */
const Formula = memo(function Formula({ tex, display }: { tex: string; display: boolean }) {
  const [html, setHtml] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void loadKatex().then(
      (k) => {
        if (!live) return;
        try {
          setHtml(
            k.default.renderToString(tex, { displayMode: display, throwOnError: false, strict: false }),
          );
        } catch {
          setHtml(null);
        }
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [tex, display]);
  if (html)
    return (
      <span
        // KaTeX escapes what it writes; the HTML holds only its own spans and MathML.
        // biome-ignore lint/security/noDangerouslySetInnerHtml: rendered by KaTeX from escaped input
        dangerouslySetInnerHTML={{ __html: html }}
        className={display ? "block overflow-x-auto py-0.5" : "inline-block align-middle"}
      />
    );
  return (
    <code
      className={cx(
        "rounded-sm bg-fill px-[0.35em] py-[0.1em] font-mono text-[0.85em] [box-decoration-break:clone]",
        display && "block overflow-x-auto whitespace-pre",
      )}
    >
      {tex}
    </code>
  );
});

const components: Components = {
  h1: ({ children }) => (
    <h1 className="mt-2.5 font-semibold text-[20px] text-fg leading-[1.3]">{children}</h1>
  ),
  h2: ({ children }) => (
    <h2 className="mt-2.5 font-semibold text-title2 text-fg leading-[1.3]">{children}</h2>
  ),
  h3: ({ children }) => (
    <h3 className="mt-1.5 font-semibold text-title3 text-fg leading-[1.3]">{children}</h3>
  ),
  h4: ({ children }) => <h4 className="mt-1 font-semibold text-body text-fg leading-[1.3]">{children}</h4>,
  p: ({ children }) => <p className="leading-[1.625]">{children}</p>,
  ul: ({ children }) => <ul className="ml-[1.1em] list-disc space-y-0.5 marker:text-fg-muted">{children}</ul>,
  ol: ({ children }) => (
    <ol className="ml-[1.4em] list-decimal space-y-0.5 marker:text-fg-muted">{children}</ol>
  ),
  li: ({ children }) => <li className="pl-0.5 leading-[1.625] [&>ol]:mt-0.5 [&>ul]:mt-0.5">{children}</li>,
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
        className="-mx-1 cursor-pointer rounded-full px-1 text-accent underline decoration-accent/30 underline-offset-2 hover:bg-accent/12 hover:decoration-transparent"
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
    // remark-math's formulas, as `remarkMathAsCode` hands them over.
    if (lang === "math" || lang === "inline-math")
      return <Formula tex={text.replace(/\n$/, "")} display={lang === "math"} />;
    if (block && lang === "mermaid") {
      const source = text.replace(/\n$/, "");
      return <MermaidBlock code={source} fallback={<CodeBlock code={source} lang={lang} />} />;
    }
    if (block) return <CodeBlock code={text.replace(/\n$/, "")} lang={lang} />;
    if (text.startsWith(COMMAND_MARK))
      return (
        <span className="rounded-md bg-accent/12 px-1.5 py-0.5 font-medium font-mono text-[0.85em] text-accent">
          {text.slice(COMMAND_MARK.length)}
        </span>
      );
    const ref = parseInlineRef(text);
    if (ref)
      return (
        <button
          type="button"
          title={`Open ${ref.path}${ref.line ? ` at line ${ref.line}` : ""}`}
          onClick={() => openRef(ref)}
          className="group inline rounded-full text-accent"
        >
          <code className="rounded-full bg-accent/10 px-1.5 py-px font-mono text-[0.9em] group-hover:bg-accent/20">
            {children}
          </code>
        </button>
      );
    return (
      <code className="rounded-sm bg-fill px-[0.35em] py-[0.1em] font-mono text-[0.85em] [box-decoration-break:clone]">
        {children}
      </code>
    );
  },
};

const plugins = [remarkGfm, remarkMoneySafeMath, remarkMathAsCode];

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

/**
 * Where the reveal keeps each streaming block's word birth times, by reply and
 * block, outside the component: a row the virtualizer drops and brings back,
 * or a thread switched away and back, picks up where it was instead of fading
 * its visible text in again. Entries go when the reply stops streaming.
 */
const revealStates = new Map<string, RevealState>();

function forgetReveal(key: string) {
  for (const k of revealStates.keys()) if (k.startsWith(`${key}\u0000`)) revealStates.delete(k);
}

/** The block being written, with the words still fading wrapped in spans (lib/reveal.ts). */
function renderLive(text: string, stateKey: string): ReactElement {
  const state = advance(revealStates.get(stateKey) ?? newRevealState(), text);
  revealStates.set(stateKey, state);
  return ReactMarkdown({
    children: text,
    remarkPlugins: plugins,
    rehypePlugins: [[rehypeReveal, { state, now: performance.now() }]],
    components,
    urlTransform,
  });
}

/**
 * One block of a reply. Every block, the one being written included, is this
 * component, so when the tail finishes (or the reply ends) React keeps its
 * elements: a code block keeps its highlighting, a diagram is not drawn again,
 * and a selection inside it survives.
 */
const Block = memo(function Block({ text, live }: { text: string; live?: string }) {
  return live ? renderLive(text, live) : renderBlock(text);
});

export const Markdown = memo(function Markdown({
  text,
  className,
  streaming,
  reveal = false,
  revealKey,
}: {
  text: string;
  className?: string;
  /** Growing: parse it block by block, so a chunk re-parses only the last block. */
  streaming?: boolean;
  /** Optional: fade in the words that arrive while it streams (a reply, not a rolling preview). */
  reveal?: boolean;
  /** Which reply this is, so the reveal survives the row remounting; one per component without it. */
  revealKey?: string;
}) {
  // A finished text is one block (one parse, cached by its text); a streaming one is split so the
  // blocks above the one being written keep their elements. Splitting costs a parser per block.
  const blocks = useMemo(() => (streaming ? markdownBlocks(text) : [text]), [text, streaming]);
  const tail = blocks.length - 1;
  const root = useRef<HTMLDivElement>(null);
  const ownKey = useRef<string>(`reveal-${Math.random().toString(36).slice(2)}`);
  const key = revealKey ?? ownKey.current;
  const live = reveal && streaming;
  const tailText = blocks[tail];
  useLayoutEffect(() => {
    if (!live || tailText === undefined) return;
    // Pin each fading word's animation to its birth (lib/reveal.ts): a span React reused for
    // another word, or one that just mounted, then shows that word's point in its fade. Cheaper
    // than restarting: an animation already at its birth is left alone.
    for (const el of root.current?.querySelectorAll<HTMLElement>("[data-birth]") ?? []) {
      const birth = Number(el.dataset.birth);
      for (const animation of el.getAnimations?.() ?? [])
        if (animation.startTime !== birth) animation.startTime = birth;
    }
    // Only when the tail changed: that is when Block drew new spans. An unrelated re-render keeps
    // the running animations as they are.
  }, [tailText, live]);
  useEffect(() => {
    if (!live) forgetReveal(key);
  }, [live, key]);
  return (
    <div
      ref={root}
      // Replies read at 14px on 1.625 (22.75px), the transcript's own leading; the app's rows keep
      // the body's 19px (docs/design.md › Type).
      className={cx(
        "selectable flex flex-col gap-2.5 text-body leading-relaxed text-fg/90 break-words",
        className,
      )}
    >
      {blocks.map((block, i) => (
        <Block key={i} text={block} live={live && i === tail ? `${key}\u0000${i}` : undefined} />
      ))}
    </div>
  );
});
