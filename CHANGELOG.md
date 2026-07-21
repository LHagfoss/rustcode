# Changelog

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
