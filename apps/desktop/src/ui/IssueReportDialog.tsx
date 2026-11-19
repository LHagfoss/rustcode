import { Dialog } from "radix-ui";
import { useEffect, useRef, useState } from "react";
import { isTauri } from "../acp/transport";
import { clipboardImage, openExternal } from "../lib/native";
import { pastedImages } from "../lib/paste";
import { activeTransport, shellCall } from "../lib/shell";
import { appVersion } from "../lib/updater";
import { Bug, ExternalLink, Image, MessageSquarePlus, X } from "./icons";
import { Pill } from "./primitives";

const repository = "https://github.com/lhagfoss/rustcode";
const field =
  "w-full rounded-lg border border-border bg-bg/40 px-3 py-2 text-body text-fg placeholder:text-fg-faint focus-visible:outline-1 focus-visible:outline-accent";
const reportTypes = [
  { kind: "bug", name: "Bug report", label: "bug", Icon: Bug },
  { kind: "feature", name: "Feature request", label: "enhancement", Icon: MessageSquarePlus },
] as const;

type ReportType = (typeof reportTypes)[number];
type Options = { labels: string[]; can_label: boolean };
type Screenshot = { name: string; url: string; file: File };

export function IssueReportDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [type, setType] = useState<ReportType>(reportTypes[0]);
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [details, setDetails] = useState("");
  const [version, setVersion] = useState<string | null>(null);
  const [options, setOptions] = useState<Options | null>(null);
  const [labels, setLabels] = useState<string[]>([]);
  const [setupError, setSetupError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [issue, setIssue] = useState("");
  const [screenshots, setScreenshots] = useState<Screenshot[]>([]);
  const [imageError, setImageError] = useState("");
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
      const reader = new FileReader();
      const encoded = await new Promise<string>((resolve, reject) => {
        reader.onload = () => resolve(String(reader.result).split(",")[1] ?? "");
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(file);
      });
      await shellCall("issue_report_save_image", { name, data: encoded });
    } catch (reason) {
      setImageError(String(reason));
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

  const bug = type.kind === "bug";
  const selectedLabels = options?.can_label
    ? [...new Set([type.label, ...labels])].filter((label) => options.labels.includes(label))
    : [];
  const body = [
    !bug && "**Feature request**",
    `### ${bug ? "What happened" : "What are you trying to do?"}\n\n${description.trim()}`,
    details.trim() && `### ${bug ? "Steps to reproduce" : "Idea"}\n\n${details.trim()}`,
    bug && `### Where\n\nRustCode Desktop${version ? ` ${version}` : ""}`,
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
            className="elevated pointer-events-auto flex max-h-full w-full max-w-[560px] flex-col overflow-hidden"
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
            <header className="flex shrink-0 items-start gap-3 border-b border-border px-5 py-4">
              <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-accent/15 text-accent">
                <type.Icon size={24} />
              </span>
              <div className="min-w-0 flex-1">
                <Dialog.Title className="font-semibold text-[17px]">
                  {issue ? "Issue submitted" : "Report an issue"}
                </Dialog.Title>
                <Dialog.Description className="mt-0.5 text-callout text-fg-muted">
                  {issue
                    ? "Your report is on GitHub."
                    : "Send public feedback to lhagfoss/rustcode on GitHub."}
                </Dialog.Description>
              </div>
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
              <div className="flex min-h-0 flex-col gap-4 overflow-y-auto px-5 py-4">
                {!issue && (
                  <>
                    <div role="group" aria-label="Report type" className="grid grid-cols-2 gap-2">
                      {reportTypes.map((item) => (
                        <Pill
                          key={item.kind}
                          size="lg"
                          quiet
                          aria-pressed={type === item}
                          className="justify-center border border-border aria-pressed:border-accent/40 aria-pressed:bg-accent/15 aria-pressed:text-accent"
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
                        className={field}
                        rows={3}
                        required
                        maxLength={7000}
                        value={description}
                        onChange={(event) => setDescription(event.target.value)}
                        placeholder={
                          bug
                            ? "Describe what you expected and what happened instead."
                            : "Describe the problem or use case."
                        }
                      />
                    </label>
                    <label className="grid gap-1.5 text-callout font-medium">
                      <span>
                        {bug ? "Steps to reproduce" : "Idea"}{" "}
                        <span className="font-normal text-fg-faint">· Optional</span>
                      </span>
                      <textarea
                        className={field}
                        rows={2}
                        maxLength={7000}
                        value={details}
                        onChange={(event) => setDetails(event.target.value)}
                        placeholder={
                          bug
                            ? "List the steps someone can follow to see the problem."
                            : "Describe how you’d like it to work."
                        }
                      />
                    </label>
                    {options?.can_label && (
                      <details className="rounded-lg bg-fill px-3 py-2">
                        <summary className="cursor-pointer text-callout font-medium">
                          Labels
                          {selectedLabels.length > 0 && (
                            <span className="ml-2 font-normal text-fg-muted">
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
                  <section
                    aria-label="Screenshots"
                    className="rounded-xl border border-dashed border-border-strong px-3 py-3"
                  >
                    <div className="flex items-center gap-3">
                      <Image size={18} className="shrink-0 text-fg-muted" />
                      <div className="flex-1">
                        <p className="text-callout font-medium">Screenshots</p>
                        <p className="text-caption text-fg-muted">
                          {issue
                            ? "Save the images, then attach them in GitHub."
                            : "Paste or choose images to attach later on GitHub."}
                        </p>
                      </div>
                      {!issue && <Pill onClick={() => uploads.current?.click()}>Choose files</Pill>}
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
                <Pill quiet onClick={() => openExternal(issue || `${repository}/issues/new/choose`)}>
                  <ExternalLink size={14} /> {issue ? "Open issue on GitHub" : "Open GitHub"}
                </Pill>
                <div className="ml-auto flex items-center gap-2">
                  {issue ? (
                    <Pill prominent onClick={() => close(false)}>
                      Done
                    </Pill>
                  ) : (
                    <>
                      <Pill quiet disabled={submitting} onClick={() => close(false)}>
                        Cancel
                      </Pill>
                      {setupError && isTauri() ? (
                        <Pill prominent onClick={() => setAttempt(attempt + 1)}>
                          Retry connection
                        </Pill>
                      ) : (
                        <Pill
                          type="submit"
                          prominent
                          disabled={!options || !title.trim() || !description.trim() || submitting}
                        >
                          {submitting
                            ? "Submitting…"
                            : options || setupError
                              ? "Submit to GitHub"
                              : "Checking GitHub…"}
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
