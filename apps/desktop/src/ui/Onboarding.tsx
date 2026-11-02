// First-run onboarding (and Settings › Harnesses › Set up again): pick the
// harnesses to use, sign in with each one's own login, test it, then choose
// the default harness, model and effort for new threads.

import { Dialog } from "radix-ui";
import { useState } from "react";
import { normalizeDefault, validateDefault } from "../state/harness";
import {
  completeOnboarding,
  type NewThreadChoice,
  refreshHarnesses,
  setDefaultHarness,
  useApp,
} from "../state/store";
import { DefaultsForm, HarnessList } from "./Harnesses";
import { cx, Pill, Spinner } from "./primitives";

export function Onboarding({ onClose }: { onClose: () => void }) {
  const snapshot = useApp((s) => s.harnesses);
  const [step, setStep] = useState<0 | 1>(0);
  const [choice, setChoice] = useState<NewThreadChoice | null>(null);
  const [saving, setSaving] = useState(false);
  const value = choice ?? snapshot?.default ?? { harness: "rustcode", model: null, effort: null };
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
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-black/22" />
        <Dialog.Content
          className="fixed top-1/2 left-1/2 z-50 flex max-h-[calc(100vh-56px)] w-[min(640px,calc(100vw-56px))] -translate-x-1/2 -translate-y-1/2 flex-col rounded-[20px] border border-fg/8 bg-chat shadow-panel rc-in"
          onEscapeKeyDown={(e) => !snapshot?.onboarded && e.preventDefault()}
        >
          <div className="flex items-center gap-3 px-6 pt-6">
            <img src="/icon.png" alt="" className="size-10" />
            <div className="flex-1">
              <Dialog.Title className="font-semibold text-title2">
                {step === 0 ? "Choose your harnesses" : "Defaults for new threads"}
              </Dialog.Title>
              <Dialog.Description className="text-callout text-fg-2">
                {step === 0
                  ? "A harness runs a thread. RustCode's own engine is built in; other coding agents bring their own sign-in, models and rules."
                  : "New threads start on this harness. You can pick another one when a thread starts."}
              </Dialog.Description>
            </div>
          </div>
          <div className="flex gap-1.5 px-6 pt-3" aria-hidden>
            {[0, 1].map((i) => (
              <span
                key={i}
                className={cx("h-1 flex-1 rounded-full", i <= step ? "bg-brand-fill" : "bg-fill-2")}
              />
            ))}
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
            {!snapshot ? (
              <div className="flex items-center gap-2 text-callout text-fg-2">
                <Spinner /> Looking for harnesses…
              </div>
            ) : step === 0 ? (
              <>
                <HarnessList snapshot={snapshot} />
                <button
                  type="button"
                  onClick={() => void refreshHarnesses()}
                  className="mt-2 text-callout text-brand hover:underline"
                >
                  Detect again
                </button>
              </>
            ) : (
              <DefaultsForm snapshot={snapshot} value={value} onChange={setChoice} />
            )}
          </div>
          <div className="flex items-center gap-2 border-hairline border-t px-6 py-3.5">
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
                {saving && <Spinner size={10} className="text-white" />}
                Finish
              </Pill>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
