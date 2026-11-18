// Settings › Appearance › Live background: a field of dithered light behind the chat, after
// Sable's backdrop (lib/backdrop). Presets as live swatches, then the pattern, its texture and
// colour, how strongly it shows behind a new thread and behind an open thread's composer, how fast
// it drifts and whether it answers what you do.

import {
  LIVE_BACKDROP_DEFAULT,
  LIVE_PRESETS,
  type LiveBackdrop,
  type LiveColour,
  type LivePattern,
  type LiveStyle,
  patchLiveBackdrop,
  sameLiveBackdrop,
  setLiveBackdrop,
  useLiveBackdrop,
} from "../lib/backdrop/config";
import { useBanner } from "../lib/banner";
import { newThreadChoice, useApp } from "../state/store";
import { LiveBackdrop as Field } from "./LiveBackdrop";
import { cx } from "./primitives";
import { Action, Choice, Group, Row, Slider } from "./settingsKit";

const PATTERNS: { value: LivePattern; label: string }[] = [
  { value: "streaks", label: "Streaks" },
  { value: "tubes", label: "Tubes" },
  { value: "tiles", label: "Tiles" },
  { value: "aurora", label: "Aurora" },
];

const STYLES: { value: LiveStyle; label: string }[] = [
  { value: "dither", label: "Dither" },
  { value: "ascii", label: "ASCII" },
  { value: "halftone", label: "Halftone" },
  { value: "soft", label: "Soft" },
];

const COLOURS: { value: LiveColour; label: string }[] = [
  { value: "harness", label: "Harness" },
  { value: "mono", label: "Mono" },
  { value: "custom", label: "Custom" },
];

/** A field's own settings, leaving what the presets do not set (the custom hue) as it is. */
function applyPreset(current: LiveBackdrop, preset: LiveBackdrop): LiveBackdrop {
  const colour = preset.colour === "harness" && current.colour === "custom" ? "custom" : preset.colour;
  return { ...preset, colour, hue: current.hue, saturation: current.saturation };
}

function Swatches({ cfg, harness }: { cfg: LiveBackdrop; harness?: string }) {
  return (
    <div className="grid grid-cols-3 gap-2 p-4 sm:grid-cols-5">
      {LIVE_PRESETS.map((p) => {
        const config = applyPreset(cfg, p.config);
        const on = cfg.source === "pattern" && sameLiveBackdrop(cfg, config);
        return (
          <button
            key={p.id}
            type="button"
            aria-pressed={on}
            onClick={() => setLiveBackdrop(config)}
            className="group flex flex-col items-center gap-1.5 text-caption text-fg-muted"
          >
            <span
              className={cx(
                "relative block aspect-[16/10] w-full overflow-hidden rounded-md bg-bg ring-1",
                on ? "ring-fg/60" : "ring-border group-hover:ring-border-strong",
              )}
            >
              <Field surface="swatch" harness={harness} config={config} />
            </span>
            <span className={on ? "text-fg" : undefined}>{p.name}</span>
          </button>
        );
      })}
    </div>
  );
}

export function LiveBackgroundGroup() {
  const cfg = useLiveBackdrop();
  const banner = useBanner();
  const harness = useApp((s) => newThreadChoice(s)?.harness);
  const on = cfg.source === "pattern";
  const d = LIVE_BACKDROP_DEFAULT;
  const reset = <K extends keyof LiveBackdrop>(key: K) =>
    cfg[key] !== d[key] ? () => patchLiveBackdrop({ [key]: d[key] } as Partial<LiveBackdrop>) : undefined;
  return (
    <Group
      title="Live background"
      enabled={on}
      onEnabledChange={(enabled) => patchLiveBackdrop({ source: enabled ? "pattern" : "off" })}
      switchLabel="Show the live background"
      // A preset also sets what has no row of its own (where the light gathers, how far it
      // reaches, its angle): this puts all of it back, where each row's reset puts back its own.
      action={
        on && !sameLiveBackdrop(cfg, d) ? (
          <Action quiet onClick={() => setLiveBackdrop(d)}>
            Reset
          </Action>
        ) : undefined
      }
      footer={
        banner.name
          ? "A chosen chat background picture shows instead on new threads."
          : "Rests after 20 seconds without input, and holds still under Reduce motion."
      }
    >
      {on && (
        <>
          <Swatches cfg={cfg} harness={harness} />
          <Row title="Pattern" reset={reset("pattern")}>
            <Choice
              label="Live background pattern"
              value={cfg.pattern}
              options={PATTERNS}
              onChange={(pattern) => patchLiveBackdrop({ pattern })}
            />
          </Row>
          <Row title="Texture" reset={reset("style")} subtitle="How each cell of light is drawn">
            <Choice
              label="Live background texture"
              value={cfg.style}
              options={STYLES}
              onChange={(style) => patchLiveBackdrop({ style })}
            />
          </Row>
          <Row title="Colour" reset={reset("colour")} subtitle="Harness follows the thread's harness">
            <Choice
              label="Live background colour"
              value={cfg.colour}
              options={COLOURS}
              onChange={(colour) => patchLiveBackdrop({ colour })}
            />
          </Row>
          {cfg.colour === "custom" && (
            <Row title="Hue" reset={reset("hue")}>
              <Slider
                label="Live background hue"
                value={cfg.hue}
                min={0}
                max={360}
                step={5}
                unit="°"
                onChange={(hue) => patchLiveBackdrop({ hue })}
              />
            </Row>
          )}
          <Row
            title="New threads"
            reset={reset("amount")}
            subtitle="How strongly it shows behind a new thread"
          >
            <Slider
              label="Live background strength on new threads"
              value={cfg.amount}
              min={0}
              max={100}
              unit="%"
              onChange={(amount) => patchLiveBackdrop({ amount })}
            />
          </Row>
          <Row
            title="Open threads"
            reset={reset("threadAmount")}
            subtitle="The band under the composer: brighter while working, amber when waiting on you"
          >
            <Slider
              label="Live background strength in threads"
              value={cfg.threadAmount}
              min={0}
              max={100}
              unit="%"
              onChange={(threadAmount) => patchLiveBackdrop({ threadAmount })}
            />
          </Row>
          <Row title="Speed" reset={reset("speed")} subtitle="0 holds it still">
            <Slider
              label="Live background speed"
              value={cfg.speed}
              min={0}
              max={100}
              unit="%"
              onChange={(speed) => patchLiveBackdrop({ speed })}
            />
          </Row>
          <Row title="Detail" reset={reset("detail")} subtitle="Finer or coarser cells">
            <Slider
              label="Live background detail"
              value={cfg.detail}
              min={0}
              max={100}
              unit="%"
              onChange={(detail) => patchLiveBackdrop({ detail })}
            />
          </Row>
          <Row
            title="React"
            reset={reset("react")}
            subtitle="A wave when you send, a ring when a turn ends, light that follows the pointer"
          >
            <Choice
              label="Live background reacts"
              value={cfg.react ? "on" : "off"}
              onChange={(v) => patchLiveBackdrop({ react: v === "on" })}
              options={[
                { value: "on", label: "On" },
                { value: "off", label: "Off" },
              ]}
            />
          </Row>
        </>
      )}
    </Group>
  );
}
