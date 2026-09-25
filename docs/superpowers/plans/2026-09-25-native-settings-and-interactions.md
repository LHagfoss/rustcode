# Native Settings and Interactions Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a coherent graphite native GPUI interface with a first-class General settings page and reliable keyboard, approval, and hover interactions.

**Architecture:** Keep navigation and interaction state in `AppView`, extract pure policy helpers where behavior can be unit tested, and use one native palette module as the source of semantic surface and accent colors. Preserve the existing settings persistence and chat/session stores; the new page is a projection over those stores, not a second configuration system.

**Tech Stack:** Rust, GPUI, gpui-component, existing `rustcode-app` state modules, Cargo tests.

**Spec:** `docs/superpowers/specs/2026-09-25-native-settings-and-harness-reliability-design.md`

## Global Constraints

- Work only in the isolated task worktree and keep the user's checkout untouched.
- Preserve existing settings persistence, session loading, chat streaming, and command semantics.
- Use semantic palette constants; do not scatter replacement color literals through the view.
- Settings is an in-app destination opened by the sidebar row and `Cmd+,`; it is not a modal or a separate OS window.
- The General page must remain useful as more settings sections are added later.
- Slash commands are purple and bold only while the draft is a recognized command token; normal arguments retain normal composer styling.
- Arrow navigation and Enter completion must work while the composer retains focus, and completion must leave the caret after the inserted text.
- Pending permission requests must present persistent Approve and Deny controls above the composer until resolved.
- Copy controls must remain reachable while the pointer travels from message content to the control.
- Use existing icon assets/components, rounded rectangles only where they communicate grouping, and visible keyboard focus/disabled states.
- Follow test-driven development: add a failing focused test before each behavior change.
- Run `cargo check --tests` and `cargo test` before the branch is declared complete.

## Review Focus

- Keyboard event ownership and caret position after programmatic composer updates.
- One-way settings projection with no duplicate state or persistence path.
- Approval actions remain tied to the exact pending request and cannot approve a stale request.
- Hover hit regions include both content and copy affordance without overlaying selectable text.
- Contrast, hierarchy, control states, and visual consistency in the dark graphite theme.

---

## Task 1: Establish a semantic native palette and selected-session treatment

**Files:**
- Create: `crates/rustcode-app/src/theme.rs`
- Modify: `crates/rustcode-app/src/main.rs`
- Modify: `crates/rustcode-app/src/view.rs`
- Test: `crates/rustcode-app/src/theme.rs`

- [ ] Add a failing unit test asserting that the selected sidebar surface is lighter than the sidebar background and that primary/secondary foreground colors meet the intended light-on-dark ordering.
- [ ] Add a small `NativePalette`/semantic constants module covering app background, sidebar, elevated/composer surface, selected/hover surface, subtle/strong borders, primary/muted text, purple command accent, destructive/approval states, and focus ring.
- [ ] Export the module from `main.rs` and apply its tokens to the GPUI component dark theme during application setup so component-backed controls and custom surfaces share the same visual system.
- [ ] Replace the directly related hard-coded colors in root, sidebar, selected session row, message surfaces, composer, slash popup, and approval surfaces with semantic tokens. Do not mechanically rewrite unrelated syntax/highlight colors.
- [ ] Give the selected session row a visibly brighter neutral gray than both the sidebar and hover state, with readable text and a restrained rounded radius.
- [ ] Run `cargo test -p rustcode-app theme` and `cargo check -p rustcode-app --tests`.
- [ ] Review the diff for raw color literals added outside `theme.rs`, accidental state regressions, and contrast hierarchy; commit the task.

## Task 2: Replace the settings modal with an in-app General destination

**Files:**
- Modify: `crates/rustcode-app/src/view.rs`
- Modify: `crates/rustcode-app/src/settings.rs`
- Test: `crates/rustcode-app/src/view.rs`
- Test: `crates/rustcode-app/src/settings.rs`

- [ ] Add failing pure-state tests for `AppDestination::Chat`/`AppDestination::Settings(SettingsSection::General)`, opening settings, and returning to the previously selected chat without discarding the session.
- [ ] Introduce the minimal navigation state needed to distinguish chat content from the settings destination; make the existing `OpenSettings` action switch state instead of opening a modal.
- [ ] Add a bottom-anchored sidebar divider and Settings row with a real settings icon, active/hover/focus states, and click behavior matching `Cmd+,`.
- [ ] Render a Settings page with a compact section rail headed “Settings”, General selected at top, and a scrollable content area headed “General”. Keep the content width readable rather than filling the full window.
- [ ] Rebuild the currently supported settings as polished native rows: label and supporting text on the left, control on the right; use rounded dropdown/select controls for model and approval/permission mode only.
- [ ] Ensure each picker displays its current persisted value, writes through the existing settings update path, and exposes clear selected, hover, focus, and disabled states.
- [ ] Remove the obsolete modal construction without removing shared settings persistence helpers.
- [ ] Run focused `rustcode-app` settings/navigation tests and `cargo check -p rustcode-app --tests`.
- [ ] Review the page at narrow and wide native window sizes for clipping, excessive cards, inconsistent radii, and missing focus states; commit the task.

