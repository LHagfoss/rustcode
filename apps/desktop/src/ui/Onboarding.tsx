// First-run onboarding (and Settings › Advanced › Set up again): pick the
// harnesses to use, sign in with each one's own login, test it, then choose
// the default harness, model and effort for new threads.

import { Dialog } from "radix-ui";
import { useState } from "react";
import { normalizeDefault, preferredDefault, validateDefault } from "../state/harness";
import {
  completeOnboarding,
  type NewThreadChoice,
  refreshHarnesses,
  setDefaultHarness,
  useApp,
} from "../state/store";
import { DefaultsForm, HarnessList, HarnessListSkeleton } from "./Harnesses";
import { cx, Pill, Spinner } from "./primitives";

export function Onboarding({ onClose }: { onClose: () => void }) {
  const snapshot = useApp((s) => s.harnesses);
  const [step, setStep] = useState<0 | 1>(0);
  const [choice, setChoice] = useState<NewThreadChoice | null>(null);
  const [saving, setSaving] = useState(false);
  // A first run suggests Claude Code (or the first agent that can run); later runs show the saved default.
  const value = choice ??
    (snapshot ? (snapshot.onboarded ? snapshot.default : preferredDefault(snapshot)) : null) ?? {
      harness: "rustcode",
      model: null,
      effort: null,
    };
  const problem = snapshot ? validateDefault(snapshot, value) : "Loading";

  const finish = async () => {
    if (!snapshot || problem) return;
    setSaving(true);
    await setDefaultHarness(value);
    await completeOnboarding();
    setSaving(false);
    onClose();
  };

  return (
    <Dialog.Root open onOpenChange={(open) => !open && snapshot?.onboarded && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <Dialog.Content
          className="elevated fixed top-1/2 left-1/2 z-50 flex max-h-[calc(100vh-56px)] w-[min(640px,calc(100vw-56px))] -translate-x-1/2 -translate-y-1/2 flex-col rc-in"
          onEscapeKeyDown={(e) => !snapshot?.onboarded && e.preventDefault()}
        >
          <div className="flex items-center gap-3 px-6 pt-6">
            <img src="/icon.png" alt="" className="size-10" />
            <div className="flex-1">
              <Dialog.Title className="font-semibold text-title2">
                {step === 0 ? "Choose your harnesses" : "Defaults for new threads"}
              </Dialog.Title>
              <Dialog.Description className="text-callout text-fg-muted">
                {step === 0
                  ? "A harness is the coding agent that runs a thread: Claude Code, Codex and others, each with its own sign-in, models and rules. RustCode's built-in harness stays available for the TUI and local models."
                  : "New threads start on this harness. You can pick another one when a thread starts."}
              </Dialog.Description>
            </div>
          </div>
          <div className="flex gap-1.5 px-6 pt-3" aria-hidden>
            {[0, 1].map((i) => (
              <span key={i} className={cx("h-1 flex-1 rounded-full", i <= step ? "bg-accent" : "bg-fill")} />
            ))}
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
            {!snapshot ? (
              <HarnessListSkeleton />
            ) : step === 0 ? (
              <>
                <HarnessList snapshot={snapshot} />
                <button
                  type="button"
                  onClick={() => void refreshHarnesses()}
                  className="mt-2 text-callout text-accent hover:underline"
                >
                  Detect again
                </button>
              </>
            ) : (
              <DefaultsForm snapshot={snapshot} value={value} onChange={setChoice} />
            )}
          </div>
          <div className="flex items-center gap-2 border-border border-t px-6 py-3">
            {snapshot?.onboarded && <Pill onClick={onClose}>Cancel</Pill>}
            <span className="flex-1" />
            {step === 1 && <Pill onClick={() => setStep(0)}>Back</Pill>}
            {step === 0 ? (
              <Pill
                prominent
                size="lg"
                disabled={!snapshot}
                onClick={() => {
                  if (snapshot) setChoice(normalizeDefault(snapshot, value));
                  setStep(1);
                }}
              >
                Continue
              </Pill>
            ) : (
              <Pill prominent size="lg" disabled={!!problem || saving} onClick={() => void finish()}>
                {saving && <Spinner size={10} className="text-bg" />}
                Finish
              </Pill>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
