// Settings › Appearance › Chat background: the picture behind the chat (lib/banner.ts), how it is
// drawn (as is, in text, printed by a worker as a dither, a halftone or scanlines, or hazed),
// how dark and blurred it is, whether open threads show it too and how strongly.

import {
  BANNER_BLUR_DEFAULT,
  BANNER_BLUR_MAX,
  BANNER_DARKEN_DEFAULT,
  BANNER_DARKEN_MAX,
  BANNER_IN_THREADS_DEFAULT,
  BANNER_NAME_DEFAULT,
  BANNER_STYLE_DEFAULT,
  BANNER_VISIBILITY_DEFAULT,
  BANNER_VISIBILITY_MAX,
  type BannerStyle,
  chooseBanner,
  darkenOverlay,
  parseBannerStyle,
  removeBanner,
  setBannerBlur,
  setBannerDarken,
  setBannerInThreads,
  setBannerStyle,
  setBannerVisibility,
  useBanner,
} from "../lib/banner";
import { toast } from "../state/store";
import { NewThreadBanner } from "./NewThreadBanner";
import { Select } from "./primitives";
import { Action, Choice, Group, Row, Slider } from "./settingsKit";

const EFFECTS: { value: BannerStyle; label: string }[] = [
  { value: "picture", label: "None" },
  { value: "ascii", label: "ASCII" },
  { value: "dither", label: "Dither" },
  { value: "halftone", label: "Halftone" },
  { value: "scanlines", label: "Scanlines" },
  { value: "haze", label: "Haze" },
];

export function ChatBackgroundGroup() {
  const banner = useBanner();
  const choose = () =>
    void chooseBanner().catch((e) =>
      toast(`Could not use that picture: ${e instanceof Error ? e.message : e}`, "error"),
    );
  // No picture is the default: choosing one is the user's call, so removing it is the reset.
  const reset = () => {
    void removeBanner();
    setBannerStyle(BANNER_STYLE_DEFAULT);
    setBannerDarken(BANNER_DARKEN_DEFAULT);
    setBannerBlur(BANNER_BLUR_DEFAULT);
    setBannerInThreads(BANNER_IN_THREADS_DEFAULT);
    setBannerVisibility(BANNER_VISIBILITY_DEFAULT);
  };
  return (
    <Group title="Chat background">
      <Row
        title="Picture"
        reset={banner.name !== BANNER_NAME_DEFAULT ? reset : undefined}
        subtitle={
          banner.name
            ? "Behind the chat, cropped to fill it."
            : "A picture behind new threads, drawn as is or in text."
        }
      >
        <div className="flex shrink-0 items-center gap-2">
          {banner.name && <Action onClick={() => void removeBanner()}>Remove</Action>}
          <Action onClick={choose}>{banner.name ? "Change…" : "Choose image…"}</Action>
        </div>
      </Row>
      {banner.name && (
        <>
          <Row
            title="Effect"
            reset={
              banner.style !== BANNER_STYLE_DEFAULT ? () => setBannerStyle(BANNER_STYLE_DEFAULT) : undefined
            }
            subtitle="Drawn once, then kept"
          >
            <Select
              label="Background effect"
              value={banner.style}
              options={EFFECTS}
              onChange={(value) => setBannerStyle(parseBannerStyle(value))}
            />
          </Row>
          <Row
            title="Darken"
            reset={
              banner.darken !== BANNER_DARKEN_DEFAULT
                ? () => setBannerDarken(BANNER_DARKEN_DEFAULT)
                : undefined
            }
            subtitle="Black over the picture, so the text reads"
          >
            <Slider
              label="Darken the background"
              value={banner.darken}
              min={0}
              max={BANNER_DARKEN_MAX}
              step={5}
              unit="%"
              onChange={setBannerDarken}
            />
          </Row>
          <Row
            title="Blur"
            reset={banner.blur !== BANNER_BLUR_DEFAULT ? () => setBannerBlur(BANNER_BLUR_DEFAULT) : undefined}
            subtitle="Softens the picture"
          >
            <Slider
              label="Blur the background"
              value={banner.blur}
              min={0}
              max={BANNER_BLUR_MAX}
              step={2}
              unit="px"
              onChange={setBannerBlur}
            />
          </Row>
          <Row
            title="Show on"
            reset={
              banner.inThreads !== BANNER_IN_THREADS_DEFAULT
                ? () => setBannerInThreads(BANNER_IN_THREADS_DEFAULT)
                : undefined
            }
            subtitle="All threads puts it behind the transcript too, faded"
          >
            <Choice
              label="Show the background on"
              value={banner.inThreads ? "all" : "new"}
              onChange={(value) => setBannerInThreads(value === "all")}
              options={[
                { value: "new", label: "New threads" },
                { value: "all", label: "All threads" },
              ]}
            />
          </Row>
          {banner.inThreads && (
            <Row
              title="Visibility"
              reset={
                banner.visibility !== BANNER_VISIBILITY_DEFAULT
                  ? () => setBannerVisibility(BANNER_VISIBILITY_DEFAULT)
                  : undefined
              }
              subtitle="How strongly it shows behind a thread's messages"
            >
              <Slider
                label="Background visibility in threads"
                value={banner.visibility}
                min={0}
                max={BANNER_VISIBILITY_MAX}
                step={5}
                unit="%"
                onChange={setBannerVisibility}
              />
            </Row>
          )}
          {/* As a new thread shows it, at a size that reads. */}
          <div className="px-4 pb-4">
            <div className="relative overflow-hidden rounded-lg bg-bg">
              <NewThreadBanner
                key={banner.style}
                name={banner.name}
                style={banner.style}
                height={240}
                blur={banner.blur}
              />
              {banner.darken > 0 && (
                <div
                  aria-hidden
                  className="pointer-events-none absolute inset-0"
                  style={{ background: darkenOverlay(banner.darken) ?? undefined }}
                />
              )}
            </div>
          </div>
        </>
      )}
    </Group>
  );
}
