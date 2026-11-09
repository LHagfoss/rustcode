// "Add harness": a local ACP command, launched directly with no shell. It is
// verified on add (the ACP handshake and a model list, the same as Check) and
// kept off the pickers until that worked; a failure shows the error text and
// leaves nothing behind. Rules live in state/harnessForm.ts.

import { Plus, X } from "lucide-react";
import { useState } from "react";
import { forgetHarnessCheck, runHarnessCheck } from "../state/harnessChecks";
import {
  buildHarnessRequest,
  EMPTY_HARNESS_FORM,
  type HarnessForm,
  hasFormErrors,
  validateHarnessForm,
} from "../state/harnessForm";
import {
  addCustomHarness,
  discardCustomHarness,
  enableCustomHarness,
  errorText,
  toast,
  useApp,
} from "../state/store";
import { cx, Spinner } from "./primitives";
import { Action } from "./settingsKit";

const FIELD =
  "h-8 min-w-0 rounded-md bg-fill px-2 font-mono text-callout outline-none placeholder:font-sans placeholder:text-fg-faint focus:bg-fill-strong";

function Field({
  label,
  error,
  hint,
  children,
}: {
  label: string;
  error?: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-caption text-fg-muted">{label}</span>
      {children}
      {error ? (
        <span className="text-caption text-warning">{error}</span>
      ) : hint ? (
        <span className="text-caption text-fg-faint">{hint}</span>
      ) : null}
    </div>
  );
}

function RemoveRow({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      className="flex size-6 shrink-0 items-center justify-center rounded-md text-fg-faint hover:bg-fill hover:text-danger"
    >
      <X size={12} />
    </button>
  );
}

