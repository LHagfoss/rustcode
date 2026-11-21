import { Dialog } from "radix-ui";
import { useEffect, useRef, useState } from "react";
import { isTauri } from "../acp/transport";
import { redact, type SystemInfo } from "../lib/diagnostics";
import { clipboardImage, openExternal, systemInfo, threadLogTail } from "../lib/native";
import { pastedImages } from "../lib/paste";
import { activeTransport, shellCall } from "../lib/shell";
import { appVersion } from "../lib/updater";
import { Bug, ChevronDown, ExternalLink, Image, MessageSquarePlus, X } from "./icons";
import { Pill } from "./primitives";

const repository = "https://github.com/lhagfoss/rustcode";
// A field is a fill a step over the dialog, never an outline (docs/design.md › Borders).
const field =
  "w-full rounded-lg bg-fill px-3 py-2 text-body text-fg outline-none placeholder:text-fg-faint hover:bg-fill-strong focus:bg-fill-strong";
const reportTypes = [
  { kind: "bug", name: "Bug", label: "bug", Icon: Bug },
  { kind: "feature", name: "Feature", label: "enhancement", Icon: MessageSquarePlus },
] as const;

type ReportType = (typeof reportTypes)[number];
type Options = { labels: string[]; can_label: boolean };
type Screenshot = { name: string; url: string; file: File };

const OS_NAMES: Record<string, string> = { macos: "macOS", windows: "Windows", linux: "Linux" };

/** A bug report's `### Where`: the app, the OS and its architecture, and the thread's harness. */
export const LOG_TAIL_LINES = 200;

/** A thread log for a public issue: redacted and folded, in a fence no log line closes. */
export function logSection(log: string, home?: string | null): string {
  const text = redact(log.trim(), home).replaceAll("~~~~", "~~~");
  return `<details><summary>Thread log (last ${LOG_TAIL_LINES} lines)</summary>\n\n~~~~text\n${text}\n~~~~\n\n</details>`;
}

export function whereLines(version: string | null, system: SystemInfo | null, harness?: string): string {
  const os = system && [OS_NAMES[system.os] ?? system.os, system.osVersion].filter(Boolean).join(" ");
  return [
    `- RustCode Desktop${version ? ` ${version}` : ""}`,
    system && `- OS: ${os} (${system.arch})`,
    harness && `- Harness: ${harness}`,
  ]
    .filter(Boolean)
    .join("\n");
}