## Task 3: Make slash completion keyboard-correct and visibly intentional

**Files:**
- Modify: `crates/rustcode-app/src/slash.rs`
- Modify: `crates/rustcode-app/src/view.rs`
- Test: `crates/rustcode-app/src/slash.rs`
- Test: `crates/rustcode-app/src/view.rs`

- [ ] Add failing tests for key normalization/navigation policy (`ArrowUp`/`ArrowDown` plus GPUI canonical names), wrapping selection, Enter completion, Escape dismissal, and the exact cursor offset returned for an inserted command.
- [ ] Add a pure `SlashInteraction` decision helper that consumes the current draft, selected index, dismissal state, and key name and returns `Move`, `Complete { value, cursor_offset }`, `Dismiss`, or `Ignore`.
- [ ] Wire the composer key handler at the focus-owning input layer (or an action handler reached while it owns focus), stopping propagation only when a visible slash command menu handles the key.
- [ ] On Enter/click completion, set the command value, then call the textarea cursor API with `Position::new(0, cursor_offset)` after `set_value`, preserving composer focus.
- [ ] Add a recognized-command predicate that styles only the leading recognized slash token in purple bold when the component supports spans; if the textarea API cannot style ranges, style the full draft only while it consists solely of the recognized token and restore normal styling as soon as arguments are present.
- [ ] Keep mouse selection and keyboard selection synchronized and scroll the highlighted item into view when needed.
- [ ] Run focused slash/view tests and `cargo check -p rustcode-app --tests`.
- [ ] Self-review event propagation, IME/text-entry behavior, empty suggestions, and command-with-arguments behavior; commit the task.

## Task 4: Add persistent permission actions and reachable copy controls

**Files:**
- Modify: `crates/rustcode-app/src/ui_adapter.rs`
- Modify: `crates/rustcode-app/src/view.rs`
- Test: `crates/rustcode-app/src/ui_adapter.rs`
- Test: `crates/rustcode-app/src/view.rs`

- [ ] Add a failing adapter test that projects a pending approval request, including request identity, tool/action summary, and risk context, and removes it only after the matching decision is recorded.
- [ ] If the test proves the projection bridge incomplete, fix only the adapter path required to preserve pending approval information from the canonical chat state.
- [ ] Replace the transient approval dialog presentation with a persistent inline permission card immediately above the composer. Include clear Approve and Deny buttons, keyboard focus, disabled/in-flight state, and the exact pending request identity in callbacks.
- [ ] Preserve existing approval resolution effects and ensure Ask First allows safe non-confirming work while interrupting only confirmation-required actions.
- [ ] Add a failing layout/state test for the message hover region or extract a pure visibility policy proving that entering the copy-control region keeps the control visible.
- [ ] Move user and assistant copy buttons inside a shared parent hover/hit region (or reserve in-bounds space for them), while keeping message text selectable and the button clickable.
- [ ] Run focused adapter/view tests and `cargo check -p rustcode-app --tests`.
- [ ] Self-review stale approval callbacks, double submission, hover gaps, and overlap with message text; commit the task.

## Task 5: Integrate, visually audit, and verify the native surface

**Files:**
- Modify as required by verified defects only: `crates/rustcode-app/src/theme.rs`
- Modify as required by verified defects only: `crates/rustcode-app/src/view.rs`
- Modify as required by verified defects only: `crates/rustcode-app/src/settings.rs`

- [ ] Build and launch the native app against a disposable/fresh session and verify: selected session contrast, Settings sidebar/Cmd+comma navigation, General picker persistence, slash arrow/Enter/mouse completion and caret, slash styling, pending approval actions, and both copy buttons.
- [ ] Capture one batched inspection at representative narrow and wide sizes and check contrast, spacing, type hierarchy, focus, disabled, hover, loading, error, and empty states against the craft floor.
- [ ] Run the Impeccable detector exactly once over all changed UI targets: `/Users/lagos/.agents/skills/impeccable/scripts/impeccable detect --json crates/rustcode-app/src/theme.rs crates/rustcode-app/src/view.rs crates/rustcode-app/src/settings.rs crates/rustcode-app/src/ui_adapter.rs`.
- [ ] Fix confirmed detector/manual findings in one bounded batch and rerun only the focused tests covering those fixes.
- [ ] Run `cargo fmt --check`, `cargo check --tests`, and `cargo test` and record exact results.
- [ ] Review the entire branch diff against issue #1411 and the spec, remove incidental changes, and commit any bounded integration fixes.
