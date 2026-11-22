import { useState, useSyncExternalStore } from "react";
import { revealPath } from "../lib/native";
import { dirname } from "../lib/paths";
import { appPlatform, paintsBlur } from "../lib/platform";
import { hasShell } from "../lib/shell";
import { BUILTIN_THEMES, cssName, type Theme, TOKENS } from "../lib/themes";
import { TRANSPARENCY_MAX, TRANSPARENCY_MIN } from "../lib/transparency";
import { getZoom, stepZoom, subscribeZoom, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN, zoomLabel } from "../lib/zoom";
import {
  createTheme,
  SETTINGS_DEFAULTS,
  setActiveTheme,
  setAnimateStreaming,
  setFont,
  setReduceTransparency,
  setTextSize,
  setTheme,
  setTransparency,
  toast,
  useApp,
} from "../state/store";
import { ChatBackgroundGroup } from "./ChatBackgroundGroup";
import { Check, FolderOpen, Minus, Plus } from "./icons";
import { LiveBackgroundGroup } from "./LiveBackgroundGroup";
import { cx, noTextHelpers, Switch } from "./primitives";
import { SettleAfterSetting, SidebarDensitySetting, SidebarToolbarSetting } from "./SidebarDensitySetting";
import { Action, Choice, fieldClass, Group, Row, SectionTitle, Slider } from "./settingsKit";

// hidden and inert, so closing Settings shows the thread exactly where it was.
// A grouped nav on the left searches every row's title and keywords
// (settingsIndex.ts) and shows one page at a time beside it; picking a result
// opens its page and lights the row. After SettingsView.swift.
// Server-backed pages (Harnesses, Connections, Capabilities, Appearance's
// themes, Usage, Statistics) read and write through `_rustcode/*` requests.
// Diagnostics (DiagnosticsPage.tsx) asks the shell about the server itself.

/** A small picture of the app in a theme's own colours: sidebar, rows, a reply and the composer. */
function ThemePreview({ theme }: { theme: Theme }) {
  const t = theme.tokens;
  const r = theme.radius === "sharp" ? 2 : theme.radius === "round" ? 7 : 4;
  return (
    <div className="flex h-[74px] w-full overflow-hidden" style={{ background: t.bg }} aria-hidden>
      <div className="flex w-[34%] flex-col gap-1 p-1.5" style={{ background: t.sidebar }}>
        <div className="h-2.5" style={{ background: t.surface_2, borderRadius: r }} />
        <div className="h-1.5 w-4/5" style={{ background: t.text_muted, opacity: 0.5, borderRadius: 2 }} />
        <div className="h-1.5 w-3/5" style={{ background: t.text_faint, opacity: 0.5, borderRadius: 2 }} />
      </div>
      <div className="flex flex-1 flex-col justify-between p-1.5">
        <div className="flex flex-col gap-1">
          <div className="ml-auto h-2.5 w-1/2" style={{ background: t.surface_2, borderRadius: r }} />
          <div className="h-1.5 w-4/5" style={{ background: t.text, opacity: 0.7, borderRadius: 2 }} />
          <div className="h-1.5 w-2/3" style={{ background: t.text_muted, opacity: 0.6, borderRadius: 2 }} />
        </div>
        <div
          className="flex h-4 items-center justify-end px-1"
          style={{ background: t.surface, borderRadius: r + 2, boxShadow: `0 0 0 1px ${t.border}` }}
        >
          <div className="size-2 rounded-full" style={{ background: t.accent }} />
        </div>
      </div>
    </div>
  );
}

function ThemeCard({ theme, active, onPick }: { theme: Theme; active: boolean; onPick: () => void }) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      onClick={onPick}
      className="group flex flex-col gap-1.5 text-left"
    >
      <span
        className={cx(
          "relative block overflow-hidden rounded-md shadow-1 transition-shadow duration-150",
          active ? "outline-2 outline-offset-2 outline-fg" : "group-hover:shadow-2",
        )}
      >
        <ThemePreview theme={theme} />
        {active && (
          <span className="absolute top-1 right-1 flex size-4 items-center justify-center rounded-full bg-fg text-bg">
            <Check size={10} strokeWidth={3} />
          </span>
        )}
      </span>
      <span className="flex items-baseline gap-1.5 px-0.5">
        <span className={cx("truncate text-callout", active ? "font-medium text-fg" : "text-fg-muted")}>
          {theme.name}
        </span>
        <span className="shrink-0 text-caption text-fg-faint">
          {theme.source === "builtin" ? theme.mode : theme.source}
        </span>
      </span>
    </button>
  );
}