export function AddHarnessForm({ onClose }: { onClose: () => void }) {
  const harnesses = useApp((s) => s.harnesses?.harnesses ?? []);
  const [form, setForm] = useState<HarnessForm>(EMPTY_HARNESS_FORM);
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState<"adding" | "verifying" | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const errors = validateHarnessForm(
    form,
    harnesses.map((h) => h.name),
  );
  // Problems show once the user tried to add; before that the form is only being filled in.
  const shown = submitted ? errors : { args: [], env: [] };
  const patch = (next: Partial<HarnessForm>) => {
    setFailure(null);
    setForm((f) => ({ ...f, ...next }));
  };

  const add = async () => {
    setSubmitted(true);
    if (hasFormErrors(errors) || busy) return;
    const request = buildHarnessRequest(
      form,
      harnesses.map((h) => h.id),
    );
    setFailure(null);
    setBusy("adding");
    let created = false;
    try {
      await addCustomHarness(request);
      created = true;
      setBusy("verifying");
      const result = await runHarnessCheck(request.id);
      if (result.state !== "ready") throw new Error(result.text);
      // Verified: only now can a thread or the model picker choose it.
      await enableCustomHarness(request.id);
      toast(`${request.name} added`, "success");
      onClose();
    } catch (error) {
      setFailure(errorText(error));
      if (created) {
        // Nothing half-added is left in the list.
        await discardCustomHarness(request.id).catch(() => undefined);
        forgetHarnessCheck(request.id);
      }
    } finally {
      setBusy(null);
    }
  };

  return (
    <form
      className="flex flex-col gap-3 border-border border-b px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault();
        void add();
      }}
    >
      <div>
        <div className="font-semibold text-body">Local ACP command</div>
        <p className="text-caption text-fg-muted">
          For an agent that speaks the Agent Client Protocol over stdin and stdout. RustCode starts it
          directly, with no shell, so <code className="font-mono">~</code>,{" "}
          <code className="font-mono">$VAR</code> and quotes are passed as typed.
        </p>
      </div>
      <Field label="Name" error={shown.name}>
        <input
          autoFocus
          value={form.name}
          onChange={(e) => patch({ name: e.target.value })}
          placeholder="My agent"
          aria-label="Name"
          className={cx(FIELD, "font-sans")}
        />
      </Field>
      <Field label="Executable" error={shown.executable} hint="A name looked up on PATH, or a path.">
        <input
          value={form.executable}
          onChange={(e) => patch({ executable: e.target.value })}
          placeholder="dsh"
          aria-label="Executable"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          className={FIELD}
        />
      </Field>
      <Field label="Arguments" hint={form.args.length ? undefined : "None. One argument per row."}>
        <div className="flex flex-col gap-1.5">
          {form.args.map((arg, i) => (
            <div key={i} className="flex flex-col gap-1">
              <div className="flex items-center gap-1.5">
                <input
                  value={arg}
                  onChange={(e) => patch({ args: form.args.map((a, j) => (j === i ? e.target.value : a)) })}
                  placeholder={i === 0 ? "--profile" : "acp"}
                  aria-label={`Argument ${i + 1}`}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  className={cx(FIELD, "flex-1")}
                />
                <RemoveRow
                  label={`Remove argument ${i + 1}`}
                  onClick={() => patch({ args: form.args.filter((_, j) => j !== i) })}
                />
              </div>
              {shown.args[i] && <span className="text-caption text-warning">{shown.args[i]}</span>}
            </div>
          ))}
          <button
            type="button"
            onClick={() => patch({ args: [...form.args, ""] })}
            className="flex items-center gap-1 self-start text-caption text-accent hover:underline"
          >
            <Plus size={11} />
            Add argument
          </button>
        </div>
      </Field>
      <Field label="Environment" hint={form.env.length ? undefined : "None. Added to RustCode's own."}>
        <div className="flex flex-col gap-1.5">
          {form.env.map((row, i) => (
            <div key={i} className="flex flex-col gap-1">
              <div className="flex items-center gap-1.5">
                <input
                  value={row.name}
                  onChange={(e) =>
                    patch({ env: form.env.map((r, j) => (j === i ? { ...r, name: e.target.value } : r)) })
                  }
                  placeholder="NAME"
                  aria-label={`Variable ${i + 1} name`}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  className={cx(FIELD, "w-40")}
                />
                <span className="text-fg-faint">=</span>
                <input
                  value={row.value}
                  onChange={(e) =>
                    patch({ env: form.env.map((r, j) => (j === i ? { ...r, value: e.target.value } : r)) })
                  }
                  placeholder="value"
                  aria-label={`Variable ${i + 1} value`}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  className={cx(FIELD, "flex-1")}
                />
                <RemoveRow
                  label={`Remove variable ${i + 1}`}
                  onClick={() => patch({ env: form.env.filter((_, j) => j !== i) })}
                />
              </div>
              {(shown.env[i]?.name || shown.env[i]?.value) && (
                <span className="text-caption text-warning">{shown.env[i]?.name ?? shown.env[i]?.value}</span>
              )}
            </div>
          ))}
          <button
            type="button"
            onClick={() => patch({ env: [...form.env, { name: "", value: "" }] })}
            className="flex items-center gap-1 self-start text-caption text-accent hover:underline"
          >
            <Plus size={11} />
            Add variable
          </button>
        </div>
      </Field>
      {failure && (
        <div role="alert" className="selectable rounded-md bg-warning/10 px-3 py-2 text-callout text-warning">
          <div className="font-medium">Could not verify it</div>
          <div className="whitespace-pre-wrap break-words">{failure}</div>
          <div className="text-caption text-fg-muted">Nothing was added. Fix the command and try again.</div>
        </div>
      )}
      <div className="flex items-center gap-2">
        <Action type="submit" prominent disabled={!!busy}>
          {busy && <Spinner size={11} />}
          {busy === "adding" ? "Adding…" : busy === "verifying" ? "Verifying…" : "Add and verify"}
        </Action>
        <Action quiet disabled={!!busy} onClick={onClose}>
          Cancel
        </Action>
        {busy === "verifying" && (
          <span className="text-caption text-fg-muted">Starting it, ACP handshake, listing models.</span>
        )}
      </div>
    </form>
  );
}
