# Changelog

## [v0.1.16] - 2026-07-24

### Features
- Add manage_task tool for background task management
- Add /skills slash command and Exa AI integration
- Complete Skills feature with discovery scanner and prompt catalog injection
- Render syntax-highlighted code diffs in chat history
- Show original vs optimized prompt diff in status banner
- Bring rich keyboard navigation to MCP edit modal

### Fixes
- Enforce finish gate compile check on complete_task
- Support both buckets and quotaBuckets JSON keys
- Fix system message notice banner classification
- Fix proxy URL resolution and API key lookup
- Fallback model matching in fetch_model_quota
- Send Authorization Bearer header in /quota command
- Break orchestrator loop immediately on complete_task
- Terminate continuous mode on plain text response
- Fail fast on interactive sudo commands
- Display complete_task result string as assistant reply
- Preserve 100% exact user prompt
- Distinguish prompt optimizer status from warning banners
- Auto-repair loose tool JSON args and dedupe file reads
- Restrict text selection to chat viewport
- Route bracketed paste events to active modal

### Chores
- Fix clippy warnings and hoist regexes
- Cleanup and refactor orchestrator prologue

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