function NewTheme({ base, onDone }: { base: string; onDone: () => void }) {
  const [name, setName] = useState("");
  const [cli, setCli] = useState<string | null>(null);
  const valid = /^[a-z0-9][a-z0-9-]{0,63}$/.test(name);
  const create = async () => {
    try {
      const path = await createTheme(name, base);
      if (path === null) {
        setCli(`rc theme new ${name} --from ${base}`);
        return;
      }
      toast(path ? `Created ${path}` : "Theme created", "success");
      onDone();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  };
  return (
    <div className="rc-expand rule-t flex flex-col gap-2 px-4 py-3">
      <div className="flex items-center gap-2">
        <input
          autoFocus
          value={name}
          onChange={(e) => {
            setName(e.target.value.toLowerCase());
            setCli(null);
          }}
          onKeyDown={(e) => e.key === "Enter" && valid && void create()}
          placeholder="theme-name"
          aria-label="Theme name"
          aria-invalid={name.length > 0 && !valid}
          {...noTextHelpers}
          className={cx(fieldClass, "flex-1 font-mono")}
        />
        <Action onClick={onDone}>Cancel</Action>
        <Action prominent disabled={!valid} onClick={() => void create()}>
          Create
        </Action>
      </div>
      {name.length > 0 && !valid ? (
        <p className="m-0 text-caption text-warning">
          A theme name is lowercase letters, digits and dashes, up to 64, starting with a letter or digit.
        </p>
      ) : cli ? (
        <p className="m-0 text-caption text-fg-muted">
          This server cannot create themes from the app. In a terminal:{" "}
          <code className="selectable rounded-xs bg-fill px-1 py-px font-mono text-fg">{cli}</code>
        </p>
      ) : (
        <p className="m-0 text-caption text-fg-faint">A copy of {base} in your themes folder, to edit.</p>
      )}
    </div>
  );
}

function UIScaleSetting() {
  const scale = useSyncExternalStore(subscribeZoom, getZoom);
  const change = (direction: -1 | 0 | 1) => {
    void stepZoom(direction).catch(() => toast("Could not change UI scale", "error"));
  };
  return (
    <Row
      title="UI scale"
      subtitle="Resizes the whole interface, including text, panels, and controls."
      reset={scale === ZOOM_DEFAULT ? undefined : () => change(0)}
    >
      <div role="group" aria-label="UI scale" className="flex items-center gap-2">
        <Action aria-label="Decrease UI scale" disabled={scale === ZOOM_MIN} onClick={() => change(-1)}>
          <Minus size={14} />
        </Action>
        <output className="w-10 text-center font-medium text-callout text-fg tabular">
          {zoomLabel(scale)}
        </output>
        <Action aria-label="Increase UI scale" disabled={scale === ZOOM_MAX} onClick={() => change(1)}>
          <Plus size={14} />
        </Action>
      </div>
    </Row>
  );
}

export function AppearancePage() {
  const mode = useApp((s) => s.theme);
  const font = useApp((s) => s.font);
  const textSize = useApp((s) => s.textSize);
  const animateStreaming = useApp((s) => s.animateStreaming);
  const reduceTransparency = useApp((s) => s.reduceTransparency);
  const transparency = useApp((s) => s.transparency);
  const active = useApp((s) => s.themeActive);
  const snapshot = useApp((s) => s.themes);
  const supported = useApp((s) => s.features.themes);
  const [creating, setCreating] = useState(false);
  const themes = snapshot?.themes.length ? snapshot.themes : BUILTIN_THEMES;
  const userPath = themes.find((t) => t.source === "user" && t.path)?.path;
  const userDir = userPath
    ? dirname(userPath)
    : appPlatform() === "windows"
      ? "%APPDATA%\\rustcode\\themes"
      : "~/.config/rustcode/themes";
  return (
    <>
      <Group title="Mode">
        <Row
          title="Light or dark"
          reset={mode !== SETTINGS_DEFAULTS.theme ? () => setTheme(SETTINGS_DEFAULTS.theme) : undefined}
          subtitle="System follows your operating system."
        >
          <Choice
            label="Appearance"
            value={mode}
            onChange={setTheme}
            options={[
              { value: "system", label: "System" },
              { value: "light", label: "Light" },
              { value: "dark", label: "Dark" },
            ]}
          />
        </Row>
        <Row
          title="Font"
          reset={font !== SETTINGS_DEFAULTS.font ? () => setFont(SETTINGS_DEFAULTS.font) : undefined}
          subtitle="For the interface and chat. Code stays monospaced."
        >
          <Choice
            label="Font"
            value={font}
            onChange={setFont}
            options={[
              { value: "inter", label: "Inter" },
              { value: "system", label: "System" },
              { value: "rounded", label: "Rounded" },
            ]}
          />
        </Row>
        <UIScaleSetting />
        <Row
          title="Text size"
          reset={
            textSize !== SETTINGS_DEFAULTS.textSize
              ? () => setTextSize(SETTINGS_DEFAULTS.textSize)
              : undefined
          }
          subtitle="Larger sizes scale the chat and sidebar text."
        >
          <Choice
            label="Text size"
            value={textSize}
            onChange={setTextSize}
            options={[
              { value: "small", label: "Default" },
              { value: "medium", label: "Large" },
              { value: "large", label: "Larger" },
            ]}
          />
        </Row>
        <Row
          title="Animate streaming text"
          reset={animateStreaming ? undefined : () => setAnimateStreaming(SETTINGS_DEFAULTS.animateStreaming)}
          subtitle="New words fade in as the model writes them."
        >
          <Switch label="Animate streaming text" checked={animateStreaming} onChange={setAnimateStreaming} />
        </Row>
        <Row
          title="Reduce transparency"
          reset={
            reduceTransparency !== SETTINGS_DEFAULTS.reduceTransparency
              ? () => setReduceTransparency(SETTINGS_DEFAULTS.reduceTransparency)
              : undefined
          }
          subtitle="Menus, dialogs and the sidebar turn solid instead of frosted."
          hint="Also on when your system asks for it."
        >
          <Switch label="Reduce transparency" checked={reduceTransparency} onChange={setReduceTransparency} />
        </Row>
        {paintsBlur() && (
          <Row
            title="Transparency"
            reset={
              transparency !== SETTINGS_DEFAULTS.transparency
                ? () => setTransparency(SETTINGS_DEFAULTS.transparency)
                : undefined
            }
            subtitle="How much of the window shows through the sidebar."
            hint={reduceTransparency ? "Reduce transparency is on, so the sidebar stays solid." : undefined}
          >
            <Slider
              label="Transparency"
              value={transparency}
              min={TRANSPARENCY_MIN}
              max={TRANSPARENCY_MAX}
              step={2}
              unit="%"
              onChange={setTransparency}
            />
          </Row>
        )}
        <SidebarDensitySetting />
        <SidebarToolbarSetting />
        <SettleAfterSetting />
      </Group>
      <Group
        title="Theme"
        action={
          <div className="flex items-center gap-1">
            {hasShell() && (
              <Action quiet size="sm" onClick={() => revealPath(userDir)}>
                <FolderOpen size={12} /> Open themes folder
              </Action>
            )}
            <Action size="sm" onClick={() => setCreating(!creating)} aria-expanded={creating}>
              <Plus size={12} /> New theme
            </Action>
          </div>
        }
        footer={
          supported === false
            ? "This server has no theme files yet. Update RustCode to add your own."
            : `Theme files live in ${userDir}; edits apply live. See docs/themes.md.`
        }
      >
        <div
          role="radiogroup"
          aria-label="Theme"
          className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-4 p-4"
        >
          {themes.map((t) => (
            <ThemeCard
              key={`${t.source}/${t.name}`}
              theme={t}
              active={t.name === active && (mode === "system" ? true : t.mode === mode)}
              onPick={() => void setActiveTheme(t.name)}
            />
          ))}
        </div>
        {creating && <NewTheme base={active} onDone={() => setCreating(false)} />}
      </Group>
      <LiveBackgroundGroup />
      <ChatBackgroundGroup />
      <TokenList />
    </>
  );
}

/** The live tokens, for theme authors: what each variable resolves to now. */
function TokenList() {
  const [open, setOpen] = useState(false);
  return (
    <section className="mb-6">
      <SectionTitle
        title="Tokens"
        action={
          <Action quiet size="sm" onClick={() => setOpen(!open)} aria-expanded={open}>
            {open ? "Hide" : "Show"}
          </Action>
        }
      />
      {open && (
        <div className="rc-expand grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-2 rounded-xl bg-surface p-3">
          {TOKENS.map((t) => (
            <div key={t} className="flex items-center gap-2 rounded-lg bg-surface-2 px-2 py-1.5">
              <span
                className="size-4 shrink-0 rounded-xs shadow-1"
                style={{ background: `var(${cssName(t)})` }}
              />
              <span className="truncate font-mono text-caption text-fg-muted">{t}</span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
