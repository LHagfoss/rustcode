## [v0.16.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.16.1) - 2026-08-11

### Fixes
- **UI:** Hide serialized tool calls from TUI transcript view ([`8a9331b`](https://github.com/LHagfoss/rustcode/commit/8a9331b))
- **MCP:** Initialize MCP servers in ACP mode ([`e765a86`](https://github.com/LHagfoss/rustcode/commit/e765a86))
- **Config:** Isolate test config directory to prevent mutating user config ([#472](https://github.com/LHagfoss/rustcode/pull/472) / [`80fd96b`](https://github.com/LHagfoss/rustcode/commit/80fd96b))
- **UI:** Resolve ApiNative tool calls in UI and preserve thinking block rendering ([#471](https://github.com/LHagfoss/rustcode/pull/471) / [`5a96883`](https://github.com/LHagfoss/rustcode/commit/5a96883))
- **Tools:** Release state lock before tool execution to prevent stalls ([`5d713f2`](https://github.com/LHagfoss/rustcode/commit/5d713f2))

## [v0.16.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.16.0) - 2026-08-10

### Features
- **Server:** Add headless ACP server for non-TUI usage ([#470](https://github.com/LHagfoss/rustcode/pull/470) / [`59b4e81`](https://github.com/LHagfoss/rustcode/commit/59b4e81))

### Documentation
- **Server:** Design acp server runtime ([`e2f2edb`](https://github.com/LHagfoss/rustcode/commit/e2f2edb))

## [v0.15.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.15.0) - 2026-08-10

### Features
- **UI:** Add interactive `/protocol` picker modal and enforce `Verbosity::High` for tool rendering ([#467](https://github.com/LHagfoss/rustcode/pull/467) / [`6aca289`](https://github.com/LHagfoss/rustcode/commit/6aca289))

### Fixes
- **UI:** Reflow markdown soft breaks instead of forcing a new line ([#468](https://github.com/LHagfoss/rustcode/pull/468) / [`2522425`](https://github.com/LHagfoss/rustcode/commit/2522425))
- **Network:** Maintain `AppStatus::Streaming` during finish-gate compiler checks ([#467](https://github.com/LHagfoss/rustcode/pull/467) / [`5642520`](https://github.com/LHagfoss/rustcode/commit/5642520))

### Tuning
- **Context:** Keep more recent turns verbatim before pruning kicks in ([#465](https://github.com/LHagfoss/rustcode/pull/465) / [`76cb796`](https://github.com/LHagfoss/rustcode/commit/76cb796))

## [v0.14.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.14.0) - 2026-08-09

### Features
- **Config:** Add per-model profile settings for temperature, max_tokens, and enable_thinking ([#461](https://github.com/LHagfoss/rustcode/pull/461) / [`5ac11b0`](https://github.com/LHagfoss/rustcode/commit/5ac11b0))
- **Config/UI:** Add `/thinking` picker modal, drop request-level temperature, raise max_tokens default ([#463](https://github.com/LHagfoss/rustcode/pull/463) / [`ce6f20b`](https://github.com/LHagfoss/rustcode/commit/ce6f20b))
- **UI:** Collapse long pasted text into display chips with character counts ([#454](https://github.com/LHagfoss/rustcode/pull/454) / [`813946d`](https://github.com/LHagfoss/rustcode/commit/813946d))
- **UI:** Fix tool argument line wrap truncation and add max input box height ([#453](https://github.com/LHagfoss/rustcode/pull/453) / [`2637dd2`](https://github.com/LHagfoss/rustcode/commit/2637dd2))
- **Tools:** Instruct model to auto-trigger matching skills at task start ([#457](https://github.com/LHagfoss/rustcode/pull/457) / [`58a1116`](https://github.com/LHagfoss/rustcode/commit/58a1116))
- **UI:** Render empty parentheses `()` for tool calls with empty arguments ([#458](https://github.com/LHagfoss/rustcode/pull/458) / [`e7e0aa2`](https://github.com/LHagfoss/rustcode/commit/e7e0aa2))
- **Network/Tools:** Nudge unstuck model and infer missing tool names ([#452](https://github.com/LHagfoss/rustcode/pull/452) / [`60a6597`](https://github.com/LHagfoss/rustcode/commit/60a6597))

### Fixes
- **UI:** Hide raw compaction summary text block from TUI transcript view ([#462](https://github.com/LHagfoss/rustcode/pull/462) / [`5275956`](https://github.com/LHagfoss/rustcode/commit/5275956))
- **UI/Tools:** Preserve skill name in `use_skill` tool call parsing and UI header ([#460](https://github.com/LHagfoss/rustcode/pull/460) / [`0dfde5b`](https://github.com/LHagfoss/rustcode/commit/0dfde5b))
- **UI:** Hide dropped tool batch notices from TUI transcript view ([#459](https://github.com/LHagfoss/rustcode/pull/459) / [`590416f`](https://github.com/LHagfoss/rustcode/commit/590416f))
- **Tools:** Infer `complete_task` name when missing from malformed tool calls ([#455](https://github.com/LHagfoss/rustcode/pull/455) / [`6d31645`](https://github.com/LHagfoss/rustcode/commit/6d31645))
- Reset `AppStatus` to `Idle` on Esc modal dismiss and instruct models against duplicate write-in options ([#451](https://github.com/LHagfoss/rustcode/pull/451) / [`1fcbab4`](https://github.com/LHagfoss/rustcode/commit/1fcbab4))
- Suppress streaming text display for JSON and embedded tool calls ([#448](https://github.com/LHagfoss/rustcode/pull/448) / [`447128e`](https://github.com/LHagfoss/rustcode/commit/447128e))
- Improve ManageTask and TaskDone formatting in chat ([#447](https://github.com/LHagfoss/rustcode/pull/447) / [`0355036`](https://github.com/LHagfoss/rustcode/commit/0355036))

### Refactor
- Make tool registration a single self-contained definition ([`c66d431`](https://github.com/LHagfoss/rustcode/commit/c66d431))

### Chores
- Replace header.png with new image ([#449](https://github.com/LHagfoss/rustcode/pull/449) / [`6dee0f9`](https://github.com/LHagfoss/rustcode/commit/6dee0f9))
- Apply cargo fmt to 9 source files ([#456](https://github.com/LHagfoss/rustcode/pull/456) / [`6f09e35`](https://github.com/LHagfoss/rustcode/commit/6f09e35))

## [v0.13.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.13.2) - 2026-08-07

### Fixes
- Reset `AppStatus` to `Idle` on Esc modal dismiss and instruct models against duplicate write-in options ([#451](https://github.com/LHagfoss/rustcode/pull/451) / [`1fcbab4`](https://github.com/LHagfoss/rustcode/commit/1fcbab4))
- Suppress streaming text display for JSON and embedded tool calls ([#448](https://github.com/LHagfoss/rustcode/pull/448) / [`447128e`](https://github.com/LHagfoss/rustcode/commit/447128e))
- Improve ManageTask and TaskDone formatting in chat ([#447](https://github.com/LHagfoss/rustcode/pull/447) / [`0355036`](https://github.com/LHagfoss/rustcode/commit/0355036))

### Refactor
- Make tool registration a single self-contained definition ([`c66d431`](https://github.com/LHagfoss/rustcode/commit/c66d431))

### Chores
- Replace header.png with new image ([#449](https://github.com/LHagfoss/rustcode/pull/449) / [`6dee0f9`](https://github.com/LHagfoss/rustcode/commit/6dee0f9))

## [v0.13.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.13.1) - 2026-08-07

### Fixes
- **UI:** Truncate long tool call args to prevent line wrapping ([#443](https://github.com/LHagfoss/rustcode/pull/443) / [`a012730`](https://github.com/LHagfoss/rustcode/commit/a012730))
- **Network:** Remove wall-clock timeout from turn budget ([#444](https://github.com/LHagfoss/rustcode/pull/444) / [`8047214`](https://github.com/LHagfoss/rustcode/commit/8047214))

### UI
- Move queue indicator above input with preview and edit hint ([#445](https://github.com/LHagfoss/rustcode/pull/445) / [`6d40e37`](https://github.com/LHagfoss/rustcode/commit/6d40e37))

## [v0.13.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.13.0) - 2026-08-06

### Features
- **Context:** Make compaction budget dynamic per model ([#418](https://github.com/LHagfoss/rustcode/pull/418) / [`dfc6b90`](https://github.com/LHagfoss/rustcode/commit/dfc6b90))
- **Context:** Add compact repo-map fragment ([#419](https://github.com/LHagfoss/rustcode/pull/419) / [`0581c8d`](https://github.com/LHagfoss/rustcode/commit/0581c8d))
- **Context:** Tiered compaction — skip LLM summary for local Ollama ([#420](https://github.com/LHagfoss/rustcode/pull/420) / [`92daaba`](https://github.com/LHagfoss/rustcode/commit/92daaba))
- **Context:** Document file-cache-diff replay ([#421](https://github.com/LHagfoss/rustcode/pull/421) / [`aef1eae`](https://github.com/LHagfoss/rustcode/commit/aef1eae))
- **Context:** Cap skill content to 12k chars ([#422](https://github.com/LHagfoss/rustcode/pull/422) / [`2bfa580`](https://github.com/LHagfoss/rustcode/commit/2bfa580))
- **Context:** Add compaction reclaim metrics ([#423](https://github.com/LHagfoss/rustcode/pull/423) / [`9690c49`](https://github.com/LHagfoss/rustcode/commit/9690c49))
- **Agent:** Preserve structured tool calls end-to-end via envelope ([#424](https://github.com/LHagfoss/rustcode/pull/424) / [`b2f8888`](https://github.com/LHagfoss/rustcode/commit/b2f8888))
- **UI:** Render markdown tables with styled column dividers ([#417](https://github.com/LHagfoss/rustcode/pull/417) / [`b9524df`](https://github.com/LHagfoss/rustcode/commit/b9524df))

### Fixes
- **UI:** Complete overhaul of markdown table rendering — aligned columns, outer borders, horizontal dividers, cell padding, right border alignment, and content wrapping ([#425](https://github.com/LHagfoss/rustcode/pull/425)–[#435](https://github.com/LHagfoss/rustcode/pull/435) / [`523f33a`](https://github.com/LHagfoss/rustcode/commit/523f33a))
- **UI:** Restore themed markdown renderer, remove `tui-markdown` dependency ([`84d185b`](https://github.com/LHagfoss/rustcode/commit/84d185b))
- **UI:** Request final redraw on turn completion and collapse thinking by default during generation ([`2730a6c`](https://github.com/LHagfoss/rustcode/commit/2730a6c))
- **Agent:** Classify tool failures and require grounded recovery ([#426](https://github.com/LHagfoss/rustcode/pull/426) / [`fe859ba`](https://github.com/LHagfoss/rustcode/commit/fe859ba))
- **Network:** Increase turn token budget limit from 500k to 5M tokens ([`83493ac`](https://github.com/LHagfoss/rustcode/commit/83493ac))
- **Network:** Remove max tool rounds limit in harness ([`eb476ef`](https://github.com/LHagfoss/rustcode/commit/eb476ef))

### Performance
- Optimize dependencies in debug builds for scroll performance ([`2bac090`](https://github.com/LHagfoss/rustcode/commit/2bac090))

## [v0.12.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.12.1) - 2026-08-06
- feat(sync): add default .gitignore to exclude logs, backups, and binaries from sync repo ([#415](https://github.com/LHagfoss/rustcode/pull/415) / [`2febdd0`](https://github.com/LHagfoss/rustcode/commit/2febdd0))

## [v0.12.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.12.0) - 2026-08-06
- Fix config.toml corruption and cursor alignment ([#414](https://github.com/LHagfoss/rustcode/pull/414) / [`5a7a7f7`](https://github.com/LHagfoss/rustcode/commit/5a7a7f7))
- Stage config directory files with git add -A ([#413](https://github.com/LHagfoss/rustcode/pull/413) / [`e90ce3b`](https://github.com/LHagfoss/rustcode/commit/e90ce3b))
- Show session duration in goodbye message ([#412](https://github.com/LHagfoss/rustcode/pull/412) / [`a700af5`](https://github.com/LHagfoss/rustcode/commit/a700af5))
- Improve tool argument matching for parallel responses ([#411](https://github.com/LHagfoss/rustcode/pull/411) / [`bdcb85a`](https://github.com/LHagfoss/rustcode/commit/bdcb85a))
- Single escape key cancels streaming or clears input ([#410](https://github.com/LHagfoss/rustcode/pull/410) / [`2e70976`](https://github.com/LHagfoss/rustcode/commit/2e70976))
- Add goodbye box on exit ([`6573270`](https://github.com/LHagfoss/rustcode/commit/6573270))
- Improve UI cursor style and chat input placeholder ([#404](https://github.com/LHagfoss/rustcode/pull/404) / [`5241bcf`](https://github.com/LHagfoss/rustcode/commit/5241bcf))
- Strip harness verification footer from TUI rendering ([#403](https://github.com/LHagfoss/rustcode/pull/403) / [`8dec836`](https://github.com/LHagfoss/rustcode/commit/8dec836))
- Add default Socraticode MCP server configuration ([`08b86d5`](https://github.com/LHagfoss/rustcode/commit/08b86d5))
- Improve code block copy feedback and selection ([#400](https://github.com/LHagfoss/rustcode/pull/400) / [`28559fe`](https://github.com/LHagfoss/rustcode/commit/28559fe))

## [v0.11.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.11.3) - 2026-08-05
### Features
- **UI:** Redesign /verbosity picker modal with modern theme card styling and descriptions ([#397](https://github.com/LHagfoss/rustcode/pull/397) / [`93d0942`](https://github.com/LHagfoss/rustcode/commit/93d0942))

### Fixes
- **Update:** Keep session active, enable 60Hz live redraw during update, and run brew update first ([#398](https://github.com/LHagfoss/rustcode/pull/398) / [`9fd283c`](https://github.com/LHagfoss/rustcode/commit/9fd283c))
- **UI:** Hide msg.diff in high verbosity and remove background block tinting behind diff text ([#396](https://github.com/LHagfoss/rustcode/pull/396) / [`f216380`](https://github.com/LHagfoss/rustcode/commit/f216380))

## [v0.11.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.11.2) - 2026-08-05
### Features
- **UI:** Format /help and /quota like /status with title headers, grouped sections, and bold commands ([#392](https://github.com/LHagfoss/rustcode/pull/392) / [`605563e`](https://github.com/LHagfoss/rustcode/commit/605563e))
- **UI:** Add top and bottom vertical padding for assistant text and markdown blocks ([#385](https://github.com/LHagfoss/rustcode/pull/385) / [`7162eac`](https://github.com/LHagfoss/rustcode/commit/7162eac))
- **UI:** Remove Resumed session and Request cancelled system messages from chat transcript and show as transient toasts ([#383](https://github.com/LHagfoss/rustcode/pull/383) / [`2eab924`](https://github.com/LHagfoss/rustcode/commit/2eab924))
- **UI:** Convert /info and /about into structured notice blocks ([#381](https://github.com/LHagfoss/rustcode/pull/381) / [`d6c45f1`](https://github.com/LHagfoss/rustcode/commit/d6c45f1))
- **UI:** Remove background tint boxes from diffs and render full-line green/red text for added/removed diff lines ([#380](https://github.com/LHagfoss/rustcode/pull/380) / [`361410f`](https://github.com/LHagfoss/rustcode/commit/361410f))

### Fixes
- **Tools:** Prevent manage_task polling loops on background tasks ([#395](https://github.com/LHagfoss/rustcode/pull/395) / [`8e795c3`](https://github.com/LHagfoss/rustcode/commit/8e795c3))
- **UI:** Decouple /status from /quota and add vertical padding lines between notice blocks ([#393](https://github.com/LHagfoss/rustcode/pull/393) / [`89066f2`](https://github.com/LHagfoss/rustcode/commit/89066f2))
- **UI:** Use exact display width of Copy badge for hover tinting and click detection ([#391](https://github.com/LHagfoss/rustcode/pull/391) / [`147b532`](https://github.com/LHagfoss/rustcode/commit/147b532))
- **UI:** Clean up unused import in ui tests ([#390](https://github.com/LHagfoss/rustcode/pull/390) / [`cd61e67`](https://github.com/LHagfoss/rustcode/commit/cd61e67))
- **UI:** Restrict Copy badge hover background tinting to button columns ([#389](https://github.com/LHagfoss/rustcode/pull/389) / [`95ab4a4`](https://github.com/LHagfoss/rustcode/commit/95ab4a4))
- **UI:** Restrict CopyBadge hover target to the top-right copy button area ([#388](https://github.com/LHagfoss/rustcode/pull/388) / [`ba53799`](https://github.com/LHagfoss/rustcode/commit/ba53799))
- **UI:** Remove dark bar under code blocks, remove background on Copy badge, and restrict Copy click target to button area ([#387](https://github.com/LHagfoss/rustcode/pull/387) / [`e3a8b1a`](https://github.com/LHagfoss/rustcode/commit/e3a8b1a))
- **UI:** Remove extra space between tool action label and argument parentheses ([#386](https://github.com/LHagfoss/rustcode/pull/386) / [`13b0a97`](https://github.com/LHagfoss/rustcode/commit/13b0a97))
- **UI:** Remove double-dots from system info rendering and format with clean whitespace padding ([#384](https://github.com/LHagfoss/rustcode/pull/384) / [`2b53e6f`](https://github.com/LHagfoss/rustcode/commit/2b53e6f))
- **UI:** Remove background colors from text/code/diffs, clean up ManageTask, and fix ask_question hangs ([#382](https://github.com/LHagfoss/rustcode/pull/382) / [`c81b20f`](https://github.com/LHagfoss/rustcode/commit/c81b20f))

### Chores
- Update Cargo.lock ([#394](https://github.com/LHagfoss/rustcode/pull/394) / [`95a6d13`](https://github.com/LHagfoss/rustcode/commit/95a6d13))

## [v0.11.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.11.1) - 2026-08-04
### Features
- **UI:** Add >_ RustCode (vX.X.X) notice header, animated /update flow, and automatic UI redraw on notice toast changes ([#375](https://github.com/LHagfoss/rustcode/pull/375) / [`41b5678`](https://github.com/LHagfoss/rustcode/commit/41b5678))
- **UI:** Display formatted tool call parameters and scope >_ RustCode notice headers exclusively to system info commands ([#377](https://github.com/LHagfoss/rustcode/pull/377) / [`e458f56`](https://github.com/LHagfoss/rustcode/commit/e458f56))
- Completely remove Discord RPC module, config settings, and dependencies ([#378](https://github.com/LHagfoss/rustcode/pull/378) / [`0859e04`](https://github.com/LHagfoss/rustcode/commit/0859e04))

### Fixes
- Clean up Cargo.lock ([#379](https://github.com/LHagfoss/rustcode/pull/379) / [`e9bd5b0`](https://github.com/LHagfoss/rustcode/commit/e9bd5b0))
- **Test:** Isolate theme unit test to prevent overwriting user config with nord ([#376](https://github.com/LHagfoss/rustcode/pull/376) / [`faceb13`](https://github.com/LHagfoss/rustcode/commit/faceb13))

## [v0.11.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.11.0) - 2026-08-04
### Features
- **UI:** Render minimal inline system messages in chat and show toast popup on session resume/notices ([#373](https://github.com/LHagfoss/rustcode/pull/373) / [`b63fac8`](https://github.com/LHagfoss/rustcode/commit/b63fac8))
- **UI:** Add verbosity picker modal and key handling ([#371](https://github.com/LHagfoss/rustcode/pull/371) / [`3e9b1d7`](https://github.com/LHagfoss/rustcode/commit/3e9b1d7))
- **Skills:** Refactor skill discovery to use lightweight SkillMetadata for dynamic lazy loading ([#365](https://github.com/LHagfoss/rustcode/pull/365) / [`a51f029`](https://github.com/LHagfoss/rustcode/commit/a51f029))
- **UI:** Load themes from `~/.config/rustcode/themes/*.toml` and open interactive modal on `/theme` ([#360](https://github.com/LHagfoss/rustcode/pull/360) / [`3a14bf3`](https://github.com/LHagfoss/rustcode/commit/3a14bf3))
- **UI:** Add interactive theme picker modal with live theme preview ([#359](https://github.com/LHagfoss/rustcode/pull/359) / [`dc5dd29`](https://github.com/LHagfoss/rustcode/commit/dc5dd29))
- **UI:** Add `/theme` slash command for UI color theme selection ([#358](https://github.com/LHagfoss/rustcode/pull/358) / [`fdf2474`](https://github.com/LHagfoss/rustcode/commit/fdf2474))
- **Tools:** Integrate ripgrep for grep search with native fallback ([#355](https://github.com/LHagfoss/rustcode/pull/355) / [`8d0832e`](https://github.com/LHagfoss/rustcode/commit/8d0832e))

### Fixes
- **UI:** Eliminate inconsistent padding around tool calls in conversation view ([#374](https://github.com/LHagfoss/rustcode/pull/374) / [`c49ab31`](https://github.com/LHagfoss/rustcode/commit/c49ab31))
- **UI:** Invalidate markdown and tool result caches when active theme changes ([#372](https://github.com/LHagfoss/rustcode/pull/372) / [`4b10975`](https://github.com/LHagfoss/rustcode/commit/4b10975))
- Make loop detection visible and specific to the model ([`81ebf57`](https://github.com/LHagfoss/rustcode/commit/81ebf57))
- **Network:** Make context compaction bounded with 25s timeout and cancellation awareness (closes #320) ([#366](https://github.com/LHagfoss/rustcode/pull/366) / [`598883a`](https://github.com/LHagfoss/rustcode/commit/598883a))
- **UI:** Fix weird background blocks when switching themes and make diffs and headings theme-aware ([#364](https://github.com/LHagfoss/rustcode/pull/364) / [`b430e3b`](https://github.com/LHagfoss/rustcode/commit/b430e3b))
- **Harness:** Exclude user interactive wait time from turn wall-clock safety budget ([#363](https://github.com/LHagfoss/rustcode/pull/363) / [`ddbd406`](https://github.com/LHagfoss/rustcode/commit/ddbd406))
- **UI:** Redesign top-right notice toast with left accent bar and route picker changes to notice toast ([#362](https://github.com/LHagfoss/rustcode/pull/362) / [`edd0307`](https://github.com/LHagfoss/rustcode/commit/edd0307))
- **UI:** Apply live theme palette dynamically across all TUI elements and fix modal active badge ([#361](https://github.com/LHagfoss/rustcode/commit/ea9115a))
- **Harness:** Improve tool error diagnostics and deduplicate loop warnings ([#357](https://github.com/LHagfoss/rustcode/pull/357) / [`b8da123`](https://github.com/LHagfoss/rustcode/commit/b8da123))
- **Harness:** Improve tool error diagnostics and deduplicate loop warnings ([#356](https://github.com/LHagfoss/rustcode/commit/a4ae04c))

### Chores
- Update .gitignore ([#368](https://github.com/LHagfoss/rustcode/pull/368) / [`cb6db91`](https://github.com/LHagfoss/rustcode/commit/cb6db91))
- Format files ([#367](https://github.com/LHagfoss/rustcode/pull/367) / [`2c26242`](https://github.com/LHagfoss/rustcode/commit/2c26242))
- Fix changelog version tag ([#354](https://github.com/LHagfoss/rustcode/pull/354) / [`470cdb9`](https://github.com/LHagfoss/rustcode/commit/470cdb9))

## [v0.10.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.10.0) - 2026-08-03
- Auto-wake orchestrator on background task completion in active sessions ([#352](https://github.com/LHagfoss/rustcode/pull/352) / [`2185208`](https://github.com/LHagfoss/rustcode/commit/2185208))
- Append non-polling guidance to manage_task description and outputs ([#351](https://github.com/LHagfoss/rustcode/pull/351) / [`e5b0f34`](https://github.com/LHagfoss/rustcode/commit/e5b0f34))
- Enforce high verbosity across all tools and allow manage_task output in transcript ([#350](https://github.com/LHagfoss/rustcode/pull/350) / [`0871646`](https://github.com/LHagfoss/rustcode/commit/0871646))
- Add /verbosity command for tool output detail ([#334](https://github.com/LHagfoss/rustcode/pull/334) / [`adf903e`](https://github.com/LHagfoss/rustcode/commit/adf903e))
- Persist verbosity setting in config.toml ([`49645a5`](https://github.com/LHagfoss/rustcode/commit/49645a5))
- Arrow-up pulls queued prompts back into the input ([`121b29c`](https://github.com/LHagfoss/rustcode/commit/121b29c))
- Various UI improvements for tool call rendering, status indicators, and input history navigation.

## [v0.9.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.9.0)
- **Network & Agent:** Improved Gemini protocol handling, added thought signatures to structured calls, and normalized file edit signatures to prevent loop churn ([#330](https://github.com/LHagfoss/rustcode/pull/330), [#329](https://github.com/LHagfoss/rustcode/pull/329), [#324](https://github.com/LHagfoss/rustcode/pull/324)).
- **UI/UX:** Added circle status indicators, improved footer pulse animations, and optimized transcript rendering for large histories ([#323](https://github.com/LHagfoss/rustcode/pull/323), [#326](https://github.com/LHagfoss/rustcode/pull/326), [#322](https://github.com/LHagfoss/rustcode/pull/322)).
- **Discord RPC:** Implemented and verified robust Discord Rich Presence behavior ([#298](https://github.com/LHagfoss/rustcode/pull/298)).
- **Harness & Safety:** Enforced bounded tool context, added multi-signal safety budgets, and improved verification of edit diffs ([#321](https://github.com/LHagfoss/rustcode/pull/321), [#307](https://github.com/LHagfoss/rustcode/pull/307), [#315](https://github.com/LHagfoss/rustcode/pull/315)).
- **Tooling:** Improved edit argument alias support and made `replace_file_content` idempotent ([#317](https://github.com/LHagfoss/rustcode/pull/317), [#306](https://github.com/LHagfoss/rustcode/pull/306)).

## [v0.8.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.8.0) - 2026-07-31

### Features

### Fixes

### Refactor

### Documentation

### Chore


## [v0.7.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.7.2) - 2026-07-31
### Features
- Check for updates on launch and via --upgrade ([#232](https://github.com/LHagfoss/rustcode/pull/232) / [`b428952`](https://github.com/LHagfoss/rustcode/commit/b428952))

### Refactor
- Unify raw_cli loop with orchestrator ([#166](https://github.com/LHagfoss/rustcode/pull/166) / [`0ce1180`](https://github.com/LHagfoss/rustcode/commit/0ce1180))
- Enforce turn state machine transitions ([#168](https://github.com/LHagfoss/rustcode/pull/168) / [`a9bac22`](https://github.com/LHagfoss/rustcode/commit/a9bac22))

## [v0.7.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.7.1) - 2026-07-31
### Fixes
- Add spacing after tool results ([#231](https://github.com/LHagfoss/rustcode/pull/231) / [`f68f33b`](https://github.com/LHagfoss/rustcode/commit/f68f33b))
- Add top padding to status panels ([#230](https://github.com/LHagfoss/rustcode/pull/230) / [`ee81190`](https://github.com/LHagfoss/rustcode/commit/ee81190))
- Cap ordinary tool output and preserve diffs ([#229](https://github.com/LHagfoss/rustcode/pull/229) / [`6c7c0ac`](https://github.com/LHagfoss/rustcode/commit/6c7c0ac))
- Hide successful exit status ([#228](https://github.com/LHagfoss/rustcode/pull/228) / [`f2a3c13`](https://github.com/LHagfoss/rustcode/commit/f2a3c13))
- Render generic tool results quietly ([#227](https://github.com/LHagfoss/rustcode/pull/227) / [`dcebd8c`](https://github.com/LHagfoss/rustcode/commit/dcebd8c))

## [v0.6.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.6.0) - 2026-07-29
### Features
- Add `/update` to check the Homebrew tap and upgrade rustcode when a newer version exists ([#96](https://github.com/LHagfoss/rustcode/pull/96) / [`8aa4aae`](https://github.com/LHagfoss/rustcode/commit/8aa4aae))
- Output `/changelog` as an assistant message ([#95](https://github.com/LHagfoss/rustcode/pull/95) / [`7117bf9`](https://github.com/LHagfoss/rustcode/commit/7117bf9))

### Fixes
- Correct native tool schemas for array params and drop the football tool ([#94](https://github.com/LHagfoss/rustcode/pull/94) / [`b7af589`](https://github.com/LHagfoss/rustcode/commit/b7af589))
- Render side-by-side diffs full-width with syntax highlighting ([#93](https://github.com/LHagfoss/rustcode/pull/93) / [`f861a45`](https://github.com/LHagfoss/rustcode/commit/f861a45))

## [v0.5.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.5.0) - 2026-07-29
### Features
- Add transient notice toast in the top-right corner ([#92](https://github.com/LHagfoss/rustcode/pull/92) / [`76f9c7f`](https://github.com/LHagfoss/rustcode/commit/76f9c7f))
- Render custom tool calls in PascalCase with a parameter ([#91](https://github.com/LHagfoss/rustcode/pull/91) / [`e7f5b87`](https://github.com/LHagfoss/rustcode/commit/e7f5b87))
- Enable mouse selection in the input box ([#90](https://github.com/LHagfoss/rustcode/pull/90) / [`446bf54`](https://github.com/LHagfoss/rustcode/commit/446bf54))
- Implement robust edit matching and specific tool-call JSON errors ([#88](https://github.com/LHagfoss/rustcode/pull/88) / [`41f80d1`](https://github.com/LHagfoss/rustcode/commit/41f80d1))
- Add padding to spinner and more random words to UI loading messages ([#87](https://github.com/LHagfoss/rustcode/pull/87) / [`2f9a878`](https://github.com/LHagfoss/rustcode/commit/2f9a878))

### Fixes
- Allow selecting the first two columns of chat content ([#89](https://github.com/LHagfoss/rustcode/pull/89) / [`8b3a7c6`](https://github.com/LHagfoss/rustcode/commit/8b3a7c6))
- Remove unnecessary log messages ([#86](https://github.com/LHagfoss/rustcode/pull/86) / [`244523b`](https://github.com/LHagfoss/rustcode/commit/244523b))

## [v0.4.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.4.0) - 2026-07-29
### Features
- Adjust selection colors for better contrast ([#85](https://github.com/LHagfoss/rustcode/pull/85) / [`89cc364`](https://github.com/LHagfoss/rustcode/commit/89cc364))
- Update image paths and move images to new directory ([#81](https://github.com/LHagfoss/rustcode/pull/81) / [`6b5a50a`](https://github.com/LHagfoss/rustcode/commit/6b5a50a))
- Implement auto-recap feature and recap for model streams ([#80](https://github.com/LHagfoss/rustcode/pull/80) / [`ac9a9be`](https://github.com/LHagfoss/rustcode/commit/ac9a9be), [`b522245`](https://github.com/LHagfoss/rustcode/commit/b522245))
- One-line streaming status without model/build and split streaming status onto two lines with orange status word ([#79](https://github.com/LHagfoss/rustcode/pull/79), [#76](https://github.com/LHagfoss/rustcode/pull/76) / [`1c3e5de`](https://github.com/LHagfoss/rustcode/commit/1c3e5de), [`247037f`](https://github.com/LHagfoss/rustcode/commit/247037f))
- Implement beautiful markdown prose rendering with styled headers, clean links, and bullet points ([#73](https://github.com/LHagfoss/rustcode/pull/73) / [`9bee474`](https://github.com/LHagfoss/rustcode/commit/9bee474))

### Fixes
- Stop loop detector from killing legitimate recovery ([#75](https://github.com/LHagfoss/rustcode/pull/75) / [`3dadbfd`](https://github.com/LHagfoss/rustcode/commit/3dadbfd))
- Render code blocks as a solid full-width panel with aligned copy button ([#74](https://github.com/LHagfoss/rustcode/pull/74) / [`c69fc07`](https://github.com/LHagfoss/rustcode/commit/c69fc07))
- Clean up assistant markdown prose background and add padding around system notices ([#72](https://github.com/LHagfoss/rustcode/pull/72) / [`a1971ea`](https://github.com/LHagfoss/rustcode/commit/a1971ea))
- Restrict diff syntax highlighting to diff blocks, fix single-line diffs, and add top padding to assistant responses ([#71](https://github.com/LHagfoss/rustcode/pull/71) / [`4054611`](https://github.com/LHagfoss/rustcode/commit/4054611))

### Documentation
- Improve README with new sections and formatting ([#84](https://github.com/LHagfoss/rustcode/pull/84), [#83](https://github.com/LHagfoss/rustcode/pull/83) / [`bad90b2`](https://github.com/LHagfoss/rustcode/commit/bad90b2), [`4428465`](https://github.com/LHagfoss/rustcode/commit/4428465))
- Changelog updates for streaming status, loop detector, and code panel fixes ([#76](https://github.com/LHagfoss/rustcode/pull/76), [#75](https://github.com/LHagfoss/rustcode/pull/75), [#74](https://github.com/LHagfoss/rustcode/pull/74) / [`b81b699`](https://github.com/LHagfoss/rustcode/commit/b81b699), [`c76647d`](https://github.com/LHagfoss/rustcode/commit/c76647d), [`311cc68`](https://github.com/LHagfoss/rustcode/commit/311cc68))

### Chore
- Remove clewdr proxy files and ignore them ([#77](https://github.com/LHagfoss/rustcode/pull/77) / [`e480761`](https://github.com/LHagfoss/rustcode/commit/e480761))



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
## [v0.7.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.7.0) - 2026-07-31
### Features
- Refactor the agent orchestration, typed turn lifecycle, continuation policy, context handling, and subagent workspace boundaries ([`d7d7229`](https://github.com/LHagfoss/rustcode/commit/d7d7229), [`4310340`](https://github.com/LHagfoss/rustcode/commit/4310340), [`3e549f7`](https://github.com/LHagfoss/rustcode/commit/3e549f7))
- Add structured tool-result metadata, safer tool validation, explicit delegation contracts, and project-aware verification ([`14637e9`](https://github.com/LHagfoss/rustcode/commit/14637e9), [`1083dd6`](https://github.com/LHagfoss/rustcode/commit/1083dd6), [`1cc96b1`](https://github.com/LHagfoss/rustcode/commit/1cc96b1), [`e1b79ee`](https://github.com/LHagfoss/rustcode/commit/e1b79ee))
- Add structured, syntax-highlighted tool output and improved assistant code/diff rendering ([`3c923a8`](https://github.com/LHagfoss/rustcode/commit/3c923a8), [`b9f227a`](https://github.com/LHagfoss/rustcode/commit/b9f227a), [`cb61915`](https://github.com/LHagfoss/rustcode/commit/cb61915), [`bb47b05`](https://github.com/LHagfoss/rustcode/commit/bb47b05))

### Fixes
- Prevent harmless repeated Git inspection commands from disabling agent tools ([#223](https://github.com/LHagfoss/rustcode/pull/223))
- Repair project-root discovery so automatic Cargo verification receives a real absolute working directory ([#221](https://github.com/LHagfoss/rustcode/pull/221))
- Improve code panels, Edit/Delete diffs, tool output spacing, line gutters, and assistant transcript readability ([#218](https://github.com/LHagfoss/rustcode/pull/218), [#219](https://github.com/LHagfoss/rustcode/pull/219), [#220](https://github.com/LHagfoss/rustcode/pull/220))
- Bound loop detection, soften legitimate read recovery, and remove the obsolete continuation round cap ([`676e3d2`](https://github.com/LHagfoss/rustcode/commit/676e3d2), [`29f0ced`](https://github.com/LHagfoss/rustcode/commit/29f0ced))

### Documentation
- Strengthen the Git feature workflow and shell command-chaining guidance for agentic coding ([#222](https://github.com/LHagfoss/rustcode/pull/222))
