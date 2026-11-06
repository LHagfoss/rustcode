// design-lint: allow-file (brand marks carry their owners' colours, not theme tokens)
//
// The mark of each harness, inline so nothing is fetched at run time:
// - Codex: the OpenAI blossom, from @lobehub/icons-static-svg 1.102.0 (MIT,
//   https://github.com/lobehub/lobe-icons), monochrome in currentColor.
// - Claude Code: the Claude mark in its orange, same source.
// - Gemini CLI: the Gemini spark with its gradient, same source.
// - OpenCode: `packages/identity/mark.svg` of https://github.com/anomalyco/opencode
//   (MIT), its two greys as currentColor at full and 35%, cropped to the mark.
// - RustCode: the app's own icon.
// - Pi: the letter π in the rounded square other harnesses get.
// NOTICE: these marks are trademarks of their owners (OpenAI, Anthropic,
// Google, Anomaly/OpenCode); they are used only to identify the integration.
// A harness that sends its own `icon` (a URL or `data:`) shows that instead,
// and any other harness a letter in a rounded square.

import { useId } from "react";
import { cx } from "./primitives";

const OPENAI =
  "M9.205 8.658v-2.26c0-.19.072-.333.238-.428l4.543-2.616c.619-.357 1.356-.523 2.117-.523 2.854 0 4.662 2.212 4.662 4.566 0 .167 0 .357-.024.547l-4.71-2.759a.797.797 0 00-.856 0l-5.97 3.473zm10.609 8.8V12.06c0-.333-.143-.57-.429-.737l-5.97-3.473 1.95-1.118a.433.433 0 01.476 0l4.543 2.617c1.309.76 2.189 2.378 2.189 3.948 0 1.808-1.07 3.473-2.76 4.163zM7.802 12.703l-1.95-1.142c-.167-.095-.239-.238-.239-.428V5.899c0-2.545 1.95-4.472 4.591-4.472 1 0 1.927.333 2.712.928L8.23 5.067c-.285.166-.428.404-.428.737v6.898zM12 15.128l-2.795-1.57v-3.33L12 8.658l2.795 1.57v3.33L12 15.128zm1.796 7.23c-1 0-1.927-.332-2.712-.927l4.686-2.712c.285-.166.428-.404.428-.737v-6.898l1.974 1.142c.167.095.238.238.238.428v5.233c0 2.545-1.974 4.472-4.614 4.472zm-5.637-5.303l-4.544-2.617c-1.308-.761-2.188-2.378-2.188-3.948A4.482 4.482 0 014.21 6.327v5.423c0 .333.143.571.428.738l5.947 3.449-1.95 1.118a.432.432 0 01-.476 0zm-.262 3.9c-2.688 0-4.662-2.021-4.662-4.519 0-.19.024-.38.047-.57l4.686 2.71c.286.167.571.167.856 0l5.97-3.448v2.26c0 .19-.07.333-.237.428l-4.543 2.616c-.619.357-1.356.523-2.117.523zm5.899 2.83a5.947 5.947 0 005.827-4.756C22.287 18.339 24 15.84 24 13.296c0-1.665-.713-3.282-1.998-4.448.119-.5.19-.999.19-1.498 0-3.401-2.759-5.947-5.946-5.947-.642 0-1.26.095-1.88.31A5.962 5.962 0 0010.205 0a5.947 5.947 0 00-5.827 4.757C1.713 5.447 0 7.945 0 10.49c0 1.666.713 3.283 1.998 4.448-.119.5-.19 1-.19 1.499 0 3.401 2.759 5.946 5.946 5.946.642 0 1.26-.095 1.88-.309a5.96 5.96 0 004.162 1.713z";

const CLAUDE =
  "M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z";

const GEMINI =
  "M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z";

/** Which mark a harness id or name shows. */
function markOf(id: string, name?: string): "openai" | "claude" | "gemini" | "opencode" | undefined {
  const key = `${id} ${name ?? ""}`.toLowerCase();
  if (key.includes("claude")) return "claude";
  if (key.includes("gemini")) return "gemini";
  if (key.includes("opencode")) return "opencode";
  if (key.includes("codex") || key.includes("openai")) return "openai";
  return undefined;
}

