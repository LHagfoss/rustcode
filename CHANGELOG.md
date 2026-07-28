## Unreleased
### Fixes
- Render code blocks as a solid full-width panel with an aligned copy button, and stop running prose fences through the Rust highlighter ([#74](https://github.com/LHagfoss/rustcode/pull/74) / [`c69fc07`](https://github.com/LHagfoss/rustcode/commit/c69fc07))
- Stop the loop detector from aborting an agent mid-recovery: read-only tool repeats only warn, a successful edit resets the detector, and non-unique-target edit errors point at the `start_line`/`end_line` anchor ([#75](https://github.com/LHagfoss/rustcode/pull/75) / [`3dadbfd`](https://github.com/LHagfoss/rustcode/commit/3dadbfd))

## [v0.3.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.3.1) - 2026-07-28
### Features
- Implement Pi/Claude Code single-line tool execution rendering ([#70](https://github.com/LHagfoss/rustcode/pull/70) / [`0361239`](https://github.com/LHagfoss/rustcode/commit/0361239))
- Implement `/compact` slash command to manually optimize session context ([#69](https://github.com/LHagfoss/rustcode/pull/69) / [`491ead8`](https://github.com/LHagfoss/rustcode/commit/491ead8))
- Compact tool call rendering and remove vertical line gaps ([#68](https://github.com/LHagfoss/rustcode/pull/68) / [`a218baf`](https://github.com/LHagfoss/rustcode/commit/a218baf))
- Add parameter aliases, batch edits array, and `view_file` directory fallback ([#66](https://github.com/LHagfoss/rustcode/pull/66) / [`d7f3af5`](https://github.com/LHagfoss/rustcode/commit/d7f3af5))
- Integrate `tiktoken-rs` for accurate token counting ([#64](https://github.com/LHagfoss/rustcode/pull/64) / [`1c6a629`](https://github.com/LHagfoss/rustcode/commit/1c6a629))

### Fixes
- Use absolute `/bin/sh` path for spawning compiler check ([`f945c25`](https://github.com/LHagfoss/rustcode/commit/f945c25))
- Remove fake `view_file` superseding, soften read repeat notice, increase `view_file` line window ([#65](https://github.com/LHagfoss/rustcode/pull/65) / [`e4d00a2`](https://github.com/LHagfoss/rustcode/commit/e4d00a2))

### Performance
- Memoize `tiktoken` BPE instance and remove debug `eprintln` ([#67](https://github.com/LHagfoss/rustcode/pull/67) / [`d0cd4ad`](https://github.com/LHagfoss/rustcode/commit/d0cd4ad))

### Refactor
- Remove `tiktoken-rs` integration test ([`9b8c32f`](https://github.com/LHagfoss/rustcode/commit/9b8c32f))

## [v0.3.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.3.0) - 2026-07-27
### Features
- Add /summarize slash command ([#58](https://github.com/LHagfoss/rustcode/pull/58) / [`e22cfe8`](https://github.com/LHagfoss/rustcode/commit/e22cfe8))
- Add /info slash command ([`0c73c29`](https://github.com/LHagfoss/rustcode/commit/0c73c29))
- Remove verbose continuous mode completion message ([#51](https://github.com/LHagfoss/rustcode/pull/51) / [`ef73145`](https://github.com/LHagfoss/rustcode/commit/ef73145))

### Fixes
- Speed up summarize and render as normal assistant reply ([`3322a66`](https://github.com/LHagfoss/rustcode/commit/3322a66))
- Flatten transcript, add spinner/elapsed/debug logging for summarize ([`29e1c4e`](https://github.com/LHagfoss/rustcode/commit/29e1c4e))
- Drop lock and run detached to stop deadlock/freeze in summarize ([`d7fa35c`](https://github.com/LHagfoss/rustcode/commit/d7fa35c))
- Stop inheriting Claude Code's ~/.claude/skills ([`140ec66`](https://github.com/LHagfoss/rustcode/commit/140ec66))
- Run compile gate through sh so cargo resolves on GUI launch ([`7c52193`](https://github.com/LHagfoss/rustcode/commit/7c52193))
- Paste into ask_question modal + stop duplicate turns ([`0d9fd0d`](https://github.com/LHagfoss/rustcode/commit/0d9fd0d))
- Gate background-task wakeups and truncate their output ([`e2ac296`](https://github.com/LHagfoss/rustcode/commit/e2ac296))
- Memoize conversation render to fix scroll lag ([`410fd9a`](https://github.com/LHagfoss/rustcode/commit/410fd9a))
- Clipboard image paste chip and consistent chat bubble padding ([`72b89eb`](https://github.com/LHagfoss/rustcode/commit/72b89eb))
- Harden build gate, tool parsing, and command PATH ([`0853a32`](https://github.com/LHagfoss/rustcode/commit/0853a32))
- Expand leading tilde (~) in resolve_tool_path ([#50](https://github.com/LHagfoss/rustcode/pull/50) / [`32a7f65`](https://github.com/LHagfoss/rustcode/commit/32a7f65))
- Cancel stream when dismissing ask_question modal via Esc ([#49](https://github.com/LHagfoss/rustcode/pull/49) / [`cf8616b`](https://github.com/LHagfoss/rustcode/commit/cf8616b))

### Documentation
- Remove AGENTS.md, keep guidance in the hardcoded system prompt ([`3ebbda7`](https://github.com/LHagfoss/rustcode/commit/3ebbda7))
- Add AGENTS.md conventions + prompt nudge to mirror sibling patterns ([`a6cb380`](https://github.com/LHagfoss/rustcode/commit/a6cb380))

### Chore
- Remove history_token_budget configuration ([`db6b15d`](https://github.com/LHagfoss/rustcode/commit/db6b15d))

## [v0.2.2](https://github.com/LHagfoss/rustcode/compare/v0.2.1...v0.2.2) - 2026-07-26
- Fix: Invalidate read-only signature cache on file mutations ([4fcdae4](https://github.com/LHagfoss/rustcode/commit/4fcdae4))
- Fix: Restore /quota slash command branch ([e82ecd7](https://github.com/LHagfoss/rustcode/commit/e82ecd7))
- Feat: Add optional Discord Rich Presence integration and /discord toggle ([cba0f23](https://github.com/LHagfoss/rustcode/commit/cba0f23))
- Fix: Include pattern in grep category signature to prevent false loop aborts ([aa9995f](https://github.com/LHagfoss/rustcode/commit/aa9995f))
- Fix: Mandate sequential tool execution and view_file before editing in system prompt ([e79b26a](https://github.com/LHagfoss/rustcode/commit/e79b26a))
- Fix: Improve replace_file_content mismatch error feedback and loop warning guidance ([a5c3f79](https://github.com/LHagfoss/rustcode/commit/a5c3f79))
- Fix: Restore diff syntax colors in tool confirmation modal ([fa5cd65](https://github.com/LHagfoss/rustcode/commit/fa5cd65))

## [v0.2.1](https://github.com/LHagfoss/rustcode/compare/v0.2.0...v0.2.1) - 2026-07-25
- Fix: Hide intermediate loop warnings from TUI history view ([3accd84](https://github.com/LHagfoss/rustcode/commit/3accd84))
- Feat: Refactor sync command into subcommands (push, pull, init) with progress feedback ([3e26914](https://github.com/LHagfoss/rustcode/commit/3e26914))
- Feat: Add harder async-deadlock task and label quota bucket model aliases ([e27d344](https://github.com/LHagfoss/rustcode/commit/e27d344))
- Fix: Wire /quota slash command execution in TUI command picker ([0d4a13d](https://github.com/LHagfoss/rustcode/commit/0d4a13d))
- Fix: Dynamic sizing for tool confirmation modal based on diff preview content ([f681f3b](https://github.com/LHagfoss/rustcode/commit/f681f3b))
- Fix: Prevent out-of-bounds index panic in tool confirmation modal ([8923bf1](https://github.com/LHagfoss/rustcode/commit/8923bf1))
- Test: Task-based harness benchmark runner ([83df86e](https://github.com/LHagfoss/rustcode/commit/83df86e))
- Fix: Gemini OpenAI-compat, working single [Copy] badge and tighter chat gap ([d80ff36](https://github.com/LHagfoss/rustcode/commit/d80ff36))
- Feat: Interactive ask_question modal with custom-answer slot ([cab7714](https://github.com/LHagfoss/rustcode/commit/cab7714))

## [v0.2.0](https://github.com/LHagfoss/rustcode/compare/v0.1.19...v0.2.0) - 2025-05-14
- Refactor state management and UI components ([db638d4](https://github.com/LHagfoss/rustcode/commit/db638d4))
- Improve network view_file deduplication for paged reads ([8eb9770](https://github.com/LHagfoss/rustcode/commit/8eb9770))

## [v0.1.19](https://github.com/LHagfoss/rustcode/compare/v0.1.18...v0.1.19) - 2025-07-25
- Add network text and UI components ([7b31c7c](https://github.com/LHagfoss/rustcode/commit/7b31c7c))
- Improve changelog linking and slash command embedding ([04e0d0b](https://github.com/LHagfoss/rustcode/commit/04e0d0b), [48d4a15](https://github.com/LHagfoss/rustcode/commit/48d4a15))
- Add Git-backed cross-device config & skills sync ([39b8cb5](https://github.com/LHagfoss/rustcode/commit/39b8cb5))
- UI: add animated status bar and spinner, refine tool output display ([48d28d4](https://github.com/LHagfoss/rustcode/commit/48d28d4), [c300b3d](https://github.com/LHagfoss/rustcode/commit/c300b3d))
- Fixes: system prompt leak, config safeguard, and model profile updates ([cb08974](https://github.com/LHagfoss/rustcode/commit/cb08974), [c05482e](https://github.com/LHagfoss/rustcode/commit/c05482e), [2e928da](https://github.com/LHagfoss/rustcode/commit/2e928da))

 - 2025-07-25
- Add ApiNative tool protocol using provider function-calling ([3676af0](https://github.com/LHagfoss/rustcode/commit/3676af0))
- Improve JS/TS project check (use biome if available) ([018e245](https://github.com/LHagfoss/rustcode/commit/018e245))
- Stabilize prompt cache prefix and dedupe compiler checks ([33e7872](https://github.com/LHagfoss/rustcode/commit/33e7872))
- UI: dynamic modal sizing, scrollable diff preview, and toggleable inline diff cards ([64f9ac8](https://github.com/LHagfoss/rustcode/commit/64f9ac8))

## [v0.1.17](https://github.com/LHagfoss/rustcode/compare/v0.1.16...v0.1.17) - 2025-07-24
- Refactor: update dependencies, main logic, and UI structure ([c3146d1](https://github.com/LHagfoss/rustcode/commit/c3146d1))
- Chore: bump version to 0.1.17 ([464d7c5](https://github.com/LHagfoss/rustcode/commit/464d7c5))

## [v0.1.16](https://github.com/LHagfoss/rustcode/compare/v0.1.15...v0.1.16) - 2026-07-24

### Features
- Add manage_task tool for background task management ([d2ce287](https://github.com/LHagfoss/rustcode/commit/d2ce287))
- Add /skills slash command and Exa AI integration ([f1a88ef](https://github.com/LHagfoss/rustcode/commit/f1a88ef))
- Complete Skills feature with discovery scanner and prompt catalog injection ([6432c78](https://github.com/LHagfoss/rustcode/commit/6432c78))
- Render syntax-highlighted code diffs in chat history ([967913b](https://github.com/LHagfoss/rustcode/commit/967913b))
- Show original vs optimized prompt diff in status banner ([19832c5](https://github.com/LHagfoss/rustcode/commit/19832c5))
- Bring rich keyboard navigation to MCP edit modal ([0923e3a](https://github.com/LHagfoss/rustcode/commit/0923e3a))

### Fixes
- Enforce finish gate compile check on complete_task ([5b985cd](https://github.com/LHagfoss/rustcode/commit/5b985cd))
- Support both buckets and quotaBuckets JSON keys ([b4ef0a4](https://github.com/LHagfoss/rustcode/commit/b4ef0a4))
- Fix system message notice banner classification ([4628c95](https://github.com/LHagfoss/rustcode/commit/4628c95))
- Fix proxy URL resolution and API key lookup ([47c3336](https://github.com/LHagfoss/rustcode/commit/47c3336))
- Fallback model matching in fetch_model_quota ([c4aecf5](https://github.com/LHagfoss/rustcode/commit/c4aecf5))
- Send Authorization Bearer header in /quota command ([0772777](https://github.com/LHagfoss/rustcode/commit/0772777))
- Break orchestrator loop immediately on complete_task ([fffdd28](https://github.com/LHagfoss/rustcode/commit/fffdd28))
- Terminate continuous mode on plain text response ([25a4c2c](https://github.com/LHagfoss/rustcode/commit/25a4c2c))
- Fail fast on interactive sudo commands ([6267b23](https://github.com/LHagfoss/rustcode/commit/6267b23))
- Display complete_task result string as assistant reply ([24dae03](https://github.com/LHagfoss/rustcode/commit/24dae03))
- Preserve 100% exact user prompt ([b04ad73](https://github.com/LHagfoss/rustcode/commit/b04ad73))
- Distinguish prompt optimizer status from warning banners ([c7b28cf](https://github.com/LHagfoss/rustcode/commit/c7b28cf))
- Auto-repair loose tool JSON args and dedupe file reads ([b2fc4c2](https://github.com/LHagfoss/rustcode/commit/b2fc4c2))
- Restrict text selection to chat viewport ([37adf21](https://github.com/LHagfoss/rustcode/commit/37adf21))
- Route bracketed paste events to active modal ([dd20f1d](https://github.com/LHagfoss/rustcode/commit/dd20f1d))

### Chores
- Fix clippy warnings and hoist regexes ([fac6653](https://github.com/LHagfoss/rustcode/commit/fac6653))
- Cleanup and refactor orchestrator prologue ([5b599a6](https://github.com/LHagfoss/rustcode/commit/5b599a6))

## [v0.1.15] - 2025-07-11

### Features
- Add right-aligned [Copy] badge to code blocks and clean code extraction for /copy
- Add goal mode completion green banner when continuous autoloop completes
- Add @ file reference autocomplete popup and tab completion
- Add unified green/red inline diff rendering for file edits

### Fixes
- Support Ctrl+Backspace for backward word deletion and explain Ghostty macos-option-as-alt setting
- Enable PushKeyboardEnhancementFlags for Ghostty and handle raw DEL events cleanly
- Support native macOS Option character compositions (∫, ƒ, ∂, \x7f, \x08, \x17) and Cmd+Backspace for word and line deletion
- Update prompt box mode label dynamically on Tab toggle and map Mac main delete key with Option to backward word deletion
- Ensure Option+Backspace deletes words backward while Option+Delete deletes words forward
- Silently handle missing cargo binary during background compiler check instead of returning fake error to model

### Chores
- Add KEY_EVENT debug logging to trace Ghostty Option+Backspace events

## [v0.1.13] - 2025-07-11

### Features
- Add Tab toggling between Build and Plan modes, enforce read-only tool guard in Plan mode, dynamically label system notices vs warnings
- Add persistent logger to `~/.config/rustcode/debug.log`, exclude left vertical border from text selection
- Implement double-escape key handling to cancel stream
- Add OpenCode-style inline compiler/LSP error diagnostics to tool outputs after file edits
- Port OpenCode harness improvements (fuzzy edit matching, anti-fluff directives, 3-repeat loop interception, multi-header auth)
- Enhance tool protocol with native format support and improve parser flexibility

### Fixes
- Store painted selection text during render pass to solve double-buffer empty clipboard issue
- Skip mouse selection highlighting on empty rows, margins, and empty space under chat
- Constrain mouse text selection strictly to chat viewport area
- Upgrade clipboard with OSC 52 ANSI escapes and clamp mouse text selection to actual line bounds
- Fix mouse text selection background color for high visibility
- Guard prompt classifier against conversational inputs and avoid continuous autoloops on non-tool replies
- Adjust user message rendering width and padding in ui.rs
- Fix network.rs issues
- Fixed some test errors

### UI
- Make system warning messages collapsible accordions by default
- Remove scrollbar UI components and logic while preserving scrolling functionality

### Performance
- Optimize small model classifier system prompt with few-shot examples for minicpm5
- Add retry, loop detection, faster token counting, better compaction
- Update dependencies and improve network compaction
- Update app state, config, context, network, compaction, tools, and UI modules

### Chores
- Cleanup unused variables in network.rs and tools/mod.rs

## [v0.1.12]
-e 