/** A file's bytes as base64, as the shell's save commands take them. */
function base64Of(file: File): Promise<string> {
  const reader = new FileReader();
  return new Promise<string>((resolve, reject) => {
    reader.onload = () => resolve(String(reader.result).split(",")[1] ?? "");
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

export function IssueReportDialog({
  open,
  onOpenChange,
  harness,
  threadId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The harness of the thread the report was opened from, for a bug's `### Where`. */
  harness?: string;
  /** The thread the report was opened from, whose log a bug can attach. */
  threadId?: string;
}) {
  const [type, setType] = useState<ReportType>(reportTypes[0]);
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [details, setDetails] = useState("");
  const [version, setVersion] = useState<string | null>(null);
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [options, setOptions] = useState<Options | null>(null);
  const [labels, setLabels] = useState<string[]>([]);
  const [setupError, setSetupError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [issue, setIssue] = useState("");
  const [screenshots, setScreenshots] = useState<Screenshot[]>([]);
  const [attachLog, setAttachLog] = useState(false);
  const [readLog, setReadLog] = useState<{ id: string; text: string } | null>(null);
  const log = readLog && readLog.id === threadId ? readLog.text : null;
  const [imageError, setImageError] = useState("");
  const [savingAll, setSavingAll] = useState(false);
  const titleInput = useRef<HTMLInputElement>(null);
  const uploads = useRef<HTMLInputElement>(null);
  const imageUrls = useRef(new Set<string>());
  const mock = import.meta.env.DEV && activeTransport()?.name === "mock";

  useEffect(() => {
    const urls = imageUrls.current;
    return () => {
      for (const url of urls) URL.revokeObjectURL(url);
    };
  }, []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: Retry explicitly rechecks GitHub CLI setup.
  useEffect(() => {
    if (!open || issue) return;
    let current = true;
    setSetupError("");
    setOptions(null);
    const load = mock
      ? Promise.resolve({ labels: ["bug", "desktop", "enhancement", "needs-repro", "ui"], can_label: true })
      : isTauri()
        ? shellCall<Options>("issue_report_options")
        : Promise.reject("Open RustCode Desktop to submit with GitHub CLI.");
    void load.then(
      (value) => {
        if (current) setOptions(value);
      },
      (reason) => {
        if (current) setSetupError(String(reason));
      },
    );
    void appVersion().then((value) => {
      if (current) setVersion(value);
    });
    void systemInfo().then((value) => {
      if (current) setSystem(value);
    });
    return () => {
      current = false;
    };
  }, [open, issue, mock, attempt]);

  const addImages = (files: File[]) => {
    const allowed = files.filter((file) => file.type.startsWith("image/") && file.size <= 10 * 1024 * 1024);
    const additions = allowed.slice(0, 5 - imageUrls.current.size).map((file) => {
      const url = URL.createObjectURL(file);
      imageUrls.current.add(url);
      return { name: file.name || "Screenshot.png", url, file };
    });
    setImageError(additions.length < files.length ? "Choose up to five images, each 10 MB or smaller." : "");
    setScreenshots((current) => [...current, ...additions]);
  };

  const removeImage = (url: string) => {
    URL.revokeObjectURL(url);
    imageUrls.current.delete(url);
    setScreenshots(screenshots.filter((image) => image.url !== url));
    setImageError("");
  };

  const saveImage = async ({ name, file }: Screenshot) => {
    try {
      await shellCall("issue_report_save_image", { name, data: await base64Of(file) });
    } catch (reason) {
      setImageError(String(reason));
    }
  };

  // Every screenshot in one press: the app asks for one folder; a browser downloads each.
  const saveAll = async () => {
    if (savingAll || !screenshots.length) return;
    setImageError("");
    if (!isTauri()) {
      for (const { url, name } of screenshots) {
        const link = document.createElement("a");
        link.href = url;
        link.download = name;
        link.click();
      }
      return;
    }
    setSavingAll(true);
    try {
      const images = await Promise.all(
        screenshots.map(async ({ name, file }) => ({ name, data: await base64Of(file) })),
      );
      await shellCall<number>("issue_report_save_images", { images });
    } catch (reason) {
      setImageError(String(reason));
    } finally {
      setSavingAll(false);
    }
  };

  const close = (next: boolean) => {
    if (submitting) return;
    if (!next && issue) {
      setIssue("");
      setType(reportTypes[0]);
      setTitle("");
      setDescription("");
      setDetails("");
      setLabels([]);
      setError("");
      setImageError("");
      for (const url of imageUrls.current) URL.revokeObjectURL(url);
      imageUrls.current.clear();
      setScreenshots([]);
    }
    onOpenChange(next);
  };

  useEffect(() => {
    if (open) setAttachLog(false);
  }, [open]);

  useEffect(() => {
    if (!attachLog || !threadId || log !== null) return;
    const id = threadId;
    let current = true;
    void threadLogTail(id, LOG_TAIL_LINES).then(
      (text) => {
        if (current) setReadLog({ id, text });
      },
      () => {
        if (current) setReadLog({ id, text: "" });
      },
    );
    return () => {
      current = false;
    };
  }, [attachLog, threadId, log]);

  const bug = type.kind === "bug";
  const selectedLabels = options?.can_label
    ? [...new Set([type.label, ...labels])].filter((label) => options.labels.includes(label))
    : [];
  const body = [
    !bug && "**Feature request**",
    `### ${bug ? "What happened" : "What are you trying to do?"}\n\n${description.trim()}`,
    details.trim() && `### ${bug ? "Steps to reproduce" : "Idea"}\n\n${details.trim()}`,
    bug && `### Where\n\n${whereLines(version, system, harness)}`,
    bug && attachLog && log && logSection(log, system?.home),
  ]
    .filter(Boolean)
    .join("\n\n");

  const submit = async () => {
    if (!options || !title.trim() || !description.trim() || submitting) return;
    setSubmitting(true);
    setError("");
    try {
      const report = { title: title.trim(), body, labels: selectedLabels };
      setIssue(mock ? `${repository}/issues` : await shellCall<string>("issue_report_submit", { report }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={close}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-5">
          <Dialog.Content
            className="elevated pointer-events-auto flex max-h-full w-full max-w-[520px] rounded-xl flex-col overflow-hidden"
            aria-describedby={undefined}
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              titleInput.current?.focus();
            }}
            onPointerDownOutside={(event) => event.preventDefault()}
            onPaste={(event) => {
              const files = pastedImages(event.clipboardData);
              if (issue || submitting) return;
              if (files.length) {
                event.preventDefault();
                addImages(files);
              } else if (isTauri() && !event.clipboardData.types.includes("text/plain")) {
                void clipboardImage()
                  .then((image) => {
                    if (!image || !titleInput.current) return;
                    const bytes = Uint8Array.from(atob(image.data), (character) => character.charCodeAt(0));
                    addImages([new File([bytes], image.name, { type: image.mimeType })]);
                  })
                  .catch((reason) => setImageError(String(reason)));
              }
            }}
          >
            <header className="flex shrink-0 items-center justify-between gap-3 px-5 pt-4">
              <Dialog.Title className="font-semibold text-title3">
                {issue ? "Issue submitted" : "Report an issue"}
              </Dialog.Title>
              <Pill quiet disabled={submitting} aria-label="Close report" onClick={() => close(false)}>
                <X size={16} />
              </Pill>
            </header>
            <form
              className="flex min-h-0 flex-col"
              onSubmit={(event) => {
                event.preventDefault();
                void submit();
              }}
            >
              <div className="flex min-h-0 flex-col gap-4 overflow-y-auto px-5 pb-4 pt-3">
                {!issue && (
                  <>
                    <div
                      role="group"
                      aria-label="Report type"
                      className="flex w-fit gap-1 rounded-lg bg-fill p-1"
                    >
                      {reportTypes.map((item) => (
                        <Pill
                          key={item.kind}
                          quiet
                          aria-pressed={type === item}
                          className="justify-center px-3 text-fg-muted aria-pressed:bg-knob aria-pressed:text-knob-fg aria-pressed:shadow-knob"
                          onClick={() => setType(item)}
                        >
                          <item.Icon size={16} />
                          {item.name}
                        </Pill>
                      ))}
                    </div>
                    <label className="grid gap-1.5 text-callout font-medium">
                      Title
                      <input
                        ref={titleInput}
                        className={field}
                        required
                        maxLength={256}
                        value={title}
                        onChange={(event) => setTitle(event.target.value)}
                        placeholder={bug ? "Summarize the problem" : "Summarize the idea"}
                      />
                    </label>
                    <label className="grid gap-1.5 text-callout font-medium">
                      {bug ? "What happened?" : "What are you trying to do?"}
                      <textarea
                        className={`${field} resize-y`}
                        rows={3}
                        required
                        maxLength={7000}
                        value={description}
                        onChange={(event) => setDescription(event.target.value)}
                        placeholder={
                          bug ? "What did you expect to happen?" : "Describe the problem or use case."
                        }
                      />
                    </label>
                    <label className="grid gap-1.5 text-callout font-medium">
                      <span>
                        {bug ? "Steps to reproduce" : "Idea"}{" "}
                        <span className="font-normal text-fg-faint">(optional)</span>
                      </span>
                      <textarea
                        className={`${field} resize-y`}
                        rows={2}
                        maxLength={7000}
                        value={details}
                        onChange={(event) => setDetails(event.target.value)}
                        placeholder={bug ? "How can we reproduce it?" : "Describe how you’d like it to work."}
                      />
                    </label>
                    {bug && threadId && (
                      <div className="grid gap-1.5">
                        <label className="flex items-center gap-1.5 text-callout">
                          <input
                            type="checkbox"
                            checked={attachLog}
                            onChange={(event) => setAttachLog(event.target.checked)}
                          />
                          Attach this thread’s log
                          <span className="text-fg-faint">(last {LOG_TAIL_LINES} lines, redacted)</span>
                        </label>
                        {attachLog && (
                          <pre className="max-h-32 overflow-auto rounded-lg bg-fill px-3 py-2 text-caption text-fg-muted">
                            {log === null
                              ? "Reading the log…"
                              : log
                                ? redact(log.trim(), system?.home)
                                : "This thread has no log yet."}
                          </pre>
                        )}
                      </div>
                    )}
                    {options?.can_label && (
                      <details className="group">
                        <summary className="flex cursor-pointer list-none items-center gap-2 text-callout font-medium [&::-webkit-details-marker]:hidden">
                          <ChevronDown size={14} className="-rotate-90 text-fg-muted group-open:rotate-0" />
                          Labels
                          {selectedLabels.length > 0 && (
                            <span className="min-w-0 truncate font-normal text-fg-muted">
                              {selectedLabels.join(", ")}
                            </span>
                          )}
                        </summary>
                        <div className="mt-2 flex max-h-28 flex-wrap gap-2 overflow-y-auto">
                          {options.labels
                            .filter((label) => label !== type.label)
                            .map((label) => (
                              <label key={label} className="flex items-center gap-1.5 text-caption">
                                <input
                                  type="checkbox"
                                  checked={labels.includes(label)}
                                  onChange={(event) =>
                                    setLabels(
                                      event.target.checked
                                        ? [...labels, label]
                                        : labels.filter((value) => value !== label),
                                    )
                                  }
                                />
                                {label}
                              </label>
                            ))}
                        </div>
                      </details>
                    )}
                  </>
                )}
                {(!issue || screenshots.length > 0) && (
                  <section aria-label="Screenshots" className="rounded-lg bg-fill-subtle px-3 py-2">
                    <div className="flex items-center gap-2">
                      <Image size={18} className="shrink-0 text-fg-muted" />
                      <div className="flex-1">
                        <p className="text-callout font-medium">Screenshots</p>
                        <p className="text-caption text-fg-muted">
                          {issue
                            ? "Save them, then drag them into the issue on GitHub."
                            : "GitHub cannot take them from the app: after filing, save them and attach them to the issue."}
                        </p>
                      </div>
                      {issue ? (
                        <Pill disabled={savingAll} onClick={() => void saveAll()}>
                          {savingAll ? "Saving…" : "Save all"}
                        </Pill>
                      ) : (
                        <Pill onClick={() => uploads.current?.click()}>Add images</Pill>
                      )}
                      <input
                        ref={uploads}
                        type="file"
                        accept="image/*"
                        multiple
                        hidden
                        onChange={(event) => {
                          addImages(Array.from(event.target.files ?? []));
                          event.target.value = "";
                        }}
                      />
                    </div>
                    {screenshots.length > 0 && (
                      <ul className="mt-3 flex flex-wrap gap-2">
                        {screenshots.map(({ url, name, file }) => (
                          <li key={url} className="relative w-24">
                            <a
                              href={url}
                              download={name}
                              aria-label={`Save ${name}`}
                              onClick={(event) => {
                                if (!isTauri()) return;
                                event.preventDefault();
                                void saveImage({ name, url, file });
                              }}
                            >
                              <img
                                src={url}
                                alt={name}
                                className="h-16 w-24 rounded-md bg-fill object-contain"
                              />
                              {issue && (
                                <span className="block truncate pt-1 text-caption text-accent">
                                  Save image
                                </span>
                              )}
                            </a>
                            {!issue && (
                              <button
                                type="button"
                                aria-label={`Remove ${name}`}
                                onClick={() => removeImage(url)}
                                className="absolute right-1 top-1 rounded-md bg-surface-2 p-1"
                              >
                                <X size={12} />
                              </button>
                            )}
                          </li>
                        ))}
                      </ul>
                    )}
                    {imageError && (
                      <p role="alert" className="mt-2 text-caption text-danger">
                        {imageError}
                      </p>
                    )}
                  </section>
                )}
                {(setupError || error) && (
                  <p role="alert" className="text-callout text-danger">
                    {error || setupError}
                  </p>
                )}
              </div>
              <footer className="flex flex-wrap items-center justify-between gap-2 border-t border-border px-5 py-3">
                {setupError && !issue ? (
                  <Pill quiet onClick={() => openExternal(`${repository}/issues/new/choose`)}>
                    GitHub <ExternalLink size={12} />
                  </Pill>
                ) : !issue ? (
                  <span className="text-caption text-fg-muted">Public on GitHub</span>
                ) : null}
                <div className="ml-auto flex items-center gap-2">
                  {issue ? (
                    <>
                      <Pill quiet onClick={() => close(false)}>
                        Done
                      </Pill>
                      <Pill prominent onClick={() => openExternal(issue)}>
                        {screenshots.length
                          ? `Open issue to attach ${screenshots.length} screenshot${screenshots.length === 1 ? "" : "s"}`
                          : "View issue"}{" "}
                        <ExternalLink size={14} />
                      </Pill>
                    </>
                  ) : (
                    <>
                      <Pill quiet disabled={submitting} onClick={() => close(false)}>
                        Cancel
                      </Pill>
                      {setupError && isTauri() ? (
                        <Pill prominent onClick={() => setAttempt(attempt + 1)}>
                          Retry
                        </Pill>
                      ) : (
                        <Pill
                          type="submit"
                          prominent
                          disabled={!options || !title.trim() || !description.trim() || submitting}
                        >
                          {submitting ? "Submitting…" : options || setupError ? "Submit" : "Checking GitHub…"}
                        </Pill>
                      )}
                    </>
                  )}
                </div>
              </footer>
            </form>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