function GeminiMark({ mono }: { mono: boolean }) {
  const id = useId().replace(/:/g, "");
  if (mono) return <path d={GEMINI} fill="currentColor" />;
  return (
    <>
      <defs>
        <linearGradient id={`${id}g0`} gradientUnits="userSpaceOnUse" x1="7" x2="11" y1="15.5" y2="12">
          <stop stopColor="#08B962" />
          <stop offset="1" stopColor="#08B962" stopOpacity="0" />
        </linearGradient>
        <linearGradient id={`${id}g1`} gradientUnits="userSpaceOnUse" x1="8" x2="11.5" y1="5.5" y2="11">
          <stop stopColor="#F94543" />
          <stop offset="1" stopColor="#F94543" stopOpacity="0" />
        </linearGradient>
        <linearGradient id={`${id}g2`} gradientUnits="userSpaceOnUse" x1="3.5" x2="17.5" y1="13.5" y2="12">
          <stop stopColor="#FABC12" />
          <stop offset=".46" stopColor="#FABC12" stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={GEMINI} fill="#3186FF" />
      <path d={GEMINI} fill={`url(#${id}g0)`} />
      <path d={GEMINI} fill={`url(#${id}g1)`} />
      <path d={GEMINI} fill={`url(#${id}g2)`} />
    </>
  );
}

export function HarnessIcon({
  id,
  name,
  icon,
  size = 14,
  mono = false,
  plain = false,
  className,
}: {
  id: string;
  name?: string;
  /** A harness-supplied icon (URL or `data:`), preferred when present. */
  icon?: string;
  size?: number;
  /** Draw a coloured mark (Claude, Gemini) in currentColor too, e.g. for an unavailable row. */
  mono?: boolean;
  /**
   * A harness with no mark of its own keeps its letter alone, with no square
   * behind it: it is drawn inside a circle (the agent stack), where a square
   * would read as a badge within a badge.
   */
  plain?: boolean;
  className?: string;
}) {
  if (icon && /^(https?:|data:image\/)/.test(icon)) {
    return (
      <img src={icon} alt="" width={size} height={size} className={cx("shrink-0 rounded-xs", className)} />
    );
  }
  if (id === "rustcode") {
    return (
      <img
        src="/icon.png"
        alt=""
        width={size}
        height={size}
        draggable={false}
        className={cx("shrink-0 rounded-xs", className)}
      />
    );
  }
  const mark = markOf(id, name);
  if (mark) {
    return (
      <svg
        viewBox={mark === "opencode" ? "96 96 320 320" : "0 0 24 24"}
        width={size}
        height={size}
        aria-hidden
        className={cx("shrink-0", className)}
        fill="currentColor"
      >
        {mark === "openai" && <path d={OPENAI} fillRule="evenodd" />}
        {mark === "claude" && <path d={CLAUDE} fill={mono ? "currentColor" : "#D97757"} />}
        {mark === "gemini" && <GeminiMark mono={mono} />}
        {mark === "opencode" && (
          <>
            <path d="M320 224V352H192V224H320Z" opacity={0.35} />
            <path
              fillRule="evenodd"
              clipRule="evenodd"
              d="M384 416H128V96H384V416ZM320 160H192V352H320V160Z"
            />
          </>
        )}
      </svg>
    );
  }
  // Pi has no mark of its own to ship; its name is the letter.
  const letter =
    id === "pi"
      ? "π"
      : (name || id)
          .replace(/[^a-z0-9]/gi, "")
          .charAt(0)
          .toUpperCase() || "?";
  if (plain)
    return (
      <span
        aria-hidden
        className={cx(
          "inline-flex shrink-0 items-center justify-center font-semibold leading-none",
          className,
        )}
        style={{ width: size, height: size, fontSize: Math.round(size * 1.05) }}
      >
        {letter}
      </span>
    );
  return (
    <span
      aria-hidden
      className={cx(
        "inline-flex shrink-0 items-center justify-center rounded-xs bg-fill-strong font-semibold text-fg-muted leading-none",
        className,
      )}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.62) }}
    >
      {letter}
    </span>
  );
}
