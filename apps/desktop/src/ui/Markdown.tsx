// Reply Markdown, styled after MarkdownText.swift: 13px body, bold headings,
// mono inline code without a box, code blocks on a quiet fill with a Copy
// button and Xcode colours, tables and quotes on rounded fills.

import { Check, Copy } from "lucide-react";
import { type ComponentProps, memo, type ReactNode, useEffect, useRef, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { highlight } from "../lib/highlight";
import { openExternal } from "../lib/native";
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
        "inline-flex h-[22px] items-center gap-1 rounded-[7px] text-caption text-fg-2 transition-colors hover:bg-fill-2 hover:text-fg",
        iconOnly ? "w-[22px] justify-center" : "px-2",
      )}
    >
      {copied ? <Check size={12} className="text-success" /> : <Copy size={12} />}
      {!iconOnly && <span>{copied ? "Copied" : label}</span>}
    </button>
  );
}

export function CodeBlock({ code, lang }: { code: string; lang?: string }) {
  const [html, setHtml] = useState<string | null>(null);
  const highlighted = useRef(false);
  useEffect(() => {
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
    <div className="my-0.5 overflow-hidden rounded-xl bg-fill-3">
      <div className="flex items-center justify-between pt-1.5 pr-1.5 pl-3">
        <span className="font-mono text-caption text-fg-2">{lang || "code"}</span>
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
  ul: ({ children }) => <ul className="ml-[1.1em] list-disc space-y-0.5 marker:text-fg-2">{children}</ul>,
  ol: ({ children }) => <ol className="ml-[1.4em] list-decimal space-y-0.5 marker:text-fg-2">{children}</ol>,
  li: ({ children }) => <li className="pl-0.5 leading-[20px] [&>ol]:mt-0.5 [&>ul]:mt-0.5">{children}</li>,
  a: ({ href, children }) => (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        if (href) openExternal(href);
      }}
      className="cursor-pointer text-brand underline decoration-brand/45 underline-offset-2"
    >
      {href ? shortUrl(href, children) : children}
    </a>
  ),
  blockquote: ({ children }) => (
    <blockquote className="relative rounded-lg bg-fill-4 py-2 pr-3 pl-[21px] text-fg/80 before:absolute before:top-2 before:bottom-2 before:left-2 before:w-[3px] before:rounded-full before:bg-fg-2/45">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-1 border-hairline" />,
  table: ({ children }) => (
    <div className="overflow-x-auto rounded-xl bg-fill-3 py-1.5">
      <table className="w-full border-collapse text-left">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border-b border-hairline px-2.5 py-1.5 font-semibold">{children}</th>,
  td: ({ children }) => <td className="px-2.5 py-1.5 align-top">{children}</td>,
  pre: ({ children }) => <>{children}</>,
  code: ({ className, children, node }: ComponentProps<"code"> & { node?: { position?: unknown } }) => {
    const text = String(children ?? "");
    const lang = /language-([\w+#.-]+)/.exec(className ?? "")?.[1];
    // A fenced block has a language class or spans lines; inline code has neither.
    const block = lang !== undefined || text.includes("\n");
    void node;
    if (block) return <CodeBlock code={text.replace(/\n$/, "")} lang={lang} />;
    return <code className="font-mono text-[0.95em]">{children}</code>;
  },
};

const plugins = [remarkGfm];

export const Markdown = memo(function Markdown({ text, className }: { text: string; className?: string }) {
  return (
    <div className={cx("selectable flex flex-col gap-2.5 text-body break-words", className)}>
      <ReactMarkdown remarkPlugins={plugins} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
