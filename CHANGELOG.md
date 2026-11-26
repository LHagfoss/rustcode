## [v1.54.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.54.0) - 2026-11-26

### Features
- Jobs shows placeholder job cards while it loads. (#4332)
- The sidebar lights only the page you are on: opening Dashboard no longer leaves the last thread highlighted, and New thread is highlighted while you start one. (#4334)
- Chat recaps now show up in every chat that has been quiet for a while, have a cleaner card, and close on their own when a new message arrives. (#4335)
- Statistics shows tooltips on its tiles, trends, cost bar and table, and hovering a sparkline names each day. (#4336)
- The command palette shows at most three settings or actions above matching threads, so threads stay in view while searching. (#4337)
- Fixed the hub switch flipping back for a few seconds after a change, and the issue report showing an earlier submit failure when opened again. (#4339)
- The dashboard loads with skeletons shaped like its cards, trims the activity heatmap to your history, and shows why pull requests could not be read. (#4333)
- Needs you shows the command an approval would run with its risk, keeps destructive calls off one-click Allow, and counts threads the same way in the sidebar, dashboard and inbox. (#4340)
- A later approval's buttons no longer show keys that answer a different approval. (#4341)
- Sidebar thread titles are no longer cut short by the Approval/Review status; the status now shows on the row's second line. (#4342)
- The dashboard no longer replays its entrance animation each time you open it. (#4343)
- The dashboard's Models card shows model names and counts each model once, however its turns were recorded. (#4344)
- The command palette lists Settings pages only when you search and heads the results above the threads "Best matches" instead of a second "Actions". (#4345)
- The thread header names a worktree thread's project instead of its worktree folder. (#4346)
- The + menu's tiles read "Send later", "New thread" and "In folder". (#4347)
- Settings: idle-thread settling and toast position move to General, General shows the new-thread harness and model, and usage bars show only what is left. (#4348)
- Statistics and dashboard days now follow your local clock instead of UTC. (#4349)
- Jobs whose harness is turned off now offer a Turn on button that enables it and resumes the job. (#4350)
- Agents that compact and continue no longer stop when a wake arrives during their compaction. (#4351)
- Customize on the dashboard now shows removed cards as a gallery you can add back from, without scrolling to find it. (#4352)
- The Turns panel names a turn the app continued by what it did, with "After compaction" beside its time and cost. (#4353)
- Needs you can approve every waiting read-only call across threads in one click. (#4354)
- Fix the Trace chip floating after an empty gap under older turns. (#4357)
- Statistics' activity grid shows bigger, squarer day cells without outlines. (#4358)
- Vercel connects from Settings › Connections, and a failed connection shows why, with Retry and Remove. (#4359)
- Stack dashboard cards so they take turns, and show model and harness icons on the dashboard. (#4361)
- The thread panel shows the thread's total cost, time worked and tokens at a glance, and its context meter warns as the window fills. (#4360)
- New files an agent creates now show their contents in Review and the Changes panel, and can be commented on. (#4415)
- Review keeps your draft comments when you leave the screen or reload, and never sends one source's comments as another's. (#4418)
- Resuming a workflow run from the app now starts its workers with approval cards instead of failing for threads below Full access. (#4413)
- Fixed: on the built-in harness, browser screenshots reach the model as an image instead of base64 text. (#4414)
- Review shows a pull request's added, deleted, renamed and binary files as such, matching the pull request's Diff tab. (#4420)
- Removing a machine now also ends this computer's access on that machine when it can be reached, and says so when it cannot. (#4419)
- Review: mark files viewed, see your progress, and step through files with J, K and V. A file the agent changes again is flagged. (#4423)
- Settings and `rustcode doctor` now say when config.toml could not be read, with the line to fix, instead of silently using defaults. (#4416)
- Workflow runs cut off by a restart now continue on their own when the engine starts again, without opening their thread (Full-access threads). (#4421)
- Fixed: browser tools on the built-in harness ask before every action even with auto-approve on, and Plan mode no longer lets them click or type. (#4422)
- A full disk or a crash can no longer leave a thread's history, status or settings file half-written, and a history that stops saving is now logged. (#4417)
- `rustcode doctor` reports each harness's install, version and sign-in, and each named account's sign-in. (#4424)
- Review: Shift-click a second line number to comment on a range of lines; the agent and pull request reviews get the whole range. (#4425)
- Fixed: a thread's headless browser closes after 10 idle minutes or when the thread is deleted, and browser profiles left by a crash are cleaned up. (#4427)
- Revoke a lost computer on every machine at once: Settings › Machines › Revoke everywhere, or `rc machines revoke-everywhere`. (#4426)
- Review a worktree branch one commit at a time, with merges marked, instead of only the whole branch at once. (#4431)
- Smart UI review findings get a Fix chip each and a Fix all open chip, so a finding can be sent back to the agent in one click. (#4433)
- A thread waiting on you is called "Needs you" everywhere, and the recap says when you were last there instead of a misleading "Last activity". (#4437)
- Fixed: the browser tools' domain allowlist now also covers WebSockets, workers, iframes and WebRTC, through a local proxy. (#4435)
- Workflow runs that failed, timed out or were stopped can be retried from their card; finished workers are kept and only the failed ones run again. (#4428)
- Rate-limited requests on the built-in harness wait as long as the provider asks before retrying. (#4430)
- Stopping a workflow or a delegate now stops its worker threads even when the RustCode app is not open. (#4438)
- Job webhooks can be limited to chosen GitHub events, tell the agent and the pre-check script which event arrived, ignore redeliveries, and answer oversized requests with 413. (#4439)
- Browser tools now list a page's interactive elements with refs, click with real mouse input, wait for pages to load, and mark page content as untrusted. (#4441)
- A thread on Claude Code, Codex or another external harness keeps what a running turn did when the app or engine stops mid-turn. (#4432)
- Workflow scripts can start workers from a saved delegate preset and with a lower access, account, project or worktree choice. (#4440)
- A computer whose clock is ahead can no longer lock a machine's name; renaming always takes effect. (#4436)
- Threads with no RustCode app open are now told when their delegates or workflow runs finish. (#4445)
- Saved project memory is shared by all worktrees of a repository instead of being lost between task worktrees. (#4434)
- Harness processes left behind by an engine crash are now stopped the next time the engine starts. (#4446)
- Usage and activity writes to the local data store are faster. (#4447)
- Threads on a Claude or ChatGPT subscription now show "API value" instead of "Cost", since the plan is not billed per token. (#4456)
- Switching a long thread to another harness or account keeps the original request and the latest summary. (#4457)
- The hub now shows whether each queued call ran or was refused, never delivers one twice, and reports when a call could not be queued. (#4458)
- Allow a site for a thread so browser reads and screenshots there stop asking, and stop the browser from the thread header's Browser chip. (#4459)
- External-harness threads (Claude Code, Codex, Gemini CLI, OpenCode, Pi) can now save, recall and forget project memory through the rustcode MCP server, shared with built-in threads. (#4460)
- Settings › Agents shows saved project and global memory, with Forget and Forget all. (#4461)
- Threads on Claude Code, Codex or another harness can switch back to RustCode from the model picker and keep their conversation. (#4462)
- Workflow runs can be capped by tokens or reported USD, and every run shows what its workers spent. (#4463)
- list_machines shows each machine's load, free memory, battery state and running turns, so agents can place work on idle machines. (#4465)
- Paired computers can be limited to reading threads or running threads, instead of always getting full control of this machine. (#4464)
- Workflow scripts can stop and ask you with checkpoint(question, options), and the run card shows Needs you while it waits. (#4466)
- Jobs can run a saved workflow on a schedule or from a webhook, and the job run ends with the workflow run's status. (#4467)
- Middle-click a thread in the sidebar to settle it. (#4468)
- Paired devices unused for 90 days (configurable with [remote] idle_expiry_days) are refused until they pair again, and listed as expired. (#4469)
## [v1.53.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.53.0) - 2026-11-25

### Features
- The activity heatmap drops the streak outlines and badge glow for a cleaner look. (#4319)
- The chat recap is a compact row that fits the chat instead of a large card. (#4320)
- The Turns panel shows the time and cost of every turn after a restart or in an old thread, not only the latest ones. (#4321)
- Settings › Machines: the map stays inside its card, and pressing a machine on the map or in the list opens its settings. (#4323)
- Settings › Capabilities: MCP server rows highlight across the whole row on hover and press, and an opening row's details no longer draw over the rows below. (#4322)
- The thread info panel now uses the same glass background as the chat composer. (#4324)
- Statistics now opens with your lifetime tokens, peak day, longest turn, both streaks and a year-long activity grid. (#4325)
- Add a Dashboard screen to the sidebar with a customizable grid of activity, threads, spend and pull-request cards; the new-thread screen shows only the composer again. (#4326)
- Mark text in a reply and press Quote to ask about exactly that part. (#4329)
- Code blocks with long lines have a Wrap button that wraps them instead of scrolling sideways. (#4328)
- The usage card says when each plan window will run out at the current pace, or how full it will be at the reset. (#4327)
## [v1.52.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.52.0) - 2026-11-25

### Features
- Removed the thread Preview dialog, remote loopback preview relay, and public preview link sharing. (#4297)
- Keep a thread's turns listed in the right sidebar after restarting the app or reloading the thread. (#4298)
- Redesigned the Jobs overview dialog and cards, and fixed legacy or hub-dispatched scheduled job runs reappearing in the sidebar. (#4299)
- Show each turn's prompt in full width with its duration and price below, and price the turns after a reload. (#4300)
- The chat recap now appears inside the chat after the last turn instead of floating over the top of it. (#4302)
- Toasts are lighter, frosted like the composer, and can be moved to another corner in Settings › Appearance. (#4303)
- Markdown table toolbars and dragged header buttons get their shadow back. (#4305)
- Screenshots an agent saves in one of the project's git worktrees now show in its reply. (#4307)
- The new-thread overview's Activity card shows your day streak. (#4306)
- The Changes chip under a turn no longer counts a git pull, checkout or merge as the agent's edits, and Rewind keeps what it pulled in. (#4308)
- Paired devices see a thread's turn as running even when the desktop is not sharing that thread. (#4310)
- Settings › Remote access: a status banner at the top says whether and where a phone can reach this computer. (#4314)
- Settings: details, add forms and feature cards unfold smoothly, and the section highlight glides between pages. (#4311)
- Settings: Advanced is merged into Diagnostics, and Orchestration's second Delegation group is renamed Delegate limits. (#4312)
- Settings › Connections: sign in to GitHub and sign accounts out without a terminal; model accounts can refresh their models. (#4313)
- Agents now name a new thread with their first tool call, so fewer threads keep the raw first prompt as their title. (#4315)
- RustCode now describes itself as an agentic development environment (ADE); Settings › Orchestration is renamed Delegation. (#4316)
- Settings › Connections lists apps (Linear, Notion, Jira, Sentry, Vercel and more) to connect in one click with a browser sign-in. (#4317)
## [v1.51.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.51.0) - 2026-11-23

### Features
- Manage scheduled jobs across all paired machines directly without requiring a hub (#4266)
- Start a top-level thread when asked to open a new thread, and reserve delegation for explicit requests. (#4269)
- Clean up Antigravity and Gemini tool rows, thought previews, and expanded tool groups in the chat transcript. (#4267)
- Fix sidebar keeping the old folder group after moving a thread to another machine and changing its directory. (#4268)
- Fix settled threads waking on restart, stop cached remote threads from flashing at launch, and clean up Settings and popover button/badge hierarchy. (#4265)
- Keep a faint theme tint on the macOS sidebar at maximum transparency. (#4271)
- Give thread header buttons more room and show the device in the composer footer. (#4272)
- Open a running app privately from a paired RustCode desktop or share it with a temporary public link. (#4273)
- Keep thread properties visible while scrolling turns, and open individual or combined transcripts from the sidebar. (#4274)
- Open a job to see its run history, schedule, and outcome counts. (#4275)
- Make desktop thread settling immediate and prevent a stale refresh from bringing settled threads back. (#4277)
- Preview Markdown headings and continue lists automatically in the chat composer. (#4278)
- Show matching Settings entries and actions above threads in Cmd+K. (#4281)
- Fixed paired-machine harness listings to report the calling thread's harness and search models across every listed harness. (#4283)
- Composer Send, Queue and Stop tooltips open below the box instead of covering the draft. (#4285)
- The thread header's Preview button is now a pill, matching the other header buttons. (#4291)
- Hovering a job card's run dots shows how its recent runs went again. (#4292)
- Icon-only copy buttons in the transcript show what they copy on hover. (#4293)
- The delegate status card now opens on `/status`; a plain "status" goes to the model. (#4294)
- The command palette keeps the selected result in view while you arrow through it, and highlights it in the neutral fill. (#4295)
## [v1.50.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.50.0) - 2026-11-23

### Features
- Claude Code and Pi threads start again on computers whose npm cache was older than the adapter's dependencies, instead of failing with `ETARGET`. (#4255)
- Recaps are now per chat: back in a chat after a while, a card says what you asked and where it stands; Settings › General › Chat recaps turns them off. (#4256)
- Restarting the app no longer flashes old threads from paired machines in the sidebar. (#4257)
- Jobs: cards with each job's last result, one dialog to edit a job beside its runs, a day-grouped Review that folds repeated failures, and job runs no longer show up in the thread list. (#4258)
- The side panel is now one Thread info card that rises over the header, with a list of turns that opens a full event log of what the agent did. (#4259)
- Machines now pair both ways by default and can be allowed back with one click; the Map/List switch is rounded again. (#4260)
- Settings actions are easier to scan, with shorter labels, rounded selectors and clearer icons. (#4261)
- The job dialog folds the pre-check script again and explains why only this computer can run a job. (#4262)
- Smart UI tables and source lists read better, and every pasted image can be previewed and removed before sending. (#4263)
## [v1.49.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.49.0) - 2026-11-22

### Features
- Appearance: Transparency now applies to the sidebar alone; the composer, menus and dialogs keep their default glass, and the Blur slider is removed. (#4208)
- Settings opens in the sidebar and main window instead of a floating panel; the gear button opens and closes it. (#4209)
- The header's buttons with the sidebar hidden now match the sidebar's (toggle, search, Settings), and Settings pages fade in. (#4211)
- Fix missing blur behind menus, popovers and the composer in the web app outside Safari. (#4213)
- Bring the frosted glass (menus, popovers, composer, dialogs) back on Windows and on Linux where the web view composites. (#4214)
- Thread menu: the Copy Link quick action now reads Copy, so the tiles stay compact. (#4215)
- Thread menu: Mark for Review and Review Changes now sit under More, so the menu is shorter. (#4216)
- Claude Code and Pi threads start a pinned, tested adapter version instead of the newest npm release. (#4217)
- Delegate build slots no longer grow without bound: a slot over 40 GB starts over. (#4218)
- Smart UI forms show long choices as a readable list, keep commas inside parentheses in an option, and line up checkboxes. (#4219)
- The desktop app records each machine's connects and drops in `connections.log`, and a steer shows at once and waits in the queue if the machine is reconnecting. (#4224)
- The app now shares the RustCode server with the terminal and paired devices, so threads keep running after you close the app. (#4223)
- The RustCode server now runs scheduled jobs itself, so they fire from login without `rc daemon start`. (#4225)
- Fix ⌘F's find bar opening hidden behind the right panel. (#4226)
- Fix: an agent asking to compact and continue from a turn woken by a finished background task now compacts and carries on instead of stopping. (#4227)
- The side panel has softer pill tabs, bolder details and no close button; the header toggle closes it. (#4229)
- None (internal refactor). (#4233)
- None (internal refactor). (#4234)
- None (internal refactor). (#4235)
- Jobs page shows the last runs at a glance, moves Pause into the menu, and the job editor gains Weekly day chips and Save and run. (#4236)
- The chat folds runs of short automatic updates (merged PRs, finished background tasks) into one row and shows inline pull request links by number. (#4238)
- No user-facing change. (#4237)
- No user-facing change. (#4240)
- The chat reads calmer: tighter steps, roomier answers, softer rounding by role, and a quiet exit-code capsule on failed commands. (#4241)
- No user-facing change. (#4239)
- No user-facing change. (#4242)
- No user-facing change. (#4243)
- No user-facing change. (#4245)
- No user-facing change. (#4247)
- No user-facing change. (#4249)
- No user-facing change. (#4250)
- No user-facing change. (#4251)
- Toasts now swipe away to the right only, and a quick flick dismisses them. (#4248)
- The desktop app records the window's web process memory, CPU and restarts in `webview.log`, so a hot or bloated window leaves evidence. (#4253)
- The thread header's title opens the thread's menu, the side panel button names its tab, the Details panel is a full-height list of foldable sections, Settings' buttons match the rest of the app, and switches and segmented controls can be dragged. (#4252)
## [v1.48.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.48.0) - 2026-11-22

### Features
- The project picker spaces its rows like the machine picker. (#4195)
- Machines settings: tidier map, Hide for machines known through another, clearer hub explanation. (#4196)
- Opened sections in the chat ("Worked for", tool groups, tool calls, thoughts) keep their header pinned while you scroll through them. (#4198)
- Screenshots in replies fill the reply's width with rounded corners, and agents show a screenshot when asked to see something. (#4197)
- Smart UI views now draw shorthand blocks written after the view's fence, and stats written without the kit's classes stack value and label properly. (#4194)
- Threads running on another computer can now be renamed, by their agent and from the sidebar. (#4199)
- Settings › Usage merges the same Antigravity install across machines and no longer swaps live numbers for a paired machine's older ones. (#4200)
- Codex, OpenCode, Gemini, Pi and other non-Claude harnesses send about 6,500 fewer tokens per request: RustCode's thread tools are found on demand instead of being listed up front. (#4202)
- The composer is a shade lighter, so it stands out from the chat behind it. (#4205)
- A machine reached through your own `ssh -L` now switches to an SSH tunnel RustCode manages the first time that tunnel dies after a sleep, so it reconnects within seconds after later wakes. (#4204)
- Smart UI gets a component catalog: agents build views from blocks such as charts, progress, key-value rows, timelines, comparisons, heatmaps, tabs and forms, so views look consistent and arrive faster. (#4203)
- Toasts show Undo and Open as a tinted button with an icon, stack without the text behind showing through, and no longer draw a timer line. (#4201)
- Appearance: Transparency now goes up to 100%, and a new Blur slider sets how strongly the glass blurs. (#4206)
## [v1.47.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.47.0) - 2026-11-22

### Features
- Removed the built-in terminal panel (⌘J) and project quick actions, including actions that ran when a worktree was created or a thread settled. (#4161)
- Settings › Machines can update an outdated paired machine (Update, Update all), and a waiting app update now says which threads it waits for and offers Install now and resume threads. (#4167)
- Fix `gh` and GitHub tools failing with a certificate error when a dotfile sets a TLS variable from another variable or inside a condition. (#4165)
- A machine reached through your own `ssh -L` now says what to run when that tunnel dies after a sleep, and offers "Let RustCode manage this tunnel", which switches it to an SSH route that reconnects by itself after a wake. (#4163)
- The machine and project pickers are tidier. (#4169)
- Antigravity quotas show at once and refresh in the background. (#4170)
- Toasts now sit in the bottom-right corner, dismiss with a click, and leave sooner. (#4171)
- Agents and `rc threads configure` can now read and change a thread's model and effort on a headless server with no app attached. (#4172)
- Thread header keeps only the side panel toggle (and plan); git actions move to the Changes tab, the side panel shares one narrower width and the Details tab gets titled sections. (#4173)
- The macOS sidebar is now frosted glass over the desktop, and a new Transparency slider in Settings › Appearance sets how see-through the sidebar, composer and menus are. (#4174)
- The working line in chat says `Working for …` again, and it still stays up for as long as the engine runs the turn. (#4168)
- Fixed: a thread asking for a secret under Full access now shows the secret form instead of failing. (#4184)
- Added: a bug report from a thread can attach the thread's recent log, redacted, after you preview it. (#4187)
- Fixed: cleaning up a merged worktree no longer stops at Expo caches or Claude Code settings, or when its folder was already deleted. (#4186)
- Fixed: `merge_when_green` with a PR number works from a thread on another branch, such as a release thread on `main`. (#4185)
- Turns that finish without a reply now say what they did ("Read 1 file and edited 1 file") instead of only how long they worked. (#4188)
- Add automatic desktop crash reporting (on by default with first-run notice and opt-out in Settings › General). (#4189)
- Settings › Machines has a Map view showing how this computer reaches each machine, route by route. (#4190)
## [v1.46.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.46.1) - 2026-11-21

### Fixes
- Fixed `rc --update` on 1.45.0 and older, which could not find the new binary in the release archive. (#4154)
## [v1.46.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.46.0) - 2026-11-21

### Features
- Fleet: cross-machine orchestration, remote commands and fleet-wide defaults, and threads can move between paired machines. (#4136, #4135)
- Remote gateway serves its own TLS and pins its certificate. (#4141)
- Scheduled jobs run with the app closed, and Loops are renamed to Jobs with fixed scheduling and a redesigned page. (#4133, #4123)
- Workflow tools are exposed to outside agents. (#4132)
- Smart UI views, tables and diagrams are sharper, stay monochrome, and wrap paths cleanly. (#4142, #4148)
- Changes and Pull requests move into the side panel's tabs; the sidebar Threads heading gains filter and sort and keeps thread and project order. (#4108, #4118, #4128)
- Plan usage from paired machines merges by subscription. (#4117)
- Live ACP turn phase and activity age are shown, autonomous harness turns are tracked, and stalled first turns recover. (#4124, #4131, #4122)
- Remote threads can be steered, and the machine is shown while a thread starts. (#4127, #4116)
- `merge_when_green` can merge a numbered PR from a worktree. (#4150)
- Harnesses are found on the login shell's PATH first, zsh arrays in dotfiles no longer break the shell env, and pre-rc server services migrate to `rc`. (#4111, #4149, #4106)
- New worktrees install node_modules before checks. (#4137)
- Model names are formatted everywhere, the new-thread draft is kept, and harness settings are tidier. (#4147, #4129)
- Smoother selection across replies, Smart UI views and composer links; simpler edit icon; tooltips replace native titles. (#4112, #4121, #4113, #4109)
- Whole-file edit diffs count only what changed; forwarded ports get overlay timeouts after wake. (#4134, #4130)
- Outdated machines explain how to update and name `rc` as the CLI; Antigravity's Gemini cost is estimated at list prices. (#4110, #4107)
- Performance: flat ACP request dispatch, one shared zbus stack, a trimmed Shiki bundle, restored lazy view chunks, and release footprint measured in CI. (#4146, #4143, #4114, #4145, #4138)
## [v1.45.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.45.0) - 2026-11-20

### Features
- Show /compact in the slash-command menu for Codex threads. (#4047)
- ⌘K now finds individual settings, projects, pull requests and keyboard shortcuts. (#4050)
- Thread rows in the sidebar always show their machine instead of switching to the branch or worktree. (#4051)
- Settings: cleaner spacing and hierarchy for the default harness and the harness list across machines. (#4052)
- The right panel's views share the dark Details card look and are shorter, and the chat scrolls full width under the floating panel. (#4049)
- File references in replies are now the same size as inline code and no longer stretch the line. (#4054)
- Settings › Usage and Statistics now include usage from paired machines, so work run on a server shows up on your Mac. (#4055)
- Show Antigravity's quota in the meter under the message box, and say why an Antigravity quota read or model list fails. (#4057)
- Settings: consistent button styles and section titles, commands shown as code, and a cleaner hosted-agent picker. (#4056)
- Fix moving a thread to another project when it runs on a paired computer. (#4058)
- Settings › Machines: each machine's status sits beside its name, and its settings open from a gear at the row's end. (#4059)
- Paired machines reconnect within seconds while the app is open, instead of after up to a minute. (#4066)
- Scheduled jobs: edit from the row, open runs with one click, run threads name their job, and edits no longer drop the account or select the job name. (#4060)
- Desktop: a paired machine can follow this computer's settings: turn on **Use <this computer>'s settings** on its Settings page. (#4067)
- Smart UI views are about a third cheaper to write thanks to a `::` shorthand for bars, stats, steps, lists, cards, tables, callouts and sources, and the first bar's hover tip no longer hides under the view's buttons. (#4068)
- Thread copies now hold the whole conversation of every thread on a paired machine, not only the ones opened on this computer. (#4070)
- `rustcode --help` lists commands in groups, and config commands live under `rustcode config` (`config sync`, `config mcp`, `config settings`, …). The old names still work. (#4071)
- Add a Refresh models button to the model picker that lists every harness's models again. (#4072)
- Align the favourite star and selected checkmark columns across all rows in the desktop model picker. (#4073)
- Polish model names and icons and collapse live thinking by default. (#4074)
- Split the orchestrator CLI into `rc` (`rustcode/cli`) bundled with the desktop app, moved the standalone `rustcode` TUI/harness to top-level `harness/`, and made the built-in harness opt-in instead of the orchestrator default. (#4075)
- Keep desktop mock windows isolated across checkouts. (#4076)
- Rename Scheduled Jobs to **Loops** across the app, CLI (`rustcode loops`), and agent MCP tools (`list_loops`, `create_loop`, `update_loop`, `run_loop`, `delete_loop`), surface each run's one-line verdict in Loops and Review, and add quick-start Loop templates. (#4077)
- Show GitHub pull request and issue links as compact chips in chat and Smart UI. (#4078)
- Retrying an interrupted engine-owned thread start with the same request ID recovers its recorded outcome without creating a duplicate thread. (#4082)
- Show Antigravity tool failures, interruptions, and ACP prompt error details in thread activity. (#4083)
- Wake linked threads when the pull request already has failing CI checks at link time. (#4089)
- Open New Thread when the thread on screen settles itself. (#4092)
- Worktree threads now clean up their own worktree and branch once their pull request merges. (#4091)
- Agents are told to rerun a `git fetch`/`git pull` that failed because another thread fetched the same repository at the same time. (#4099)
- Antigravity tool rows that show only a preview now link to the saved full output, and Antigravity is told to keep command output short. (#4101)
- Desktop: ⌘K lists every Settings page, /report can save all screenshots at once and records the OS and harness, and secret prompts, file pills and harness rows match the design system. (#4103)
- Agents on a machine without the app open can read usage and the data store and rename their own thread. (#4102)
- Stop shows "Stopping…" right away, says when a stop did not reach another computer, and each stop can be traced from the press to the outcome in the logs. (#4104)
## [v1.44.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.44.0) - 2026-11-19

### Features
- Choose how many finished workflow runs each thread keeps, in Settings › Orchestration › Workflows. (#4007)
- Fix a paired machine staying "Connected" after its link died mid-send, and deliver bursts of its frames to the app in one message. (#4006)
- Machines: a paired machine's transcript loads with less CPU on both computers. (#4009)
- Fixed: harnesses now receive RustCode's full MCP instructions, including the credential and settings rules that were cut off, and can tell its read-only tools from the ones that change something. (#4008)
- Paired machines: a stalled LAN address no longer keeps the VPN route from connecting, and the app does less background work per machine. (#4011)
- Outside agents (Claude Desktop, Cursor, Codex): the RustCode connection keeps working after RustCode restarts, without restarting the other app. (#4012)
- Threads on the built-in RustCode harness can start, read and stop workflow runs. (#4015)
- Thread rows show the state in words with its time beside the title, and the branch or computer with the model on the second line. (#4010)
- Threads on an external harness (Codex, Claude Code, Gemini CLI, OpenCode, Pi) are given a shorter RustCode tool list by default; turn `orchestration-all-tools` on to list the project, scheduled task, workflow and worktree handoff tools again. (#4014)
- Delegates on the orchestrator's own login start without an account approval. (#4016)
- Scheduled jobs can be started by a webhook, with the request body added to the prompt, and can run with no schedule at all. (#4025)
- Improve Markdown link and file reference click targets, clarify named links, and tighten URL chip spacing. (#4028)
- Scheduled jobs run on your current account instead of a signed-out default login, say which login to sign in when one is signed out, and keep their run threads out of the sidebar. Old delegate threads no longer come back to the sidebar once you have more than 500 threads. (#4029)
- Tighten heading spacing in sent messages, highlight Markdown as you type, and omit heading markers from automatic thread names. (#4030)
- Give new threads shorter, clearer titles based on the first request. (#4031)
- Paired computers now sign in to each other with a device key instead of a reusable token. (#4026)
- An always-on machine can now be the fleet's hub (`rustcode machines hub on`). Other machines keep an outbound link to it, and outside agents on the hub reach them with `machine`, at the access ceiling each machine grants. Starting or messaging a thread on an offline machine waits on the hub until that machine connects. (#4032)
- Hosted-agent sign-ins (ChatGPT, Claude) can be approved with the machine owner's passkey on the sign-in page, with no sign-in code. Add one with `rustcode outside owner add-passkey` or in Settings › Outside agents. (#4033)
- Show concrete model and account choices and combine Claude Code's Opus 5.5 usage aliases in Statistics. (#4036)
- Make message actions easier to click and add a whole-interface scale setting. (#4037)
- Toggle the desktop model picker with Cmd+M or Ctrl+M. (#4034)
- The machine with a public address serves the RustCode app to a browser: open it, sign in with your passkey, and use RustCode with no app installed. Signed-in browsers are listed and signed out in Settings › Outside agents. (#4035)
- Show Antigravity model quotas and reset times in Settings › Usage on macOS and Linux. (#4038)
- Make a computer the fleet's hub, or join a hub, from Settings › Machines, without the command line. (#4040)
- Releases include the web app a hub serves to a browser; the installer, Homebrew and `rustcode update` install and update it next to the CLI. (#4041)
- The fleet's hub can run Scheduled Jobs on another machine, holding each run until that machine is online. (#4043)
- Show a compact thread details card with workspace actions, delegates, context usage, and a Command-Shift-B shortcut. (#4042)
- RustCode MCP can compact the current thread or another thread and optionally continue afterward. (#4044)
- Simplify machine management and detect silently dropped machine connections sooner. (#4045)
- File bugs and feature requests from the desktop composer with `/report`. (#4046)
## [v1.43.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.43.0) - 2026-11-18

### Features
- Highlight only Loops in the sidebar while the Loops page is open. (#3982)
- Keep the composer’s remaining plan usage visible while a turn runs. (#3984)
- Show live work in the transcript when a paired thread is running on another client. (#3985)
- Keep the thread name visible in the sidebar while an agent runs commands on a paired machine. (#3988)
- RustCode's MCP now advertises thread names such as `list_threads` and `read_thread`, with compatibility for existing callers. (#3987)
- Choose a harness account for each loop, scan upcoming runs at a glance, and create loops through the RustCode MCP. (#3986)
- Give the model picker's Effort panel more room and show logos for all supported harnesses. (#3989)
- Shorten the docked right sidebar. (#3991)
- Resume an interrupted workflow from its run card and pick up from saved progress. (#3992)
- Threads on a paired machine can answer with Smart UI views and charts when Smart UI is on. (#3993)
- With Smart UI on, agents answer with charts, stats and tables in views instead of Markdown tables, and keep the text around them short. (#3994)
- Sidebar thread rows show their branch or worktree instead of repeating the project path, and rows on the default branch take one line. (#3998)
- Loops are now **Scheduled Jobs**, and a job can run a script first so the agent only wakes when it prints something. (#3995)
- Paired devices and machines can connect through the machine's public address (`wss://`, the same tunnel as hosted agents' `/mcp`). (#3996)
- Show screenshots that Antigravity saves in its own session folder in the chat. (#4000)
- Permission refusals now say which layer refused and where to change it; a delegate on a named account asks with a card instead of refusing, and Claude Code auto-mode denials are marked on the tool row. (#4003)
- Fix Claude Code threads looking finished while a background command they wait on still runs. (#4002)
## [v1.42.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.42.0) - 2026-11-18

### Features
- Settings › Appearance: the live background presets no longer touch the top of their card. (#3964)
- Sidebar: Capabilities and Usage are no longer in the nav block; open them from Settings or ⌘K. (#3966)
- The thread header's buttons now read as one group: the git button is an icon with its actions in its menu, and the status button has no separate background. (#3968)
- Fix the model picker not showing new or newly installed harnesses' models, and its list not scrolling to the last model (#3965)
- Plan usage now shows for threads running on a paired machine (the gateway no longer refuses the request). (#3967)
- Loops: pick days, a time or a date and time from pickers, choose the model from a list, and the dialog no longer resets while you type; weekday schedules now run on the right days. (#3969)
- Threads whose agent is still working on its own (a background command finished and woke it) now show as working when you open them or reconnect, including threads on a paired machine. (#3970)
- A background RustCode server now restarts onto a newer version as soon as one is installed, once no turn is running, so paired apps no longer talk to an outdated engine. (#3971)
- Cmd+W closes a thread that is already settled, the command palette's sort and filter sit beside the search field, and the Settled shelf folds while a settled delegate is open. (#3972)
- `/rename` now names the thread on every harness: alone, the agent picks a title; with a name, the thread takes that name. (#3974)
- Thread names in the sidebar stay put instead of following what a Claude Code agent is doing. (#3975)
- Agents link issues and pull requests by URL, and the chat shows them as chips with their number and title. (#3978)
- Outside agents can be approved from Chromium browsers (Aside, Chrome, Edge), and the sign-in page matches the app's design. (#3977)
- Fix the composer's + menu: Attach, Send later and New Thread did nothing when clicked. (#3979)
- Outside agents get an immediate, actionable refusal instead of a two-minute timeout when a call needs approval and no RustCode app is connected to the machine. (#3973)
- Hosted agents behind a tunnel keep working after RustCode restarts: the engine no longer moves its outside-agent port when the old process is still releasing it. (#3976)
## [v1.41.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.41.0) - 2026-11-18

### Features
- Group thread header actions together and simplify the title to the project, thread name and device. (#3914)
- Simplify sidebar threads to two lines with clearer titles, folder paths and quieter machine icons. (#3918)
- Simplify the sidebar and move thread search, sorting and filtering into Cmd+K. (#3917)
- Group matching project folders across machines in the sidebar. (#3900)
- Auto-update is now opt-in: the background server no longer installs releases and restarts itself, and the app no longer installs updates on its own, unless you turn it on. (#3916)
- Remove suggestion buttons while keeping Tab to insert and Escape to dismiss. (#3899)
- Move or rename a closed project's folder through MCP with `update_project`'s `destination`, keeping its threads and history. (#3891)
- Usage data records a failed `/compact` as a manual compaction with its reason, instead of a failed automatic one. (#3919)
- Usage data now names the program behind Pi shell calls and wrapped commands instead of leaving it empty. (#3920)
- Allow release automation to hand off CI waits without blocking. (#3922)
- Keep phone pairing status consistent as codes expire and devices join. (#3926)
- Fix double-counted usage for threads with backfilled and live turns. (#3924)
- An agent can settle its own thread without the app even when the same call sets a title; the skipped fields are named instead of failing the call. (#3923)
- Keep threads visibly waiting while background commands remain pending across turns. (#3927)
- Prevent failed or cancelled turns from repeating earlier usage and flag suspicious historical Claude usage. (#3925)
- Open Settings › Usage with Cmd+U (Ctrl+U on Windows/Linux). (#3928)
- Usage records the model Claude Code and Pi actually ran and groups provider-prefixed model names together. (#3929)
- Antigravity CLI (`agy`) is a built-in harness, and its token usage shows in Usage. (#3931)
- Fix remote thread working indicators, background updates and reconnects, and let engine-hosted threads hand off authorized pull request merges. (#3932)
- Signing in two harness accounts at once now waits for both instead of dropping the first. (#3933)
- `rustcode usage --pull-requests` reports, per model and harness, how many linked pull requests merged, their CI failures and fix rounds, and the cost and tokens per merged pull request. (#3935)
- Loops: schedule an agent prompt from the app (sidebar › Loops). Each run starts a fresh, read-only thread, a missed run fires once, and three failed runs pause the loop and say why. (#3938)
- Removing a paired machine while it is still connecting no longer leaves its connection running. (#3940)
- Linux and Windows: the File/Edit/Window/Help bar at the top of the window is gone again. (#3941)
- Loops can run once ("in 2h", "at 09:00") or send every run to one thread, and a run that finds nothing to report archives its thread. Loop threads show a Loop badge in the sidebar. (#3942)
- Workflow runs no longer pile up: each thread keeps its 20 newest finished runs. (#3944)
- `rustcode usage --pull-requests --prices prices.toml` adds a clearly labelled cost estimate per merged PR from a price file you supply, kept apart from the cost each harness reported. (#3945)
- New live background: a subtle field of dithered light behind the chat that reacts when you send and when a turn finishes, with presets in Settings › Appearance. (#3947)
- Smart UI views can be saved as an HTML page that opens in any browser. (#3952)
- The live background holds still while Settings is open, and its group has a Reset that undoes a preset completely. (#3953)
- Hosted agents such as ChatGPT and Claude.ai can connect to RustCode through your own HTTPS address (`[outside] public_url` and a tunnel), approved with `rustcode outside code`. (#3959)
- Fixed a race where the engine's settings could be applied after start-up had already finished. (#3961)
- Pictures an agent shows in a thread running on another computer now appear in the app instead of "No such picture.", and a picture that can't be shown no longer breaks the reply's layout (#3960)
- ChatGPT, Claude and other hosted agents can be approved from the RustCode app on any of your devices, from a new consent screen with match words and access levels. (#3962)
## [v1.40.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.40.0) - 2026-11-17

### Features
- Fix projects failing to collapse in the sidebar. (#3887)
- Improve sidebar thread readability with more spacing, clearer text, and aligned machine labels. (#3889)
- Sidebar thread rows stay in place while threads work, show what each thread is doing under its name, put the folder and branch on one line, and drop the diff count. (#3893)
- The sidebar has Loops, Capabilities and Usage rows under New thread. (#3894)
- Claude Code plan usage now shows for threads running on a paired machine. (#3895)
- Threads started on another machine can run in a new worktree. (#3896)
- Agents on a paired machine can settle their own thread and use the thread tools again. (#3897)
## [v1.39.4](https://github.com/LHagfoss/rustcode/releases/tag/v1.39.4) - 2026-11-17

### Features
- Align and tighten sidebar thread rows, preserve single-project headings, and add contextual ⌘N with ⌘⇧N for the default folder. (#3876)
- Add spacing below project folders and drag projects into a saved sidebar order; applying a sort resets manual arrangements. (#3877)
- Refine the sidebar actions and separate the sort and filter buttons. (#3878)
- Allow starting another thread while an earlier one initializes, and remove the Starting label. (#3880)
- Smart UI is more visual: thicker bars, taller charts, option cards instead of flat tables, clickable cards and rows, and tidy sources folds. (#3879)
- Run desktop CI checks concurrently and include dependent Rust packages in CI coverage. (#3884)
## [v1.39.3](https://github.com/LHagfoss/rustcode/releases/tag/v1.39.3) - 2026-11-16

### Features
- Fix Settings › Appearance › Chat background: Dither, Halftone and Scanlines now show, and Blur and Haze work on Windows and Linux. (#3858)
- Fix an empty model picker for Codex and Claude Code on a newly paired machine. (#3855)
- Mention a machine with `@` in the composer to tell the agent where to run the work, with autocompletion. (#3859)
- Chat background effects and blur now render on web views without OffscreenCanvas, such as WebKitGTK on Linux. (#3860)
- Machines can be reached over SSH, and each machine's routes can be added, turned off and removed under Settings › Machines. (#3861)
- Show working folders in sidebar thread rows, reduce repeated machine labels, and make threads needing attention easier to spot. (#3862)
- Chat replies line up with the work above them, a sent message sits closer to its turn, and the first message of each day shows its date. (#3863)
- Clarify the chat header's thread actions button and improve new-thread heading readability over background effects. (#3865)
- Show usage left explicitly in the desktop composer, with the window and reset time on hover. (#3866)
- Settings › Capabilities shows each skill on one line, and "Create with agent" starts a thread that writes a new skill with you; agents can now save skills through the RustCode MCP server. (#3864)
- Selecting text in a message no longer shows an "Ask in a side chat" button. (#3867)
- On a paired machine, the project picker's Browse Folders… lists that machine's folders and makes a new project there. (#3868)
- Keep settled threads done by clearing pending notifications and stopping PR wakeups. (#3870)
- Cmd/Ctrl+W now settles the current thread instead of closing the window. (#3874)
- Keep self-settled threads settled when late pull request or CI notices arrive. (#3873)
- The desktop app uses Inter and rounded icons throughout, keeps compact text by default with optional larger sizes in Appearance, clarifies project and delegate nesting with a full-width New thread button, removes sidebar attention bars, and reserves space for thread pictures while loading. (#3871)
- Settings › Harnesses shows one row per harness, with its accounts and details behind a click, and switches have a smaller knob inset in the track. (#3857)
## [v1.39.2](https://github.com/LHagfoss/rustcode/releases/tag/v1.39.2) - 2026-11-16

### Fixes
- MCP servers you add, toggle, or sign in to or out of reach open threads on their next message, without restarting the app. Settings shows each remote server's sign-in, with Sign in and Sign out buttons. (#3848)
- Settings marks the page that needs attention, and server warnings are readable with a fix or Dismiss button. (#3849)
- The sidebar now sorts by Title by default and keeps project folders A–Z, so nothing moves while threads work or finish. (#3850)
- A merge started with `merge_when_green` now wakes the thread once, not twice. (#3852)
- Saying "settle" to an agent now settles that thread (or the one you name) and takes you to a new thread. (#3853)
## [v1.39.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.39.1) - 2026-11-16

### Features
- A failed turn now offers what fixes it: Sign in when the agent is signed out, Compact when the conversation is full, and Try again where that can work. (#3841)
- The Needs you inbox answers a question in place: the selected question opens with every answer, what each means, and room for your own. (#3842)
- Sidebar rows show how far a working thread is through its plan, and why a waiting thread waits (a limit reset time, no internet, or the thread it waits for). (#3844)
- The Delegates section can show lanes: one row per agent and model, with each delegate as a card in the order the work was handed out. (#3845)
- Starting a delegate now asks with a plan card: which agent and model, how long that usually takes, where it works, its access, and what happens when it is done. (#3846)
- Reviews can show as a findings card: severity, file and line, who found it, and whether it was fixed or set aside, with Reopen. (#3843)
## [v1.39.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.39.0) - 2026-11-16

### Features
- Make selected text readable with matching white highlights and dark text in chat and Smart UI. (#3834)
- Smart UI views space their sections, read list items written as sentences as one paragraph, and stand further from the text after them. (#3835)
- Agents and `rustcode settings` can read and change RustCode's settings, with an approval that shows the value before and after. (#3836)
- Agents and the CLI can turn skills and MCP servers on and off, restart an MCP server, and pick the default harness, after approval. (#3837)
- Agents can move a thread from another project to a chosen harness, account and model; new `rustcode threads move|rename|pin` and `rustcode worktree` commands. (#3838)
- New `rustcode account`, `rustcode connections` and `rustcode outside` commands, and a command reference in docs/cli.md. (#3839)
## [v1.38.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.38.0) - 2026-11-15

### Features
- Accept copied thread links in thread tools and balance the Linux/Windows sidebar titlebar controls. (#3812)
- Add square quick-action rows to thread, project-folder, composer and pull-request menus, with keyboard navigation and accessible labels. (#3813)
- Images now show in old threads and in threads on other computers, pasting an image into a remote thread works, and changing a remote thread's model or harness no longer fails. (#3814)
- Settings and menus now call other computers you run threads on "machines" instead of "environments" (Settings › Machines, Add machine). (#3815)
- The sidebar and the thread header now say which thread a delegate came from, and in which project, when the two are in different projects. (#3816)
- Agents work in their own thread and only delegate when asked. (#3817)
- Switching a thread to another account or harness no longer replays the model's reasoning, which made Claude refuse every turn after the switch. (#3819)
## [v1.37.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.37.0) - 2026-11-15

### Features
- The sidebar has a toolbar again: search threads, sort and filter the list, and start a new thread. Idle threads can settle after a chosen number of days, or never. (#3806)
- Opening a failed thread or dismissing its error now takes it out of Needs you. (#3803)
- A sent message keeps its Edit and Retry buttons while a turn runs, dimmed until it ends. (#3804)
- The thread menu leads with Pin, Rename, Mark Unread and Snooze; occasional tools moved under More. (#3807)
- Settings groups its pages under Agents, Devices & access, Activity and System. (#3808)
- Icons share one size scale across the app, and settings say "this computer" and "environments" consistently. (#3809)
- Use smaller Linux desktop downloads when system WebKitGTK is installed and share the app engine with the CLI. (#3802)
## [v1.36.3](https://github.com/LHagfoss/rustcode/releases/tag/v1.36.3) - 2026-11-15

### Features
- Keep failed tool calls’ exit status on one line beside long commands. (#3791)
- Merge a pull request from the thread panel (it waits for checks), with a tidier panel and a header that hides the hand-driven git buttons by default. (#3792)
- Show automatic pull request merge results as system notices instead of user messages. (#3794)
- The merge button no longer acts on another project's pull request after a thread switch, ignores advisory checks the way the merge does, and asks for confirmation more clearly. (#3795)
- The new thread tip is readable over the background, and General settings no longer has a row that only links to Harnesses. (#3797)
- A Claude Code thread that opens a pull request is woken again when its checks fail or it merges. (#3800)
- Usage data now records Claude Code compactions (recovering past ones), each tool's real name, and why a tool call or turn failed. (#3799)
## [v1.36.2](https://github.com/LHagfoss/rustcode/releases/tag/v1.36.2) - 2026-11-15

### Features
- Check the desktop shell on Linux and Windows before merge to catch platform compilation failures before release. (#3782)
- Settings: Voice and Computer use turn on with a switch in their title and stay one line while off, the composer microphone appears only when dictation is on, and the Harnesses page is split into Harnesses, Orchestration and Voice & computer use. (#3785)
- Add favourites and recent models to the composer model picker. (#3786)
- Sidebar: a Compact density puts each thread on one line (Settings › Appearance › Sidebar rows, or ⌘K "Sidebar: Compact rows"). (#3784)
- Smart UI views space blocks evenly inside tabs, sections, columns and cards, and bar charts are taller. (#3783)
- Settings › Appearance › Chat background: show your picture behind the chat as ASCII, dither, halftone, scanlines or haze, on new threads or all threads with an adjustable visibility. (#3787)
- Perch is detected as a harness, and a custom harness command that fails to start now shows its own error output. (#3788)
- Settings show less text: buttons say what they do, status chips sit beside their actions, long explanations move behind the info mark, and environment errors take one line. (#3789)
## [v1.36.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.36.1) - 2026-11-15

### Fixes
- The Linux and Windows apps build again, and bring their window forward when a second launch or a `rustcode://` link arrives. (#3780)
## [v1.36.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.36.0) - 2026-11-15

### Features
- Prevent linked pull request outcomes from replaying after restarting or updating RustCode. (#3713)
- Keep delegate PR outcomes after cleanup and collapse settled delegates in the status reply. (#3714)
- Remember delegate account approvals before starting work and add an option to trust this computer’s named accounts. (#3715)
- Fixed queued app notices showing as your own message with a raw marker, a steered message showing twice, and failed delegate starts showing an empty error or a spinner that never stops. (#3717)
- Delegates stay nested under the thread that started them when their pull request's news arrives. (#3728)
- Clarify paired-machine connection direction and keep connection settings available during refresh. (#3716)
- The desktop app shows a plan-mode proposal as a readable plan with Build / Keep planning, instead of the raw JSON of an approval. (#3719)
- Projects can keep quick actions (named shell commands) that run in the thread's terminal from the header's ⚡ menu or ⌘K. (#3720)
- The project menu in the thread header can open the project folder in VS Code, Cursor or Zed. (#3722)
- Make Settings revisits faster with cached probes, optimistic save feedback, lazy sections and remembered navigation. (#3718)
- Settings › Permissions: one place for default access, per-project defaults, per-thread overrides and remembered approvals. Changing the default now applies to open threads, and toggles save instantly. (#3723)
- The thread header has a git button: commit, push, pull and open a pull request without asking the agent. (#3729)
- The terminal has tabs, and selected terminal output can be added to a message with ⌘L. (#3730)
- Open a pull request as a worktree thread from the Pull requests panel, seeded with its review comments and failing checks. (#3731)
- Projects can run a setup action when a worktree thread is created (the agent can wait for it) and a cleanup action when the thread settles, from the actions editor or `.rustcode/actions.json`. (#3732)
- Expanded tool calls show the command, output and exit code, or the arguments as readable rows instead of raw JSON, and Settings › Chat can hide tool calls. (#3736)
- Smart UI tables wrap long code paths and links instead of scrolling sideways and clipping their last column. (#3739)
- Open shared prompt, thread and connect links directly in RustCode, and copy prompt or thread links from the app. (#3737)
- Add configurable text generators for thread titles, commit messages and pull request drafts. (#3747)
- Fix the terminal closing and reopening shells in a loop once tabs were open in five folders. (#3751)
- Reopening a pull request in a thread reuses its branch and brings it up to the head, and the git button no longer offers to create a pull request that already exists or shows another thread's git state. (#3755)
- Show the Rewind, side chat and Send later hints as the app's tooltips, and show a terminal tab's close button and a chart's Copy source on keyboard focus. (#3752)
- Answer an agent's question with files (attach, paste or drop), and the question panel keeps a draft per question, shows its place in a set and works fully from the keyboard. (#3757)
- Dictate a message: the composer's microphone button or ⌘⇧D transcribes your voice into the draft through an OpenAI-compatible provider chosen in Settings › Agents › Voice. (#3765)
- Settings can override the default harness, access and thread behaviour for one project, on one machine or on all of them. (#3770)
- Edit external harness commands and masked environment variables in Settings, and use Cursor CLI as a known ACP harness. (#3761)
- An expanded command's output shows its colours, bold and underline in the theme's colours, and progress bars show only their last frame. (#3771)
- Pull requests in the right panel have a Diff tab with review conversations on their lines, and can add one conversation or every unresolved one to the chat. (#3772)
- F6 and Shift+F6 move focus between the sidebar, the conversation, the message input and open panels, and between the section list and the section in Settings. (#3763)
- Ask before running repository-defined project actions, stop their child processes at timeout, and prevent overlapping settle actions. (#3753)
- Ask for credentials in a private masked card and save them directly to an untracked workspace dotenv file. (#3754)
- The macOS app asks for microphone access before dictating and explains how to turn it back on, and Settings warns when another copy of RustCode opens rustcode:// links, with a one-click fix. (#3775)
- Dictation can clean up its text with your text generator, and a project can choose its own text generator and voice settings. (#3776)
- Agents can ask for a secret as an environment variable: commands in that thread get it, it stays in memory, and it shows as `***` if printed. (#3774)
- Reply to, resolve and add line comments on a pull request's diff without leaving the app. (#3777)
- Type `#` or `@` in a pull request comment to complete issues, pull requests and people. (#3778)
## [v1.35.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.35.0) - 2026-11-14

### Features
- Linked pull request updates reach their thread while the app runs, even when another RustCode process checks GitHub, and show as a notice instead of a message from you. (#3677)
- The desktop app binary is about 11 MB smaller. (#3678)
- Record external tool-call duration and command names for direct command-time queries. (#3682)
- The composer's usage meter and limits now refresh on their own every three minutes, without sending a message. (#3681)
- Tab and Shift+Tab now respect the "Tab moves focus between controls" setting in Settings, the side panel and in-chat views. (#3680)
- Settings buttons, fields and toggles are slightly shorter, so the pages are more compact. (#3684)
- Fix Codex WebSocket reconnects caused by stale inherited certificate-directory settings. (#3685)
- Require account-specific approval before delegates can spend a different named login's usage. (#3686)
- Add a smaller Linux system-WebKitGTK tarball with in-app updates for Arch and other distributions providing WebKitGTK 4.1. (#3692)
- Connect over SSH can now give both computers access to each other (Both ways), and Environments shows and revokes each direction separately. (#3693)
- Outside agents connected to RustCode can now search threads and list projects. (#3689)
- Automatically notify threads about their newly created PR's checks and let them hand off an approved merge without holding their turn. (#3691)
- Keep cleaned delegates settled, restore access to retained history, and allow cleanup after desktop checks generate Tauri schemas. (#3695)
- The delegates a thread started now show as one collapsed line above the composer, with Open and Stop per delegate when opened. (#3694)
- Add a localhost HTTP MCP endpoint with OAuth discovery, S256 PKCE and scoped, expiring, revocable outside-agent access. (#3697)
- Outside agents can fan out, interrupt, merge, clean up and rename or pin threads, gated by their access level and recorded in the audit. (#3698)
- Show a copyable outside-agent HTTP URL in Settings and preserve its localhost port across restarts. (#3699)
- Add opt-in isolated browser tools with domain restrictions and RustCode approvals. (#3700)
- Settings › Remote access now shows which network each connected device uses and whether it is encrypted, and a new remote security guide describes the threat model. (#3701)
- Agents can start durable workflow runs (`run_workflow`, `workflow_status`): a sandboxed script fans work out to delegate threads and a restarted run continues from its first unfinished call. (#3702)
- Enable computer use and manage allowed browser domains from desktop Settings. (#3703)
- Document outside-agent client setup and add HTTP OAuth interoperability regression coverage. (#3704)
- Workflow runs now show in the app as a card in the chat and a row above the composer with Stop, and an agent can stop one with `stop_workflow`. (#3706)
- `rustcode machines list` shows each connected machine's network and whether it is encrypted, and the app notes when a machine moves to another network (for example "Now on LAN · Unencrypted"). (#3705)
## [v1.34.2](https://github.com/LHagfoss/rustcode/releases/tag/v1.34.2) - 2026-11-13

### Features
- Agent builds are much faster: cargo uses every core at low priority with incremental compilation, and tests run in CI instead of on the laptop. (#3674)
- Orchestrators can wait for a whole wave of delegates in one `wait_session` call (`all: true`) instead of waking once per delegate. (#3675)
## [v1.34.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.34.1) - 2026-11-13

### Fixes
- Fix Claude Code threads failing to start with an ENOENT error in projects with command policies. (#3662)
## [v1.34.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.34.0) - 2026-11-13

### Features
- Softer, inset dividers across the app, segmented controls fit their buttons, and the model picker's effort column has rounded inner corners. (#3648)
- Updates retry a dropped download, keep a failed install on the sidebar row with a Retry, and ask before a manual update ends running threads. (#3639)
- Threads from other environments are now also copied to this computer's disk, read-only, so they stay readable when an environment is offline, wiped or removed; Settings › Environments › Thread copies turns it off or leaves projects out. (#3641)
- Settings › About › System setup lists the per-OS setup steps (PATH, quarantine, launcher entry, background server) with a status and Run again, and explains the macOS prompt after each update. (#3647)
- Settings › Remote access now says when a phone connected with a secure `wss://` address or an app version this computer does not understand, and when a pairing code expired because no device reached this computer. (#3643)
- Renaming an environment now shows on every paired computer, and environments paired through another computer are listed with their version and last-seen time. (#3644)
- Pi, OpenCode, Gemini CLI and custom agents can now ask you multiple-choice questions with an `ask_user_question` tool; the question appears as a card and the thread shows "Needs you". (#3640)
- Thread folders, logs and telemetry payloads are no longer deleted automatically; old logs are compressed, and the data store records branch, commit, pull request, effort, account, diff stats and approvals. (#3646)
- Fixed thread status drifting from the engine, Stop and Trash not working on a stuck turn, and quitting leaving running delegates behind. (#3656)
- Delegate presets for `start_session` and `fan_out`, and an opt-in setting that moves a delegate to another provider's model when it hits a usage limit. (#3642)
- Projects can check in `.rustcode/command-policy.toml` to make agents' shell commands denied or rewritten, with violations counted per thread. (#3657)
- Idle harness sessions are no longer evicted for the session cap before 30 minutes unless memory is low, and a turn a restart cut off resumes once with a "Resumed after restart" note. (#3655)
- Parallel delegates no longer block each other on cargo build locks, and a delegate compiling for a long time is no longer reported as stalled. (#3658)
- Delegate finishes and stalls are merged into one orchestrator turn per 60 s window (setting), and orchestrator threads show a live delegate status panel and answer a bare "status" without a model turn. (#3659)
- Connect other apps (Claude Desktop, Cursor, Codex) to RustCode's MCP tools with a scoped, revocable token from Settings › Outside agents. (#3645)
## [v1.33.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.33.0) - 2026-11-13

### Features
- Show the model while a thread is running. (#3611)
- Honor the Tab focus navigation setting from text fields. (#3612)
- Polish Smart UI charts and controls and improve sidebar and project icon spacing. (#3613)
- Ask about selected message text in a side chat and add its answer to the main thread draft. (#3614)
- Scheduled and queued messages keep their attachments, and a thread past the thread-list cap keeps its scheduled messages and limit waits. (#3615)
- Settings › Chat › Suggested next prompt: after a reply that offers a next step, the empty composer suggests the answer; Tab inserts it to edit, Esc dismisses it. (#3616)
- Screen readers hear when the open thread finishes, asks a question, needs approval or fails, and the window title names the thread and its status. (#3617)
- Pull requests on a GitHub Enterprise remote you have not signed in to now show the sign-in hint, and unsupported remotes list no pull requests instead of failing. (#3618)
- Trashing a thread works on Linux and on a thread store on another macOS volume. (#3621)
- Change a thread's project from the header or composer project picker, and add projects by dropping folders on the sidebar. (#3620)
- Add selected pull request review feedback to a thread draft. (#3622)
- Fix invoking skills by name and slash command across harnesses. (#3623)
- Keep a Codex thread working until its own turn finishes. (#3626)
- The terminal no longer freezes the app when a shell stops reading input, and the link preview cache stays bounded. (#3627)
- The thread overview and compare cards no longer read the whole diff to count changes. (#3625)
- Send one-way thread messages by default and identify their sender on remote clients. (#3630)
- Let agents give their threads useful names while keeping names you choose. (#3628)
- The `@` and `/` menus in the composer are more compact and rank the best match first. (#3631)
- Settings pages are less wordy: descriptions are one line, with the detail in a tip. (#3632)
- The model card now sets model, effort and account in one place, the + icon is crisp, and threads on another machine name it in the composer. (#3633)
- Settings › Machines is now Settings › Environments, with live status and Reconnect on each row, one Add environment dialog (connect code, SSH, address) with steps and Try again, and a Run on selector for new threads that shows each environment's status and address. (#3634)
- Every harness is now told which skills exist, and a skill turned off in Settings stays off for Claude Code and for resumed Gemini CLI, OpenCode and Pi threads. (#3635)
- MCP Apps: a tool result's interactive view appears in the thread in an isolated frame, and its tool calls ask for approval. (#3636)
- Smart UI views can lead with a headline number, quiet stats and a banded chart, and chat Markdown headings are bigger. (#3637)
## [v1.32.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.32.0) - 2026-11-13

### Features
- Settle, review, unsettle and pin a disconnected machine's threads; the change shows at once and syncs when the machine is back. (#3588)
- macOS no longer asks for the login keychain password for machine tokens after every update, reconnect, or for each of the app and CLI; existing pairings are moved over with one last prompt. (#3589)
- Agents can now hand a whole task to another paired computer: they answer its prompts, manage its queue, search it, read its worktrees and pull requests, and clone projects there. A remote delegate starts in the project with the same git repository, never an unrelated one. (#3591)
- Pair two computers with a connect code (Settings › Machines › Create connect code, or `rustcode machines code` / `connect`), with no accept step; a saved machine's token now only goes to an address that proves it is that machine. (#3593)
- Paired machines reconnect as soon as any route answers (also after sleep, on app focus and when the network returns), a machine that answers but refuses says why instead of "Offline", and `list_machines` reports the reason and reconnects on `probe: true`. (#3590)
- Paired machines no longer need pairing again after months of use, or when the app and the CLI connect at the same time. (#3592)
- A RustCode server restarted over SSH on Windows no longer dies when the SSH session closes, and a machine's gateway restarts a server that has stopped, so paired machines reconnect without anyone touching it. (#3594)
- Connect to a computer you reach over SSH in one step (Settings › Machines › Connect over SSH, or `rustcode machines add-ssh user@host`): RustCode is installed there at this version if needed, its server started, and the two paired. New `rustcode up` sets up a headless machine and prints a connect code; the installers take `--version`. (#3595)
## [v1.31.4](https://github.com/LHagfoss/rustcode/releases/tag/v1.31.4) - 2026-11-12

### Fixes
- Fewer thread toasts and notifications: only a question, an approval, a failure, or a thread that is truly finished. (#3580)
- A thread whose recorded folder differs now repairs itself and opens without pressing Repair Thread. (#3578)
- Fix `rustcode server restart` run from a thread inside the server killing the server without starting a new one. (#3579)
- The desktop app installs updates by itself once no thread is working; turn it off in Settings › About. (#3581)
- A paired machine that is offline keeps its threads in the sidebar and readable from the last copy this computer saw, even after the app restarts. (#3582)
- Settings no longer freezes or loops when a paired machine or a harness does not answer; it shows the error with Retry. (#3583)
- A background RustCode server now installs new stable releases by itself once no turn is running and restarts onto them; turn it off with `[server] auto_update = false`. (#3584)
- Settings reads easier: monospace is limited to short paths, commands and ids, paths are shortened with the full path on hover, and errors start with a plain sentence. (#3585)
- A thread row keeps its own model and shows its running delegates as a count on the line above. (#3586)
## [v1.31.3](https://github.com/LHagfoss/rustcode/releases/tag/v1.31.3) - 2026-11-12

### Fixes
- A thread waiting on background work says it will continue on its own when that work finishes. (#3576)
## [v1.31.2](https://github.com/LHagfoss/rustcode/releases/tag/v1.31.2) - 2026-11-12

### Fixes
- The sidebar shows a thread's harness icon to the left of its model. (#3570)
- Sidebar rows show a thread's state as an icon, so titles get more room. (#3571)
- Sidebar rows leave the `feat/` or `fix/` prefix off a branch name, so more of it fits. (#3572)
- Smart UI views respond instantly to hover, tabs and folds, keep sources in a folded list inside the view, and links in a view open in the browser. (#3573)
- `rustcode data query` and `query_data` see a thread's harness messages within a second instead of after 128 more arrive. (#3574)
## [v1.31.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.31.1) - 2026-11-12

### Fixes
- A delegate started on another machine now shows under its parent thread, with the machine's name, and opening it scrolls to that row. (#3550)
- Fix the model picker's effort and account panel running under the title row, and make its account segments fill the panel. (#3552)
- Fix: the macOS top bar and traffic lights sit lower, on one centre line. (#3553)
- The sidebar lists every machine's threads in one flat list, tagging threads from other machines with the machine's name, and Settings moves next to the sidebar toggle. (#3555)
- Update remote thread status immediately while it saves. (#3556)
- Show the branch on thread rows and keep their heights consistent. (#3557)
- Align macOS window controls and move the sidebar controls to the right. (#3558)
- Sidebar thread rows are all one height, with every detail always in the same place. Dragging a row now opens a gap where it will land and scrolls the list near its edges. (#3560)
- The macOS installer signs RustCode.app with the local certificate, so Keychain stops asking for the password after each install. (#3559)
- A thread that messaged another no longer comes back as "Waiting" when you work in that other thread yourself. (#3561)
- Thread rows no longer cut off "Working", show the model, keep their height while renaming, and a dragged row is no longer clipped below the last row. (#3562)
- Spend on the new thread page and in Statistics now includes a list-price estimate for turns whose harness reported no cost, covering more models. (#3563)
- A settled thread stays in Settled while a thread it messaged is still working. (#3564)
- A thread row without a git branch shows its folder on the third line instead of leaving it blank. (#3565)
- New threads start in the home folder when no folder is chosen, and the folder menu no longer lists the same Windows folder twice or RustCode's install folder. (#3566)
- Claude models show as Opus 5.5 and the like, every reply says how long it took, and repeated restart lines collapse into one. (#3568)
- Threads on a Windows machine open again, and Repair Thread works on a paired computer. (#3567)
## [v1.31.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.31.0) - 2026-11-12

### Features
- Threads started on another paired computer appear there and here at once, a title change reaches every app, a trashed thread leaves every list, and a thread no list has yet is no longer dropped by a read that started before it. (#3522)
- A thread with no RustCode app open can now start, fan out and message other threads (at Full access), and wait on them. (#3534)
- After an app update, the next start updates the `rustcode` a terminal runs and restarts a background server still on the old engine (or names the Homebrew/cargo copy it cannot update), and a paired machine that is away says Reconnecting… while the app brings it back. (#3521)
- Threads and delegates stopped by a usage limit now continue on their own after the reset, including the Claude Code "Internal error" form of the message. (#3543)
- Machine settings lists each route of a paired machine with whether it answers and how fast, with a Test routes button; list_machines shows the same and takes probe: true. The VPN route is now saved with its port. (#3545)
- Removed the "main machine" from paired machines: every machine is a peer, and `machines list` and Settings > Machines no longer mark one. `rustcode machines list --json` no longer has a `main` key. (#3546)
- Remote delegates: an answered turn no longer shows as Failed with no reason, `start_session` names the thread on its machine and never starts in the app's data folder, and `read_activity` and a delegate's provider error are read from the machine the thread runs on. (#3528)
- A paired phone is now told who sent each message, so a delegate's report or a message from another thread no longer looks like something you typed. (#3537)
- A thread whose app is closed now gets RustCode's orchestration tools: the engine answers thread and machine lookups itself, within the app's read-access rules, and names the app for the calls that still need it. (#3520)
- A delegate waiting out a usage limit wakes its parent once instead of at every retry, stopping or archiving the parent cancels that wait, and Codex and Gemini limits now continue on their own too. (#3547)
- Fix a delegate that kept showing as working after its turn ended, and make Stop settle a link whose thread is no longer running. (#3535)
- Fix a delegate that was messaged mid-turn staying on Working after its thread went idle. (#3548)
- Fixed a thread started from another machine disappearing from the list about 10 minutes in while it was still running. (#3536)
- `thread_status` reports no pull request provider, not a GitHub error, for a project whose remote is not on GitHub. (#3538)
- A Codex thread on Windows now says in its instructions that the sandbox denies WMI, and what to read instead; `docs/harnesses.md` documents the denial and the registry/.NET alternatives (#3526). (#3531)
- A thread whose answer mentions being offline or a lost connection no longer gets "The internet connection is back. Please continue." sent to it over and over. (#3518)
- Correct the docs: a new thread's banner fills the whole pane (#3519)
## [v1.30.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.30.0) - 2026-11-11

### Features
- Keep macOS Keychain access across app updates when the local signing certificate is configured. (#3437)
- The model picker card now fits inside the window, and accounts show as side-by-side chips under the effort list. (#3450)
- Check all in Settings > Harnesses now ends with the same results as pressing Check on each harness, and shows its progress. (#3451)
- A delegate running on another machine now appears in that machine's thread list, and delegate rows show a plain-text preview of the answer. (#3452)
- Settings › Machines says a machine's route problem once and no longer lists this computer under Machines found. (#3453)
- The composer's Run on chip is idle at rest and shows just the machine's name. (#3454)
- RustCode no longer compacts threads on its own unless you turn on Auto-compact in Settings; when it does, the divider says so and links to the setting. (#3455)
- A Codex thread with Full access now runs outside Codex's sandbox, and `rustcode server status` no longer calls the server stopped when the shell is only denied access to it. (#3459)
- Windows: private files stay usable when an elevated process created them, a denied file is named in the error, and `rustcode server status` flags a server in another logon session. (#3460)
- On macOS, closing the window with Cmd+W or the red button keeps RustCode and its running threads going; click the Dock icon to bring the window back. (#3473)
- Fix Windows file reference links and warnings for writes outside the thread folder. (#3475)
- Fix Windows folder names, project and worktree grouping, shortened paths, and skill and theme folder labels in the desktop app. (#3477)
- Fix Pi thread startup with orchestrator tools on Windows. (#3481)
- Keep Windows awake while turns run, allowing idle sleep again after the last turn finishes. (#3484)
- Preserve external thread history and saved harness state across restart and handoff, and keep interrupted parents visible so delegates stay nested. (#3476)
- Move deleted Windows threads to the Recycle Bin and use the platform's name in the desktop UI. (#3474)
- Fix the workspace terminal opening a Windows shell on Windows. (#3483)
- Fix Windows terminal clipboard copy/paste and image attachments from pasted Windows paths. (#3478)
- Show Windows-compatible install hints for missing tools and stop requiring tmux in doctor. (#3482)
- The Machines page can point the `rustcode` in a terminal at the app's version in one step (`rustcode server link-path`). (#3486)
- Keep credentials, history archives and debug logs private on Windows, including when the configuration directory inherits shared permissions. (#3485)
- Send a message later: schedule a draft from the composer's + menu for 30 minutes, a few hours, this evening, tomorrow morning, or a time you pick, and it waits in a card over the composer until then. (#3490)
- Fix harness detection on macOS when the login shell probe fails: Codex and other Homebrew-installed harnesses are found again, and Pi reports Not installed instead of failing at the first turn. (#3489)
- Open Devices and Settings from the sidebar foot, with a redesigned device list and a machine card. (#3492)
- A turn a provider overload stopped retries itself with a doubling backoff, and a turn the network killed waits for the connection instead of failing for good. (#3491)
- `rustcode machines list` shows a machine's platform instead of `unknown`. (#3493)
- Paired machines report their real projects, not install folders or `/`. (#3488)
- Run scheduled tasks on Windows with daemon and cron commands, app scheduling, and MCP scheduling tools. (#3495)
- Linked pull request events now reach and wake their desktop thread once, including on paired machines. (#3498)
- Show agent screenshots from project and temporary folders inline, with Open and Reveal actions when an image cannot be displayed. (#3499)
- The `@` picker in the composer now groups its agents by harness and shows a few recent rows of each section first, with a Show all row for the rest. (#3501)
- Agents can message and steer threads across projects with user approval, with queue fallback for harnesses without live steering. (#3502)
- Validate a model profile's `env_key` as an environment variable name: a pasted key is ignored (the profile keeps working through `api_key`) and reported once, `doctor` names the field that supplies each profile's credential, and a secret stored in both `api_key` and `env_key` is reported as a misconfiguration — never echoing the value. (#3494)
- Find the home folder through `USERPROFILE` on Windows, so user skills, usage logs and `~` paths work there. (#3503)
- A delegate that finishes on another machine now reads Done in its parent's row, and `wait_session` reports it, instead of Failed — a failed row is the turn's own failure, not a machine's channel that went away. (#3504)
- Desktop: the highlighted first row in the slash and `@` drawer follows the drawer's rounded corners. (#3513)
- Run skills picked from the desktop slash drawer on every harness. (#3515)
- `rustcode machines list` no longer tells you to fix a VPN this computer is not on. (#3505)
- Rename paired computers and give each an icon in Settings › Machines; the name and glyph appear in Devices, thread headers and Run on. (#3500)
- A paired machine that goes offline is retried every minute at most, on the route that worked last, and at once when your network changes. (#3506)
- Add a Machine settings page for each paired computer: controls, its harnesses, and a table of what syncs and what stays there. (#3507)
- Free the room the built-in orchestration tool schemas needed, and list `manage_pull_links` in the built-in block. (#3508)
- `start_session` and `fan_out` can start standalone threads of the user's own, not only delegates. (#3510)
## [v1.29.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.29.1) - 2026-11-11

### Fixes
- The sidebar groups threads by project again. A Devices list at the bottom opens another computer's threads, and a new thread started there runs on that computer. New threads show a **Run on** picker whenever a computer is paired. (#3427)
- Settling, marking for review and snoozing a thread on a paired computer now saves on the computer that runs it and stays in sync across apps. (#3430)
- Agents on a Mac or Linux machine can start threads in a project on a paired Windows machine. (#3426)
- Delegate rows in the chat no longer show a separate › button; press the row to open the thread. (#3428)
- A computer asking to connect shows up under its own name instead of "RustCode on macos". (#3424)
- `rustcode remote accept <id>` and `rustcode remote deny <id>` answer a request to connect from the terminal. (#3423)
- `rustcode server status` no longer warns about the PATH copy when the server only needs a restart to match it. (#3416)
## [v1.29.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.29.0) - 2026-11-11

### Features
- Show and drive threads from every paired machine in the sidebar, with `Runs on ‹machine›` in the chat and read-only threads while a machine is away. (#3400)
- New threads can start on another connected machine: pick **Run on** in the composer, with that machine's agents, models and projects, and the thread appears under its group in the sidebar. (#3403)
- Agents can now answer the approvals and questions their threads wait on, organize several threads in one call, and attach files to the messages they send. (#3401)
- Show one row per delegate thread with its latest state and message count, and count waiting threads rather than messages. (#3410)
- `rustcode remote` now listens on a machine's LAN and overlay addresses (NetBird, Tailscale, ZeroTier) with `bind = "auto"`, `rustcode machines` races the routes it knows (LAN first, overlay after a second, five seconds at most) and saves the addresses the machine reports, and `rustcode machines list` says what to check when one network answers and another does not. (#3402)
- Agents can now read, create, clone, rename and unregister the user's projects, and schedule work on the daemon's cron from chat (`read_project`, `create_project`, `clone_project`, `update_project`, `remove_project`, `schedule_task`, `list_scheduled_tasks`, `delete_scheduled_task`, `run_scheduled_task_now`). (#3406)
- Edit settings, skills and MCP servers on one machine or on all paired machines, with Mixed where they disagree. (#3405)
- Settings › Machines lists the machines found on your network — LAN and NetBird/Tailscale — with their RustCode version, connecting in one step; turning on Remote access installs the RustCode server as a background service that starts at login. (#3404)
- Agents can list the computers this RustCode is paired with (`list_machines`) and start, message, read and wait on threads on any of them by passing `machine`. (#3407)
- Add `worktree_status` and `handoff_worktree`, so an agent can report where each worktree stands and hand a delegate's branch back to your checkout without losing a commit. (#3413)
- The RustCode server now runs the remote gateway, so a paired machine still reaches this one after the app closes; `rustcode server status` shows the gateway and warns when the `rustcode` on PATH is an older copy than the app's engine. (#3414)
## [v1.28.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.28.0) - 2026-11-10

### Features
- Sidebar threads are three-line cards with a clear status, including Done when an agent finishes; the header drops the name and version. (#3352)
- The model picker is a narrow card with a harness icon rail, and effort, Fast mode and account moved to their own small menu beside the model pill. (#3353)
- Slimmer toggle switches, and a steered message's Copy button no longer shows without its Edit and Retry. (#3354)
- Pairing QR codes now include the Mac's other addresses (LAN, mesh), so a phone that cannot reach the advertised address can still pair. (#3355)
- The sidebar no longer shows the app icon, the traffic lights sit higher again, and switch knobs fill their track. (#3356)
- Effort is back in the model pill: picking a model opens its effort levels in the same card. (#3357)
- Sidebar rows: hover actions replace the status in place instead of covering the title, and a diff alone no longer makes a card three lines tall. (#3358)
- Model picker: only the model list scrolls, and the dark divider lines are gone. (#3359)
- A steered message's badge and time no longer sit before an empty gap. (#3360)
- Smart UI views no longer have their bottom row cut off. (#3362)
- Remote access now lists every thread on the Mac, across all projects, and the phone learns the Mac's LAN, VPN and host-name routes from the handshake, so pairing once is enough to reconnect on another network. (#3361)
- Move model effort and account settings to the top of the model picker. (#3363)
- Reduce Smart UI movement and blur when hovering in chat. (#3364)
- Sidebar rows stay two lines (three with a branch): delegates sit beside the title instead of wrapping onto a new line. (#3365)
- Paired devices can read usage and settings, and pin or settle threads. (#3366)
- Skills are offered in the composer's slash menu, and `/` now opens it mid-message too. (#3367)
- Pasting a long block into the desktop composer now sends it as plain text where it was pasted, instead of a code fence at the end of the message and a chip in the transcript. (#3370)
- Orchestrator threads use less context: delegates end with a short report, waits default to 20 minutes, answers are compact, and the orchestrator is told when its context passes 25%. (#3371)
- Agents can link a thread to a pull request and hear about reviews, checks, conflicts and the merge or close once, without polling GitHub themselves. (#3373)
- Show why a phone or computer could not connect: recent refusals with what to do, and an update-the-app message with the supported versions. (#3374)
- Snoozing the open thread now leaves it for a new thread, instead of keeping it on screen. (#3377)
- Keep turns, tool calls, compactions and operational events forever; prune only bulky raw protocol payloads; record compactions from the built-in harness, Codex and Claude Code, with richer turn and tool metrics. (#3372)
- Statistics: the daily chart's tracker reaches the first and last day, and its tooltip no longer covers the measure switch. (#3380)
- Turn off macOS autocorrect, text suggestions and Writing Tools in the app's text fields. (#3378)
- The delegates above the composer open from their arrow, and fold into one line without covering the chat. (#3379)
- The model picker glides between the model list and the effort and account step, and lists each model once. (#3381)
- Tool rows in the desktop chat stop spinning once a backgrounded command returns, say "Started in background", and show fenced command output as output instead of raw backticks. (#3383)
- Repeated quiet notices in a thread's chat (one per call an access mode allowed) now read as one line with a count instead of six identical rows. (#3384)
- A thread that RustCode compacts while one of its delegates is still working carries on by itself, with a "Continued after compaction" divider, instead of stopping there. (#3385)
- Paired phones can fold delegates, forks and side chats under the thread they were started from. (#3386)
- A snoozed thread now stays quiet until its snooze ends: it keeps working in the background without coming back to the list, notifying, badging or marking unread, and returns with its state and one notification at most. (#3387)
- Smart UI views in the desktop app now grow to their full content height instead of being capped at 720 px and scrolling inside the chat. (#3396)
- Delegates read the same everywhere: the sidebar row shows "Waiting" with its live delegates, the delegate strip has one header, its popover no longer says "Interrupted" for a delegate that works or needs you, and the Overview lists delegates in their own section. (#3397)
- Mention agents, threads and files with `@` in the composer (`#` jumps to threads): an agent mention tells the orchestrator which harness, model and effort to delegate to, and the cursor now stays on its character in a draft with tokens. (#3398)
## [v1.27.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.27.0) - 2026-11-09

### Fixes
- Connecting another desktop to this machine works again: a paired desktop relays to the engine the app runs, and the Machines page says why a connection failed instead of only retrying. (#3350)
- Other machines and phones can find this one with "Look for machines" again: the network announcement survives Wi-Fi changes, sleep and startup, and macOS asks for Local Network access. (#3349)
- The sidebar shows running and idle threads in one Threads list that never stays folded, with roomier rows by default (Compact is an option). (#3345)
- The sidebar's brand and the window's title bar are one row, and Windows gets its own minimize, maximize and close buttons instead of the native title bar. (#3348)
- The model picker shows effort and account beside the model list, with no second page. (#3347)
- Toggle switches use the same thick white knob as the effort slider. (#3344)
- No blur on Windows and Linux: solid surfaces there, macOS unchanged. (#3346)
## [v1.26.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.26.0) - 2026-11-09

### Features
- Inline code in replies copies when pressed, and inline code, file references and links now share one pill shape. (#3310)
- Long pastes in the composer collapse into a pasted-text chip instead of a wall of text. (#3316)
- The model picker keeps one height while you move inside it, always opens on the model list, and shows its effort slider and accounts compactly. (#3311)
- Tooltips and hover cards now share one timing: they appear after a short rest, hand over instantly between neighbouring buttons, and dismiss on a press, Escape or a scroll instead of sticking until the next click. (#3313)
- Glass surfaces are lighter to look through — lower blur, lower alpha, the composer included — and the new thread's overview keeps its blur while it opens and closes. (#3314)
- Claude Code plan usage is cached for five minutes per account, and a rate limit waits out its `Retry-After` (else backoff up to 30 minutes) while showing the last numbers, instead of polling and showing an error. (#3315)
- Move the sidebar's new-thread, add-project, Settings, pull requests and usage buttons into ⌘K, so the thread list gets the space. (#3312)
- Sidebar thread rows are one compact line, with a second only for a thread that has a branch, a diff, a pull request or a delegate stack; Settings offers Compact and Comfortable. (#3319)
- Smart UI views now keep the same layout, spacing and type scale between turns, no longer resize when the pointer moves over them, and follow the app theme's radius, fonts, accent and text sizes on macOS too; `smart_ui_check` now flags colours, fonts and fixed heights the kit already sets. (#3320)
- The composer's attach button gets a crisper + and its text a tighter line height, opening at one line. (#3317)
- The thread's right panel is now one scrollable overview: properties, pull requests, deliverables, the execution log and the details as foldable sections with counts. (#3318)
- Dismiss items in "Needs you", one at a time or all at once, with Undo. (#3324)
- The sidebar now leads with the threads that are working and gives the threads waiting on you a section of their own, instead of a shelf at the foot. (#3321)
- Pasting a screenshot with Ctrl+V now attaches it in the desktop app on Linux. (#3329)
- The sidebar now shows the RustCode icon, its name and the running version at the top, above Needs you. (#3331)
- The sidebar's thread section is called Asking you, so it no longer reads as a second Needs you count, and a row's status icon and harness mark sit side by side instead of stacking. (#3333)
- Glass surfaces blur again in the desktop app on Linux. (#3330)
- Machine pairing no longer hangs: a credential store that cannot answer is reported before the other machine is asked, every step is bounded, and each stage of the pairing has a cancel. (#3327)
## [v1.25.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.25.0) - 2026-11-09

### Features
- Fix a possible app freeze when a thread shows a shell command with a long or unterminated quoted variable. (#3249)
- The pull request panel, its detail and the review view draw from saved answers and refresh behind, so opening one no longer waits on `gh`. (#3250)
- Sending a message in a settled thread brings it back to the active list. (#3269)
- Improve drag feedback when arranging thread header buttons in Settings. (#3270)
- Keep Tab from moving focus between controls unless enabled in Settings. (#3271)
- The sidebar is slimmer: search and the filter menu moved into ⌘K, Working, Snoozed and Settled fold on their own, and Pinned and Active are separate from them. (#3274)
- Group plan usage by harness and show Claude usage without a setting. (#3276)
- Make models easier to find with provider panes, search, and a compact account selector. (#3273)
- Add an OLED theme and soften borders, sliders and menu corners. (#3275)
- Add an in-app date and time picker for custom snoozes. (#3272)
- Connect enabled MCP servers automatically when the app starts or the engine restarts. (#3278)
- Settings › Harnesses (renamed from Coding agents) has a single Check per harness that re-detects, verifies sign-ins and lists models, plus Add harness for your own local ACP command. (#3280)
- Context compaction now shows "Compacting context…" while it runs and an error row if it fails, instead of pausing silently. (#3279)
- Add an optional title-first layout for sidebar thread rows. (#3284)
- Delegates can be messaged again after they were interrupted or moved to another harness. (#3281)
- Orchestrators can see a delegate's latest activity and provider errors first. (#3282)
- The Settings page for harnesses is called Harnesses in the sidebar too. (#3285)
- Sending in a settled thread no longer shows an Unsettled toast. (#3286)
- Forks and side chats record the thread they came from: they nest under it, a fork links back to its parent, and a reply can be asked about in a side chat beside the thread. (#3247)
- The SwiftUI iOS client is removed from this repository; the phone app is the Expo app in `rustcode-mobile`. (#3289)
## [v1.24.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.24.0) - 2026-11-09

### Features
### Machines
- Pair this computer with the other computers running RustCode: Settings › Machines lists this computer and each paired machine with its state, the route in use (LAN unencrypted or VPN encrypted), last seen and thread count, and adds machines through Bonjour or an address, ask to connect and an eight-digit code. (#3241)
- `rustcode machines add|list|remove|discover` does the same from the command line. (#3238)
- Pairings no longer expire after 30 days: apps that support it exchange rotating refresh tokens, and a replayed old token revokes the pairing. The CLI and desktop app rotate their tokens and say when a pairing must be made again. (#3237, #3244)

### Desktop
- The new thread screen keeps its tip when the overview is open, drops the pill behind its heading and tip (a soft shadow keeps them readable over a picture), the composer has no border, and the header's fade no longer covers the right panel. (#3231)
- Rearrange sidebar threads by dragging a row (or ⌥↑/↓), double-click the sidebar's edge to reset its width, and hover a row to preview its project, harness, status, branch and last message. (#3233)
- A delegated thread's chip says what last happened to it (started, message sent, asked you, answered with its time, failed, merged), and its tip lists the whole delegation. (#3232)

### Orchestration
- `list_harnesses` takes `harness` and `model` filters and leads with the models in use, so its answer is about a fifth of the size. (#3236)
- `wait_session` and `merge_when_green` send progress while they wait and return within 25 minutes, so long waits are no longer cut off by the agent's client; a dropped merge can no longer merge later. (#3245)
- `cleanup_worktree` cleans up a delegate's worktree after it renamed its branch. (#3239)

### Fixes and dependencies
- Upgrade jsonwebtoken to 10, zip to 9 and rustls to 0.23.46. (#3234, #3230)
## [v1.23.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.23.0) - 2026-11-08

### Features
- `rustcode server install` runs the RustCode server in the background so threads keep running after you close the app; `server status`, `restart`, `logs` and `uninstall` manage it. (#3218)
- The new thread screen asks "What should we build in <project>?" again (click the name to change folder) and shows a tip with a shortcut above the field. (#3226)
- Added `rustcode server run` and `rustcode server connect`: one RustCode server that several clients share, where threads keep running when a client closes. (#3213)
- A client that reconnects to the server gets only the updates it missed instead of reloading the whole thread. (#3217)
- Remote access now works on Windows, and machines are found on the network without dns-sd or Avahi. (#3212)
- Paired devices show what they are (phone or desktop, and the platform) and when they were last seen. (#3211)
- Update in the app now installs the new version on Linux (AppImage and .deb) and Windows, not only macOS. (#3216)
- Windows uses Mica and Linux gets back its blur, with a Reduce transparency setting; the new-thread background can be darkened, blurred and shown in threads; the chat input is a little shorter. (#3215)
- The new thread's picture no longer pushes the window's content up when the overview opens, and text over it, Settings notes and the Usage page read more clearly. (#3223)
- Agents can see each harness account's usage windows in list_harnesses, and Settings › Usage refreshes every 3 minutes. (#3219)
- The transcript no longer redraws finished turns while a reply streams. (#3222)
- Fix Claude Code and Pi exiting immediately when RustCode runs as a Linux AppImage. (#3221)
- Fix a server memory leak on failed thread loads, a Usage refresh that could be dropped, and a partial file left behind by a failed AppImage update. (#3224)
- A desktop app you approve on this machine can open threads here over remote access; `rustcode remote devices allow` and `deny` grant and remove that access. (#3225)
## [v1.22.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.22.0) - 2026-11-08

### Features
- Keep the remote pairing QR code visible until it is used, expires, or is dismissed. (#3200)
- Improve composer readability and refine borders and settings controls. (#3201)
- Keep Smart UI list items with badges and descriptions readable. (#3203)
- Prevent extra console windows from appearing when using the Windows desktop app. (#3204)
- A phone can now ask to connect from the local network: the desktop app shows "‹device› wants to connect" anywhere, and after Accept shows a code to enter on the phone. Paired devices can be revoked from Settings › Remote access. (#3205)
- The desktop app uses each system's own fonts on Windows and Linux, and on Linux its translucent surfaces are solid so text stays readable. Focus rings now appear only while you move focus with Tab, never when you click. (#3206)
- Settings › Diagnostics shows the RustCode server's status, live log, system and harness details and copies a redacted debug report for GitHub issues; threads get *Open Thread Log*. (#3208)
- Delegate rows say what a delegate is doing instead of a raw command, provider errors read as sentences with a hint when the conversation is full, and a delegate whose turn failed can be messaged again and leaves the Inbox once settled. (#3207)
- On Windows and Linux the app shows Ctrl/Alt shortcuts, native window controls and wording for the OS instead of macOS terms. (#3210)
## [v1.21.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.21.0) - 2026-11-08

### Features
- Remote access mirrors every open desktop thread, on any harness, to paired phones, with its prompts and approvals. Switch it on and pair a phone in Settings › Remote access. (#3183)
- A thread started on a paired phone opens in the desktop app on the harness and model the phone picks. (#3197)
- The new-thread overview fades open and closed, the dark composer is darker, and a new thread starts in the default project folder even while another thread is open. (#3194)
- What's New reads the release's own changelog: categories as small badges, pull requests as `#N` links, a skeleton while it loads, and a card that scales in and out. Link chips keep their site icon on a light tile, so dark icons stay visible. (#3195)
- Auto-compact keeps the last answer in view, with one quiet row under it. By default it runs only when the context passes its size threshold, and never while the last answer is unread. The thread menu copies the thread ID in one click. (#3196)
## [v1.20.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.20.1) - 2026-11-08

### Features
- The desktop app uses the lavender accent by default, matching the built-in themes; text on accent buttons switches to dark so it stays readable. (#3192)
## [v1.20.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.20.0) - 2026-11-08

### Features
- The desktop app can follow betas: Settings › About › Update channel adds Beta, which offers the newest prerelease in the usual update row with Skip this beta. (#3177)
- Keep the new-thread composer centred and make its overview optional. (#3175)
- The model picker's header is one row: the effort and model together, with Fast mode and reset at the end. (#3180)
- The delegate strip above the composer now shows only what runs now (`5 running · 2 need you`), with no lifetime thread count, and hides once nothing is running or waiting. (#3181)
- Settings › Harnesses lists Pi (and every standard harness) from the engine, with hand-added harnesses grouped apart. (#3179)
- The desktop sidebar shows a thread's status as one chip (`Working 57:13`, `Waiting`, `Needs you`, `Failed`) instead of two spinners and a link count, and a settled or idle row keeps just its age. (#3185)
- Add Settings › Remote access: share the app's threads with a paired phone, with a locally drawn pairing QR code. (#3178)
- Normalize gaps and padding across the desktop app: one spacing scale for the sidebar, header, Settings, menus and cards. (#3188)
- Threads compact automatically when their context grows large, and Claude Code threads compact before their prompt cache expires; Settings › General › Threads and the thread menu turn it off. (#3186)
- Count each Claude Code turn's usage once: a restarted harness no longer re-records its whole session total on every later turn. (#3182)
- Add a total delegate limit alongside the per-thread one, with a Settings › Agents Delegation group showing live usage. (#3189)
- Claude Code threads delegate through RustCode's threads instead of their own subagents. (#3187)
- MCP orchestrators can now report a delegate's pull request and checks, merge it once green, and clean up its worktree and branch and settle the thread. (#3184)
## [v1.19.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.19.0) - 2026-11-07

### Features
- Add the missing execution-graph decision note (`docs/architecture/execution-graph.md`), which several docs already linked to. (#3156)
- The new thread screen is just the field and a see-through dashboard of cards, over a picture that covers the whole pane; a press on the usage meter opens Settings' Usage again. (#3158)
- The remote gateway accepts the mobile app; it used to refuse every WebSocket upgrade that carried an `Origin` header (#3157)
- The desktop sidebar marks threads from the keyboard: P pins or unpins, D settles or unsettles, and Shift+F10 (or the ContextMenu key) opens the row's menu. (#3160)
- The desktop app drops its card borders: settings groups, panels and popovers separate by their fill, and the Settings sidebar is a flat list of pages. (#3159)
- Remove the checkpoint dots from the desktop chat's timeline rail, keeping the checkpoint and *Rewind here* in the strip's hover card. (#3164)
- The composer's delegate strip counts only the threads working now, shows the longest running one's time instead of a sum of every finished delegate, and no longer toasts each time a delegate finishes. (#3165)
- Show the sidebar's Needs you row only when a thread actually waits on you, not as an empty heading over the thread list. (#3166)
- Wake a parent thread when a delegate the app resumed after a restart answers: an orchestrator no longer stays silent after reopening the app. (#3167)
- Let a thread running in a git worktree reach its repository's threads, and the threads it started, through the agent orchestration tools. (#3162)
- `rustcode threads settle`, `unsettle` and `review` change thread status from the CLI, accepting several ids or unique id prefixes. (#3163)
- Skills installed for Claude Code (`.claude/skills` in a project, `~/.claude/skills`) now appear in Settings › Skills, read only, and threads on external harnesses are told where the discovered skills are. (#3161)
- A device that attaches during a turn no longer receives the answer twice. (#3168)
- Settings › Usage no longer calls a named Claude Code account expired when only its stored access token has lapsed: it reads both copies of that login's sign-in, and draws the numbers RustCode recorded for it last, marked as of when they were read. (#3172)
## [v1.18.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.18.0) - 2026-11-07

### Features
- The new-thread screen has a frosted composer and a row of glass cards under it: recent activity, this project's threads and pull requests, usage and this month's turns and spend. (#3142)
- Add an Unsettle action for settled threads, in the sidebar, the header, the command palette and above the composer. (#3144)
- CI: detect a pull request's changed paths from the API instead of cloning the history, and stop re-running PR labeling on every push. (#3143)
- The selected segment, switch and slider knob are white in both themes, and the Trace popover's axis, bars and row columns line up. (#3145)
- The model picker remembers the effort each model was last left on (shared by every login of a harness), keeps a new thread's model, effort and login when you open another thread and come back, and draws a smaller card with a white slider thumb that reads on any theme. (#3146)
- The timeline minimap draws a checkpoint bead inside its own strips instead of a separate lane of dots you could not hover, and the strip's preview card offers Rewind here. (#3147)
- Fix a flaky TUI test that let a parallel render test overwrite the theme a theme test was asserting on. (#3148)
## [v1.17.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.17.1) - 2026-11-07

### Features
- New thread titles use the project folder name; long Windows paths show in full on hover, and new threads no longer show another thread's model or effort while their harness loads its models. (#3119)
- A built-in thread whose config has `models = []` no longer sends its turns to a compiled private host: it stops with a message that says how to add a model or choose another harness. (#3123)
- Windows: Codex and npm-installed harnesses are detected again without a PATH edit, reinstall or extensionless copy; the executable is found through `PATHEXT`. (#3117)
- The model picker opens on an effort slider with the level in large type, the model under it and accounts below; picking a model returns to the slider with its effort clamped to that model's levels. (#3122)
- Sending the first message of a new thread no longer skips its arrival, and a message sent in an open thread no longer jolts after it lands. (#3121)
- Ease the hover and keyboard highlight in the composer's project picker. (#3120)
- Offer every reasoning effort each Pi and OpenCode model really supports (`minimal`, `low`, `medium`, `xhigh`), learning a model's own levels when it is used. (#3124)
- A needs-you inbox across all threads, and a while-you-were-away recap after a long break. (#3126)
- Fix a fresh Windows profile failing to save its migration checkpoint, which repeated every migration and warned on each start. (#3118)
- A steering note now shows on the plan step it landed on (or as a quiet line under its message), a queued bubble glides out of the stack when the turn reads it, and plan steps show a live clock and their duration. (#3125)
- Fix a blank window on start when a thread's side panel was remembered open. (#3132)
- Delegates now appear in a nested tree with live work time and rolled-up cost; finished branches fold, and Board cards summarize their delegates. (#3128)
- The working line ends on a green check, counts roll, toasts stack and fan out with a timer hairline, and the composer's usage number shimmers while a turn runs. (#3130)
- Claude Code is now reported signed in or signed out in Settings › Agents, read from its own `auth status`, instead of always showing "Installed". (#3127)
- Fix sends and wakes holding at "Starting…" while the window is hidden. (#3134)
- A new thread no longer stays on the Starting… screen with its turn running behind it: the chat hands over to the thread as soon as the message has landed. (#3137)
- The composer's context card now shows what fills the window as a tile map with the biggest files and tool calls, marks the automatic compaction point, and compares before and after a compaction. (#3131)
- Approvals show what they will run, how risky it is, and let you allow it once, for an hour, or always. (#3136)
- A finished turn's Trace chip shows where its time and cost went: each model call, tool call and delegate on the turn's timeline, the slowest by work, with per-call tokens and cost and approval waits drawn apart. (#3129)
- Rewind a thread to a turn's checkpoint as a new attempt: the turns it leaves stay below, faded under an earlier attempt you can go back to, and the minimap marks every checkpoint. (#3135)
- Redesigned Settings (in-page section nav, slider, status chips, segmented effort) and Statistics (period switch, KPI tiles with deltas and sparklines, hoverable daily chart, linked cost breakdown, plan limits). (#3138)
- Keep delegates from hanging on a git editor, pager or endless command: harness children run non-interactively, and a parent is told once when a delegate stops making progress. (#3140)
- Smart UI views gain hover tooltips, legend toggles, filled line charts, a donut total, inline sparklines and tables that sort on a heading click, and agents are told which view answers which question. (#3133)
- Delegated coding tasks now carry the repository rules and time bounds that help prevent commands from stalling. (#3139)
## [v1.17.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.17.0) - 2026-11-07

### Features
- Accounts are explicit: each harness's subscriptions are chips in its header in the model picker, "Default login" is gone, adding an account signs in from the app, and logins already on the Mac are offered to add. (#3114)
- Codex accounts: a thread can run on any Codex login (`CODEX_HOME`), chosen and added like a Claude Code account. (#3115)
- Settings › Statistics shows a best-known cost (reported, or estimated at API list prices) with a redesigned summary, cost-by-model bars and a 30-day chart. (#3112)
- The project picker lists every project, ranks them by use and opens above its button. Pressing Shift+Enter after using the model picker no longer reopens it, and pasted images sit closer to the message. (#3110)
- A finished delegate's notice now names it, its harness and how long it worked. (#3111)
- `bun run mock-window` in `apps/desktop` gives agents one shared, visible Chrome window for UI checks, with real clicks and keys. (#3109, #3113)
## [v1.16.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.16.1) - 2026-11-06

### Features
- Agent marks are solid circles without a border, so overlapping marks no longer show through each other. (#3097)
- The composer is rounder with even padding and a larger `+` button, header button groups sit on a track, and the side-panel toggle is always rightmost. (#3098)
- The chat has more room between steps and turns, and tool rows sit quieter under the prose without a line under each. (#3099)
- The data store gains provider request, harness message and operational event tables, with opt-in payload capture, redaction and bounded retention. (#3100)
- The built-in harness records each provider request attempt (timing, status, bytes, tokens) in the data store for `query_data`. (#3101)
- RustCode records every message it exchanges with ACP and Codex harnesses (method, size, latency) in the data store for `query_data`. (#3102)
- Agents can page through a thread's recorded activity (events, provider requests, harness messages) with the new `read_activity` tool. (#3103)
- Settings › Advanced can turn on recording of provider request bodies and harness messages (redacted, kept 7 days) for agents to query. (#3104)
- Agents can pass `client_request_id` to start, message and fan-out calls so a retry after a timeout never duplicates the work. (#3105)
- Agents can list, edit, cancel or steer the messages they queued to a busy thread with `manage_queue`. (#3106)
## [v1.16.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.16.0) - 2026-11-06

### Features
- The update row appears within 30 seconds of a release, and only once the release can actually be installed. (#3085)
- The side panel stays closed until you open it, and each thread reopens the panel you last had open on it. (#3086)
- The composer grows and shrinks smoothly with your message, its menus open from the button you pressed, and project, branch, access and usage move to a strip under it. (#3087)
- Chat turns are tighter: only a finished reply shows copy and fork, and the agent icons for delegated threads are clearer. (#3089)
- Smart UI views fill in while the agent writes them, and their forms can send your choices back to the agent. (#3088)
- Links in chat and the composer sit on the text line, and you can mark and copy chat text across messages, links and code. (#3090)
- Idle agent processes shut down after 30 minutes and pick up where they left off on your next message. (#3092)
- RustCode sends a system notification and badges the Dock when a thread needs you or finishes. (#3091)
- Queued messages survive a restart, and a delegated thread's question reaches its busy parent right away when that agent can take it. (#3093)
- A new thread shows your first message instantly while the agent starts, and sent messages animate into the chat. (#3095)
- Codex's own subagents show up as live agents in the thread, the same as other harnesses' subagents. (#3094)
## [v1.15.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.15.0) - 2026-11-06

### Features
- Show the RustCode version and recorded token total in Discord Rich Presence. (#3077)
- Delegated threads stay under their parent when the sidebar is filtered or searched, and the open one shows as an indented sub-row. (#3078)
- Threads whose worktree is removed still open, in the repository the worktree came from, and threads already broken this way reopen there. (#3079)
- The right panel docks beside the chat at full height instead of floating over it and following the composer, with a per-thread width, a sheet in a narrow window, and the details view as one of its tabs. (#3080)
- The chat and the composer share one 736px lane and one left edge, the composer card is rounder with evenly inset rows, it grows and shrinks with its lines instead of jumping, and ↑/↓ walk your sent messages. (#3081)
- Steering is decided per harness: the composer says whether a message will steer, queue or restart the turn, and a queued message no longer hides why it waits. (#3082)
- Menus and the model picker open above the composer card, with one searchable list that steps the effort in place, and a single usage meter. (#3083)
## [v1.14.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.14.0) - 2026-11-06

### Features
- The chat composer now floats over the conversation in a tighter 16px card, with a soft bottom fade, and its pickers open clear of the card instead of over it. (#3075)
- Reply Markdown is quieter and easier to scan: code blocks sit on the code surface, task lists show one box instead of a box and a bullet, nested lists step their markers, table rows are parted, and bold is semibold. (#3074)
## [v1.13.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.13.0) - 2026-11-06

### Features
### Chat and layout

- The composer is one card with a single row of controls: the attach button, folder, worktree and access on the left, the meters, model and send on the right, with a rounder corner and less height at rest. (#3064)
- A `@file` reference in the draft reads as a token in the text, and the plan meter, context ring and agent marks move into one menu when the control row is too narrow to hold them. (#3072)
- Changes, Pull requests and the thread peek size the side panel to what it holds instead of drawing it full height, and a long list still stops above the composer and scrolls. (#3063)
- A thread opens the right panel on its own overview (thread details) instead of the git Changes card, and the side cards float over the chat again instead of moving it aside: the chat keeps its place and the transcript scrolls under the card. (#3066)
- A draft with more than three pictures shows three thumbnails and a `+N` count that opens the rest, instead of a strip that grows with every image. (#3067)

### Threads

- A thread's sidebar row counts each file once instead of summing every edit, so a row no longer shows `+12404` for work on a dozen files. (#3069)
## [v1.12.1-beta.3](https://github.com/LHagfoss/rustcode/releases/tag/v1.12.1-beta.3) - 2026-11-06

### Fixes
### Chat and layout

- A thread's sidebar row counts each file once instead of summing every edit, so a row no longer shows five digits for work on a dozen files. (#3069)
## [v1.12.1-beta.2](https://github.com/LHagfoss/rustcode/releases/tag/v1.12.1-beta.2) - 2026-11-06

### Fixes
### Chat and layout

- A thread opens the right panel on its own overview (thread details) instead of the git Changes card, and a view with nothing to show in this project falls back to it. (#3066)
- The side cards float over the chat again: the chat keeps its place and its width, the transcript scrolls under the card, and every card ends above the composer so the field is never covered. (#3066)
- A draft with more than three pictures shows three thumbnails and a `+N` count that opens the rest, instead of a strip that grows with every image. (#3067)
## [v1.12.1-beta.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.12.1-beta.1) - 2026-11-06

### Fixes
### Chat and layout

- The composer is one card with a single row of controls: the attach button, folder, worktree and access on the left, the meters, model and send on the right, a rounder corner and less height at rest. The model reads as its own name rather than a filled pill. (#3064)
- Changes, Pull requests and the thread peek size the side panel to what it holds instead of drawing it full height, and a long list still stops above the composer and scrolls inside the card. (#3063)
## [v1.12.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.12.0) - 2026-11-06

### Features
### Usage

- Settings › Statistics estimates what each row would have cost at API list prices, in a column of its own beside the cost each harness reported. Cached input is priced as cached input, each harness's token counts are normalized first, and a model with no list price stays blank instead of guessing. (#3055)
- The context ring in the composer follows a turn while it runs on Pi and Claude Code, instead of only updating when the turn ends. (#3061)

### Chat and layout

- Changes, Pull requests and the thread peek now float as the same card as thread details, and the chat is centred in the room they leave instead of being shifted off-centre. (#3056)
- The message box is one floating card with its footer inside it, opens two lines tall, and a line typed with Shift+Enter no longer hides in the field's bottom fade. (#3057)
- The thread details card's Started and Messaged groups say what each holds, a delegated thread no longer flashes into the sidebar as a row of its own, and one whose parent is not listed keeps its row instead of disappearing. (#3058)
- A finished turn shows its changed files once instead of twice, and older turns keep that line inside their fold instead of repeating it up the thread. (#3060)

### Reliability

- Settings › Capabilities reconnects an MCP server in place with a button, instead of turning it off and on again. (#3059)
## [v1.11.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.11.0) - 2026-11-06

### Features
### Orchestration

- Choose named accounts when delegating work between threads. (#3047)
- An orchestrating thread gets one combined notice when several delegated threads finish, and is no longer woken again for results it already saw. (#3050)
- Follow-ups no longer take a delegation slot, the default is 8 threads at once (2–16 in Settings › Agents), prompts and messages may be 32 KiB, and long answers are paged from `read_session`. (#3022)
- A thread started by another thread now runs at no more than the access of the thread that started it, so an Ask or Auto-edit thread can no longer hand a child Full access. (#3025)
- Child threads' approvals and questions now appear in the parent thread's transcript, and can be answered there. (#3028)
- Delegated threads no longer add sidebar rows: the parent shows an overlapping stack of agent marks, and the chat's group cards use the same stack. (#3036)
- A thread an agent started no longer shows up as a stray top-level row: it stays with its parent, or in Settled once finished when the parent is gone. (#3031)
- Clarify delegated agent status and thread relationships. (#3045)
- Claude Code's built-in subagents now show as the same agent icons in the composer tray, sidebar and chat, and the tray stays on one row. (#3038)
- A thread blocked in `wait_session` no longer keeps its links and its children's answers for up to an hour when the harness drops that call: the wait ends at once and the finished child's answer still arrives as a wake. (#3030)
- Threads stopped by a usage limit now continue on their own when the limit resets, unless you moved them to another harness or account, sent them something else, or turned this off in Settings › General. (#3040)

### Usage

- Settings › Statistics shows recorded token use and reported cost by harness, model and thread, with delegated threads rolled into their parent. (#3051)
- Settings › Usage now updates a Claude plan after you sign in again, and stops re-prompting for Keychain access after you deny it. (#3049)
- The composer's plan meter now has numbers on threads that run on a named Claude account, reads them when the thread opens, and stops re-reading an account whose Keychain access was denied. (#3033)
- A signed-out default Claude login is hidden in Usage, the model picker and delegated threads. (#3021)
- Reopening Settings › Usage, Connections or Capabilities now draws the last data at once and refreshes behind it, instead of starting from a skeleton every time. (#3035)

### Chat and layout

- The chat transcript is quieter: one place for a turn's duration, no error mark for a turn that succeeded, tighter spacing around notifications, and copy actions on hover. (#3053)
- Open the thread panel by default and close settled chats. (#3048)
- Improve the right panel layout and resizing on narrow windows. (#3044)
- Keep composer controls usable in narrow windows. (#3046)
- Add a Chat width setting (Comfortable, Wide, Full) and show one action row under a finished reply instead of two. (#3020)
- Agents now answer structured questions with designed Smart UI views built from a component kit, and views run when they scroll into sight. (#3041)
- Threads started with a worktree now appear in the sidebar and chat immediately, with Setting up and elapsed time, instead of looking as if nothing happened. (#3027)
- Queued messages show their state and actions on hover, like a sent message, and Send now steers pictures on harnesses that take them mid-turn. (#3034)
- Toasts stack up to three, keep their own timers when another is in front, pause while hovered, focused, dragged or in a background window, never select text while dragged, and Escape dismisses them again; notices that repeated what the screen already showed are gone. (#3037)
- Give the chat's thread strip, group card and Latest pill one glass, and fade the long-draft composer instead of cutting it. (#3032)
- Opening a picture now blurs the background, and one click zooms in and out. (#3039)
- The Review screen names the base its branch totals compare against (`vs main`, full ref on hover). (#3024)
- Titles and tool labels are no longer cut inside an emoji or an accented letter. (#3023)

### Reliability

- A harness turn that stops sending output now ends as failed after 10 minutes with nothing running (no unfinished tool call, no permission or question waiting on the user, no background work), and the next message resumes the thread instead of staying "Working" until Stop. (#3026)
- A failed worktree setup no longer leaves its new branch and half-made checkout behind, so retrying a thread keeps the same branch name. (#3029)
## [v1.10.1-beta.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.10.1-beta.1) - 2026-11-05

### Features
- Add a Chat width setting (Comfortable, Wide, Full) and show one action row under a finished reply instead of two. (#3020)
- Follow-ups no longer take a delegation slot, the default is 8 threads at once (2–16 in Settings › Agents), prompts and messages may be 32 KiB, and long answers are paged from `read_session`. (#3022)
- fix(desktop): stop cutting titles and tool labels inside an emoji or an accented letter (#3023)
- The Review screen names the base its branch totals compare against (`vs main`, full ref on hover). (#3024)
- fix(desktop): hide a signed-out default Claude login in Usage, the picker and delegated threads (#3021)
- A thread started by another thread now runs at no more than the access of the thread that started it, so an Ask or Auto-edit thread can no longer hand a child Full access. (#3025)
- Threads started with a worktree now appear in the sidebar and chat immediately, with Setting up and elapsed time, instead of looking as if nothing happened. (#3027)
- Child threads' approvals and questions now appear in the parent thread's transcript, and can be answered there. (#3028)
- A failed worktree setup no longer leaves its new branch and half-made checkout behind, so retrying a thread keeps the same branch name. (#3029)
- A harness turn that stops sending output now ends as failed after 10 minutes with nothing running (no unfinished tool call, no permission or question waiting on the user, no background work), and the next message resumes the thread instead of staying "Working" until Stop. (#3026)
- A thread blocked in `wait_session` no longer keeps its links and its children's answers for up to an hour when the harness drops that call: the wait ends at once and the finished child's answer still arrives as a wake. (#3030)
- A thread an agent started no longer shows up as a stray top-level row: it stays with its parent, or in Settled once finished when the parent is gone. (#3031)
- Give the chat's thread strip, group card and Latest pill one glass, and fade the long-draft composer instead of cutting it. (#3032)
- The composer's plan meter now has numbers on threads that run on a named Claude account, reads them when the thread opens, and stops re-reading an account whose Keychain access was denied. (#3033)
- Reopening Settings › Usage, Connections or Capabilities now draws the last data at once and refreshes behind it, instead of starting from a skeleton every time. (#3035)
- Queued messages show their state and actions on hover, like a sent message, and Send now steers pictures on harnesses that take them mid-turn. (#3034)
- Delegated threads no longer add sidebar rows: the parent shows an overlapping stack of agent marks, and the chat's group cards use the same stack. (#3036)
- Toasts stack up to three, keep their own timers when another is in front, pause while hovered, focused, dragged or in a background window, never select text while dragged, and Escape dismisses them again; notices that repeated what the screen already showed are gone. (#3037)
- Claude Code's built-in subagents now show as the same agent icons in the composer tray, sidebar and chat, and the tray stays on one row. (#3038)
- Opening a picture now blurs the background, and one click zooms in and out. (#3039)
- Threads stopped by a usage limit now continue on their own when the limit resets, unless you moved them to another harness or account, sent them something else, or turned this off in Settings › General. (#3040)
- Agents now answer structured questions with designed Smart UI views built from a component kit, and views run when they scroll into sight. (#3041)
## [v1.10.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.10.0) - 2026-11-05

### Features
- Opening, zooming and panning a picture is now smooth: the image follows a trackpad pinch, a wheel or a drag without trailing behind, and a cancelled drag no longer leaves the picture stuck to the pointer. (#3011)
- The composer's attach button now sits inside the input, in a rounder, more glassy box, and slash commands read better in the field and in sent messages. (#3013)
- Settings, Pull requests and Usage move from the sidebar's foot to its top row; clicking the composer's plan meter opens Usage. (#3012)
- Choose which buttons the thread header shows, and drag them into the order you want (Settings › Chat › Thread header buttons). (#3015)
- Threads started by an agent now read as the same rows as its subagents — grouped into one card, titled by the thread, and shown in one place at a time instead of twice above and inside the chat. (#3014)
- Queued messages now show at the end of the thread, with editing in the composer; a stopped or failed turn holds them in order with Resume, wake-ups read as one quiet line instead of the user's message, and a wake no longer repeats an answer a wait_session already returned. (#3017)
- A thread whose turn ended while the threads it started still run now says so — `Waiting on 2 threads` at the foot of the transcript and `Waiting` in its sidebar row — and a finished child no longer puts a green dot on its parent. (#3018)
- Use several Claude Code logins on one machine: add named accounts in Settings › Agents and choose one per thread from the model picker. Each account keeps its own limits; RustCode never switches or pools them on its own. (#3016)
## [v1.9.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.9.0) - 2026-11-05

### Features
- Settings has a cleaner layout, and the whole app has rounder corners. (#2995)
- Sidebar rows are redrawn, and child threads list flat under their parent. (#2996)
- The composer floats over the chat with a footer tray, and slash and @ menus attach to it. (#2997)
- Subagents and delegated threads share a clearer two-line row. (#2998)
- Approvals are answered from one bar instead of showing the same buttons twice. (#2999)
- Settings is now a floating panel with Restore defaults and an About page. (#3007)
- Sidebar rows show project and PR state above the title, and common actions move to the sidebar footer. (#3004)
- Model search, editable queued messages, math in replies, and clearer tool rows. (#3005)
- Diffs gain word highlights and split view, and thread details is a floating card. (#3006)
- Thread links are kept in the data store, and a child thread can start its own children. (#3008)
- Slash commands now appear in a new thread before it starts. (#3009)
## [v1.8.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.8.0) - 2026-11-04

### Features
- Rework the chat lane, reply type and composer into one card (#2978)
- Split Settings into pages with a grouped nav and add resets to rows (#2979)
- Select several threads in the sidebar, navigate it with the keyboard, and see diff counts on rows (#2981)
- Agents can answer questions about your threads and read threads in other projects, with a setting to turn it off (#2980)
- Show the real reason when a thread fails to start (#2983)
- Remap app shortcuts in Settings > Keyboard (#2982)
- Show approvals, answered questions and plans in the timeline, and keep drafts across restarts (#2984)
- Add a local data store with `rustcode usage` and `rustcode data query` covering tokens and reported cost per harness and model (#2986)
- Wake a parent thread when its children finish or need you, and report why delegation was refused (#2987)
- Codex, Claude Code, Pi and OpenCode can draw rich Smart UI views when asked for a visualization, and Pi and OpenCode now receive RustCode's instructions (#2989)
- Keep tool output and exit codes in Pi, Codex and Claude Code threads (#2990)
- Agents can answer usage questions and query the data store with `thread_usage` and `query_data` (#2992)
## [v1.7.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.7.1) - 2026-11-04

### Features
- Fix the composer's folder menu staying open after Esc or a click elsewhere. (#2951)
- Make buttons, pickers and menus consistent across the app, with real dropdowns in Settings. (#2952)
- Let agents find harnesses and threads, and rename, pin, stop and reconfigure threads, through RustCode's MCP tools. (#2953)
- Fix Pi threads not being able to start, message or manage other threads. (#2954)
- Answer approvals with Enter and Esc, redirect the agent by typing, and answer a child thread's approval from its parent. (#2955)
- Open Settings as a floating panel and search every setting with `/`. (#2957)
- Snooze threads until later, and see running threads in their own section of the sidebar. (#2956)
- Let agents list your projects and pull requests through RustCode's MCP tools. (#2958)
- Add a Details view to the right panel: harness, workspace, linked threads, subagents with elapsed time, plan and usage. (#2961)
- Make the remaining action buttons match in shape, size and hover. (#2962)
- Reset a changed setting to its default from its own row. (#2964)
- Show model and effort changes in the transcript, and add Check again to harness errors. (#2963)
- Agents can move a thread to another harness and see which threads are waiting on you. (#2965)
- Finished and failed toasts name the thread and open it. (#2967)
- Reference project files with `@` in the composer. (#2968)
- Keep titles given when an agent starts a thread, and stop showing Pi's startup banner in threads. (#2966)
- Agents can snooze a thread until "tomorrow" or "next week", and older Pi threads no longer open with the startup banner. (#2969)
- Removed the Command-digit thread shortcuts and their hints from the sidebar. (#2971)
- Chat text is easier to read: Markdown and command chips in your messages, better line spacing, day-aware times, and "Show full message" only when it hides something. (#2972)
- The thread timeline is now a minimap: hover a strip to preview the message and its answer, click to jump. (#2973)
- Code in chat diffs is now syntax highlighted, and more file types get colours. (#2974)
- Subagents now show which harness and model they run on, stay visible after the turn folds, and announce when they finish. (#2975)
- Watch a thread you started in the side panel without leaving yours. (#2976)
## [v1.7.1-beta.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.7.1-beta.1) - 2026-11-04

### Features
- Fix the composer's folder menu staying open after Esc or a click elsewhere. (#2951)
- Make buttons, pickers and menus consistent across the app, with real dropdowns in Settings. (#2952)
- Let agents find harnesses and threads, and rename, pin, stop and reconfigure threads, through RustCode's MCP tools. (#2953)
- Fix Pi threads not being able to start, message or manage other threads. (#2954)
- Answer approvals with Enter and Esc, redirect the agent by typing, and answer a child thread's approval from its parent. (#2955)
- Open Settings as a floating panel and search every setting with `/`. (#2957)
- Snooze threads until later, and see running threads in their own section of the sidebar. (#2956)
- none (#2959)
- Let agents list your projects and pull requests through RustCode's MCP tools. (#2958)
- Add a Details view to the right panel: harness, workspace, linked threads, subagents with elapsed time, plan and usage. (#2961)
- None (internal wording only). (#2960)
- Make the remaining action buttons match in shape, size and hover. (#2962)
- Reset a changed setting to its default from its own row. (#2964)
- Show model and effort changes in the transcript, and add Check again to harness errors. (#2963)
- Agents can move a thread to another harness and see which threads are waiting on you. (#2965)
- Finished and failed toasts name the thread and open it. (#2967)
- Reference project files with `@` in the composer. (#2968)
- Keep titles given when an agent starts a thread, and stop showing Pi's startup banner in threads. (#2966)
- Agents can snooze a thread until "tomorrow" or "next week", and older Pi threads no longer open with the startup banner. (#2969)
## [v1.7.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.7.0) - 2026-11-03

### Features
- Calmer chat for threads that orchestrate others: a small shield instead of "allowed by" notice cards, readable thread tools ("Started thread", "Waited for 3 threads"), linked threads collapsed into one chip, and a parent's children folded into one sidebar line. (#2947)
- Replies fade in word by word while they stream (Settings › Appearance › Animate streaming text). (#2947)
- A solid composer, the same corner radius on every popover, menu and dialog, a project-named new-thread heading, and skeleton loaders where content loads. (#2947)
- Claude Code plan usage now shows in the composer, refreshed every 3 minutes while the app is open (turn it on in Settings › Usage). (#2947)
- Claude Code, Codex, OpenCode and Pi threads can now answer with inline charts, tables and checklists. (#2948)
## [v1.6.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.6.0) - 2026-11-03

### Features
- The chat has a new layout: "Working for 12s" at the top of a running turn, finished calls grouped as one sentence, "Worked for 1m 3s" dividers and a narrower lane. (#2943)
- The sidebar is one list, with Done/Failed/Approval/Input in each row until you open it, ⌘1–⌘9 to jump, and Mark Unread. (#2944)
- Subagents started together collapse into one "N subagents" row with how many still work. (#2945)
- The context ring fills as context is used, the folder under the composer opens a menu (Finder, Terminal, copy path, new thread), and Claude Code now gets RustCode's reply style and runtime info. (#2942)
## [v1.5.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.5.1) - 2026-11-03

### Features
- Switch an open thread to another harness from the model picker; the new harness gets the conversation so far. (#2937)
- Background work (background agents, commands, turns Claude starts by itself) shows in the thread, and subagents look the same on every harness. (#2936)
- Steer a running Claude Code or Codex turn with images, not only text. (#2938)
- CI: releases build the desktop app alongside the engine and build once instead of twice. (#2934, #2935, #2940)
## [v1.5.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.5.0) - 2026-11-03

### Features
- Switch an open thread to another harness from the model picker; the new harness gets the conversation so far. (#2937)
- Background work (background agents, commands, turns Claude starts by itself) shows in the thread, and subagents look the same on every harness. (#2936)
- Steer a running Claude Code or Codex turn with images, not only text. (#2938)
- CI: releases build the desktop app alongside the engine and build once instead of twice. (#2934, #2935)
## [v1.4.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.4.0) - 2026-11-03

### Features
### New harness
- Pi is a built-in harness: Settings › Harnesses finds an installed `pi`, and threads can run on it with pi's own models, logins and skills. (#2932)

### A calmer chat
- While a turn runs, the chat shows what it is doing now and folds its earlier steps into one row. The header's status button is icon-only, and the composer no longer cuts off the bottom of the text you type. (#2930)
- Claude Code subagents show as one live row each, with the step they are on and their step count and time when done, instead of every step in the chat. Hover "N agents working" over the composer to see what each is doing. (#2925)
- Chat rows no longer jump or overlap while a reply streams or while you scroll. (#2927)

### Usage
- Plan usage and context show how much you have left: the bars and the context ring drain as you use them, in the composer and in Settings › Usage. (#2926)
- Threads on the Codex harness show your ChatGPT plan's usage in the composer, like Settings › Usage. (#2928)

### MCP
- Claude Code and Codex threads can use remote MCP servers you signed in to in RustCode; the login is passed to them. (#2929)
- Opening an MCP server in Settings › Capabilities no longer crashes the app. (#2931)

### Fixes
- ⌘R reloads the view without stopping your threads, and a view that fails to render offers Reload view instead of a blank window. (#2931)
- The checks pill under a turn counts only real test, lint and build runs, not commands that mention a test file. (#2927)
## [v1.3.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.3.0) - 2026-11-03

### Features
### Threads keep going
- Threads that were working when you quit, updated or restarted the engine continue on their own when the app is back (Settings › General › Continue threads after a restart). (#2917)
- A thread waiting on background commands says so, and when they finish and it carries on, it shows as working again with a note. (#2921)
- A thread that will not open can be repaired: Repair Thread in its menu, or the button on the "Could not open this thread" error. (#2918)
- Threads another thread started fold away under their parent once they finish; "N finished" shows them again. (#2919)

### Look and feel
- The app uses a rounder font (Settings › Appearance › Rounded font), and new messages, finished dots and segmented controls animate in. (#2916)
- The composer is darker, links in chat are pills, sending scrolls to your message, and the context ring is back and opens on hover. (#2915)
- Notifications show one at a time under the header and leave on their own: +N clears the rest, and a swipe, Escape or × dismisses. (#2922)
- Native tooltips and macOS Writing Tools are off, and menus are rounder. (#2922)
- ⌘+, ⌘- and ⌘0 make the app's text larger, smaller or actual size, and the level is kept. (#2920)
- The composer tray no longer repeats the harness, and a turn's checks show as a small pill at the left. (#2923)

### Fixes
- Settings › Usage shows ChatGPT plan usage from the Codex CLI when the app is opened from the Dock, and says when the CLI is missing or failed instead of "Not signed in". (#2914)
- A running thread's folder can't be changed by mistake: the folder button is disabled and ⌘O says why. (#2915)
- Settings shows the app's real version at once and checks for an update when opened. (#2912)
- Releases no longer publish the old SwiftUI app's update zip. (#2913)
## [v1.2.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.2.0) - 2026-11-03

### Features
### Delegation and harnesses
- Threads can start and message subagent threads out of the box; under Ask you still approve each one. (#2908)
- Codex threads can edit files in any folder, not only ones Codex already trusts. (#2906)
- Codex threads show when Codex starts, messages or waits for its own subagents. (#2909)
- A thread on any harness runs with the MCP servers from Settings › MCP Servers. (#2897)
- Settings › Harnesses can turn the built-in RustCode harness off once another harness is the default. (#2904)
- Settings › General has a Default access row, and a Full-access thread delegates without a prompt, with the delegation recorded in its transcript. (#2889)

### Fixes
- Moving a Claude Code or Codex thread to another folder with ⌘O no longer loses the chat or switches it to the RustCode harness. (#2900)
- The project folder menu under the composer no longer opens behind the message box. (#2901)
- Delegating in the same folder no longer warns about another project, Codex and Claude Code turns no longer say "No files changed", titles end on a word, and the composer names the agent you are talking to. (#2905)
- The effort slider puts Off at the left and Max at the top. (#2884, #2903)
- Arrow Up takes the latest queued message back into the composer, skipping queued replies to other threads. (#2888, #2903)
- Settings shows download progress while an update installs, a failed update check retries sooner, and a failed manual check says so. (#2894, #2903)
- Opening Settings or a new thread closes any popover, menu or context menu left open. (#2886)
- The app publishes Discord Rich Presence again, with a switch in Settings. (#2893)

### Look and feel
- The window reopens at the size it was closed with. (#2892)
- The context meter under the composer reads as a gauge instead of a loading spinner, and its tooltip gives the percentage used. (#2907)
- Menus, popovers and the composer are near-solid with a lighter shadow, so text behind them no longer smears through. (#2883)
- Tool-call rows hug their content. (#2885)
- Settings uses a wider content column, and the sidebar's section and project headers are one type step larger. (#2890, #2891)
- Folders in /tmp show without the /private prefix. (#2910)
## [v1.1.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.1.1) - 2026-11-03

### Features
- Codex threads can run MCP tools again, including delegating to other harnesses; other servers' tools ask with an approval card instead of failing. (#2855)
- Choose Ask, Auto-edit or Full access under the composer to stop approving every command. (#2856)
- Tidier chat: no stray copy icons, readable commands, failure reasons on the row, and approval bars that keep long commands inside. (#2857)
- Lighter shadows across the app, and the chat now slides along when the sidebar opens or closes. (#2858)
- ⌘⏎ steers a running Claude Code turn, and queued messages have an Edit button. (#2859)
## [v1.1.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.1.0) - 2026-11-02

### Features
- The update check tries again when it fails and when the app comes back to the front, and Settings shows why a check failed. (#2838)
- Larger text across the app, a rounder composer, bigger pasted-image thumbnails and a lighter disabled Send button. (#2846)
- Inline code has a background, links show the pointer and a hover state, long tool rows truncate, and replies have one copy button. (#2847)
- Pictures in the chat can be zoomed with a pinch, ⌘-scroll, double-click or + and −, and panned by dragging. (#2848)
- Threads that finish while you are elsewhere show in a Finished section with a green dot until you open them. (#2849)
- Updates show a download progress bar, and the release notes open after every update. (#2850)
- The Mac app gives a light trackpad tap when you send, stop a turn or rest on Send (Settings › General to turn off). (#2851)
- Settings › Usage shows the 5-hour and weekly limits of your Claude and ChatGPT plans in one place. (#2852)
## [v1.0.0](https://github.com/LHagfoss/rustcode/releases/tag/v1.0.0) - 2026-11-02

### Features
RustCode 1.0 is one app for your coding agents: run Claude Code, Codex, Gemini CLI, OpenCode or RustCode's own agent side by side, each thread on the agent, model and effort you pick. The installer adds the app and the `rustcode` command.

### The app
- The new desktop app replaces the SwiftUI app: native installers for macOS, Windows and Linux, 120 Hz rendering, and a darker, calmer design with themes shared with the terminal. (#2788, #2791, #2800)
- Worktree threads, thread status, per-turn checkpoints, and threads that start and message other threads on any harness. (#2790)
- The sidebar shows a live clock for running threads, what a waiting thread needs, and Pin and Settle on hover; settling can be undone. (#2809)
- Settings › Harnesses lists each agent's models as soon as you pick it, saves the default as you change it, re-checks each agent when opened, and puts Claude Code first. (#2810, #2829)
- ⌘O changes the folder of the current or new thread; ⌘⇧O sets where new threads start. (#2829)
- RustCode tells you in the sidebar when an update is out, installs it in one click, and shows what changed after each update. (#2830)
- Threads untouched for three days settle on their own, so the sidebar keeps only current work; Settled loads 10 at a time. (#2832)
- Sidebar search finds threads by what was said in them, from a fast SQLite index kept beside your threads. (#2835)

### Chat
- Runs of tool calls fold into one line like "Read 4 files · Ran 2 commands"; the line under a running turn says what the agent is doing. (#2812, #2822)
- Approvals show Allow and Decline up front, with the rarer answers under More. (#2811)
- Click the context ring to see what fills the context window; plan usage opens on hover with a bar per limit and stays current. (#2808)
- /status, /context and other reports open in a panel above the composer; /clear, /model and the thread picker act in the app. (#2815, #2816)
- Copy any reply block, thinking, or tool input and output; pasted links stay where you paste them; messages with images queue whole while the agent works. (#2807, #2814, #2817)
- Escape closes the open popover first, the composer is ready to type when you come back, and slash commands read as one line. (#2813, #2818, #2819)
- Links are highlighted in the composer where you type or paste them. (#2833)
- Compact any thread with /compact or Compact now in the context panel, Codex included; a compaction shows as one quiet row. (#2834)

### The agent
- It edits in place and no longer branches or commits unless you or the project's instructions ask. (#2825)
- Running the script you asked it to fix counts as its verification. (#2827)
- A wait on a long build stops when you stop the turn, and a timed-out wait shows the build's latest output and what to do next. (#2824)
- A thread started on another harness must name one that can run and a model it offers. (#2828)
## [v1.0.0-beta.1](https://github.com/LHagfoss/rustcode/releases/tag/v1.0.0-beta.1) - 2026-11-02

### Features
RustCode 1.0 is one app for your coding agents: run Claude Code, Codex, Gemini CLI, OpenCode or RustCode's own agent side by side, each thread on the agent, model and effort you pick. The installer adds the app and the `rustcode` command.

### The app
- The new desktop app replaces the SwiftUI app: native installers for macOS, Windows and Linux, 120 Hz rendering, and a darker, calmer design with themes shared with the terminal. (#2788, #2791, #2800)
- Worktree threads, thread status, per-turn checkpoints, and threads that start and message other threads on any harness. (#2790)
- The sidebar shows a live clock for running threads, what a waiting thread needs, and Pin and Settle on hover; settling can be undone. (#2809)
- Settings › Harnesses lists each agent's models as soon as you pick it, saves the default as you change it, re-checks each agent when opened, and puts Claude Code first. (#2810, #2829)
- ⌘O changes the folder of the current or new thread; ⌘⇧O sets where new threads start. (#2829)
- RustCode tells you in the sidebar when an update is out, installs it in one click, and shows what changed after each update. (#2830)

### Chat
- Runs of tool calls fold into one line like "Read 4 files · Ran 2 commands"; the line under a running turn says what the agent is doing. (#2812, #2822)
- Approvals show Allow and Decline up front, with the rarer answers under More. (#2811)
- Click the context ring to see what fills the context window; plan usage opens on hover with a bar per limit and stays current. (#2808)
- /status, /context and other reports open in a panel above the composer; /clear, /model and the thread picker act in the app. (#2815, #2816)
- Copy any reply block, thinking, or tool input and output; pasted links stay where you paste them; messages with images queue whole while the agent works. (#2807, #2814, #2817)
- Escape closes the open popover first, the composer is ready to type when you come back, and slash commands read as one line. (#2813, #2818, #2819)

### The agent
- It edits in place and no longer branches or commits unless you or the project's instructions ask. (#2825)
- Running the script you asked it to fix counts as its verification. (#2827)
- A wait on a long build stops when you stop the turn, and a timed-out wait shows the build's latest output and what to do next. (#2824)
- A thread started on another harness must name one that can run and a model it offers. (#2828)
## [v0.115.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.115.0) - 2026-11-01

### Features
- The composer's usage meter shows the 5-hour window, and warns when the weekly window runs low. (#2767)
- An expired or replaced ChatGPT sign-in now says to sign in again with `/login openai` instead of failing with HTTP 400. (#2768)
- The Claude CLI subscription integration is removed. (#2774)
- Add experimental, permission-gated native desktop automation on macOS. (#2775)
- Betas: macOS prereleases can now be cut several times a day as `X.Y.Z-beta.N`. (#2777)
- A subagent's panel is laid out like the chat: slim header, the chat's column and a floating composer. (#2779)
- The app's floating panels, popovers and header buttons use the composer's blur instead of Liquid Glass, with rounder popovers. (#2778)
- Subagents that edit files start again and work in the thread's folder. (#2780)
## [v0.115.0-rc.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.115.0-rc.1) - 2026-11-01

### Features
- Add experimental, permission-gated native desktop automation on macOS (#2775).
- Show the 5-hour window on the composer's usage meter and warn when the weekly window runs low (#2767).
- Ask users to renew expired or replaced ChatGPT sign-ins instead of failing with HTTP 400 (#2768).
- Remove Claude CLI subscription authentication and inference while preserving legacy configuration compatibility (#2774).
## [v0.114.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.114.0) - 2026-11-01

### Features
- Smart UI: the agent now renders its interactive view offscreen and checks a screenshot before it sends it. (#2756)
- Smart UI is now on by default, and the agent shows charts for trends and comparisons without being asked. (#2757)
- Smart UI views can use a bundled Chart.js for charts, with no network access. (#2758)
- Smart UI keeps up to three interactive views running at once instead of stopping all but the last. (#2759)
- Make the new-thread heading more prominent and show helpful tips for RustCode features. (#2734)
- Keep composer text steady when focusing or leaving the input. (#2735)
- Tool approvals show the command or the diff instead of raw JSON, and the sidebar says what a waiting thread needs. (#2736)
- Git steps in a turn read "Staged calc.py" and "Committed “…”" instead of "Ran GitAdd". (#2737)
- MCP tools that report progress often no longer time out partway through a long call. (#2738)
- The turn summary no longer says "Checks passed" when one of several test suites failed. (#2739)
- Opened turns read as narration and steps without repeated thought lines, and in-progress narration no longer stacks timestamps. (#2740)
- The context and plan-usage meters are now level bars that can't be mistaken for spinners, and the thread title stays readable with a side panel open. (#2741)
- Settings descriptions wrap instead of being cut off mid-sentence. (#2742)
- Clicking Branch again in the Changes panel keeps the base branch you picked. (#2743)
- The empty sidebar no longer mentions "this project" while showing all projects. (#2744)
- The Changes panel offers the branch's changes when nothing is left uncommitted. (#2747)
- `rustcode mcp add --force` keeps a server's `timeout_secs`. (#2750)
- The turn summary flags failed commands and stopped checks again. (#2751)
- Stopping a turn or timing out an MCP tool call now tells the server to stop that work. (#2752)
- Tool rows no longer show names like `GitStatus` as their detail. (#2754)
- A finished turn no longer looks failed when one call failed along the way. (#2755)
## [v0.113.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.113.0) - 2026-10-31

### Features
- MCP tool calls can run past 30 seconds: servers that report progress keep the call alive, and `timeout_secs` raises the limit per server. (#2724)
- A new chat's context meter no longer reads near zero before the first reply. (#2726)
- Answers cite code with full repo-relative paths. (#2728)
- A thread in a deep folder no longer fills the header with its full path. (#2730)
- A new thread's folder menu lists recent project folders, so starting a thread in one skips the file dialog. (#2732)
## [v0.112.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.112.2) - 2026-10-31

### Fixes
- The summary under a turn is one line with the changed files, their lines and how checks and git steps ended; buttons and switches across the app match Settings, and the sidebar's project card floats over the list. (#2720)
## [v0.112.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.112.1) - 2026-10-30

### Fixes
### Mac app
- Showing or hiding the sidebar no longer stutters: the new-thread banner is drawn once into a picture instead of on every frame, and chat rows stay quiet while the chat slides. (#2717)
- Switching the sidebar between All and Project shows that list at once, then refreshes it. (#2717)
- The title bar's panel buttons (pull requests, terminal, changes, preview) share one glass group, its buttons fade on hover, and the folder menu gets a hover highlight. (#2717)
- About shows what's new in this version, with a link to the full changelog, plus your thread, project, pinned and archived counts and system details you can copy for a bug report. (#2717)
- The summary under a reply says what each command did, such as "Committed “message”", "Pushed to branch" or "Ran tests (cargo)", instead of the raw shell line; a pass is a small check and a failure a red badge, each changed file shows its own +/−, and the full command opens on click. (#2718)
- The chat's chrome uses a rounder font, and monospace is kept to code, commands and paths. (#2718)
## [v0.112.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.112.0) - 2026-10-30

### Features
### Pull requests
- A Pull Requests panel lists the project's pull requests with checks, reviews, files and activity, and starts a thread about one. (#2702)
- Draft line comments and a review on a pull request, posted only when you submit. (#2706)
- Watch a pull request from a thread and hear when checks fail or pass, reviews arrive, conflicts appear or it closes. Nothing is done on the pull request. (#2711)
- A review guide for a pull request: stops in reading order, each linked to its changed lines, with notes and an Ask button. (#2712)

### Threads
- Threads can message each other: an agent can ask another thread in its project and wait for the answer, with your permission. Off by default; turn it on in Settings ▸ General. (#2714)
- The first reply in a thread starts sooner: MCP servers start while you type. (#2695)
- A thread that works mostly in another folder offers to move there. (#2697)
- Stopped turns show a small "Stopped by you" line, and the raw "turn stopped" marker no longer shows as a reply. (#2692, #2698, #2713)
- Text in the chat can be selected across messages by dragging, and copied in one go. (#2704)

### Mac app
- The new thread banner fills the top of the chat, fades away when you send, shows at once, and follows the window live while it is resized. (#2693, #2694, #2707, #2709, #2710)
- The glass title bar is back, with the sidebar's buttons in one place in new threads, threads and Settings. (#2708)
- Settings and threads share one sidebar toggle (⌘B), and showing or hiding the sidebar no longer stutters in long threads. (#2710)
- The composer's footer sits on a tray, its controls get hover and rounder shapes, and chat text no longer shows through under it. (#2693, #2694, #2705)

### iPhone
- Review a thread's uncommitted changes from the phone and ask about selected lines. (#2715)

### Storage
- `rustcode storage clean` lists command output and attachments no thread uses, and `--apply` moves them aside for 14 days. (#2701)
- Old thread layouts and leftover scratch folders are tidied once after an upgrade instead of on every start. (#2703)
## [v0.111.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.111.0) - 2026-10-30

### Features
- Fix a final answer that sometimes appeared twice when streaming stopped partway. (#2654)
- Esc clears the composer draft before it stops a turn; finished agents leave the sidebar; Settings model, effort and tool-call controls redrawn. (#2656)
- Agents of other chats no longer disappear from the sidebar when one refresh fails. (#2657)
- The Agents list opens as a composer panel matching the + and model panels. (#2658)
- Settings has a search field that finds the section holding a setting. (#2660)
- Smart UI guidance now loads on demand from a built-in `smart-ui` skill, trimming the root prompt. (#2659)
- Mark threads done or snooze them onto sidebar shelves, and group threads that need you, are working or are ready to read at the top. (#2662)
- The agent's task plan has stable step ids, a blocked state and targeted updates, and rejects invalid values. (#2661)
- ⌘⇧⏎ starts a message as a new thread in the background and keeps the composer free for the next task. (#2663)
- A read-only Changes panel shows the project's uncommitted or branch changes as a file list with diffs. (#2664)
- A turn summary under each finished answer lists the files changed and the checks run, with how each ended. (#2665)
- Long single-turn sessions compact deeper and far less often, including old edit payloads. (#2668)
- A slow provider start (for example right after a compaction) is retried with a longer wait instead of stopping the turn. (#2670)
- Threads are now stored in `~/.config/rustcode/threads/`; the first start moves the old `sessions/` folder there and leaves a link behind. (#2671)
- `rustcode threads` replaces `rustcode sessions` (still accepted as an alias). (#2672)
- The macOS app calls conversations threads. (#2673)
- The iOS app calls conversations threads. (#2674)
- The terminal UI calls conversations threads. (#2676)
- An inbox in the sidebar lists approvals, questions, stopped turns and unread replies from every thread. (#2680)
- The iPhone app and the Mac talk about threads on the wire; older apps and Macs keep working. (#2679)
- Pick diff lines in the Changes panel and attach them to your message as a question or a change request. (#2681)
- New threads open with the composer centered under an optional wide banner picture (drawn as colored text or as is, set in Settings), and the composer slides down when the first message is sent. (#2683)
- The subagent panel shows the agent's tokens, tool calls and requests, and opens long agent conversations without lag. (#2682)
- ⌘O in an open thread changes that thread's folder instead of starting a new thread. (#2684)
- The subagent panel shows each agent's brief: who started it, where it works, what it may write, and whether its check actually passed. (#2685)
- Threads with a multi-step plan show a progress button that opens the plan; a step jumps to where the agent last updated it. (#2686)
- A turn's summary shows its commit, push, pull request and CI steps, each as its command actually ended. (#2687)
- A thread opening with only its latest message drawn until scrolled is less likely: the app keeps placing its older rows until they report. (#2688)
- `rustcode storage usage` shows how much space each kind of RustCode data takes, without changing anything. (#2690)
## [v0.110.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.110.0) - 2026-10-29

### Features
- Settings' sidebar now looks like the chat sidebar, with a simpler Back button, and always opens on General. (#2619)
- Tool calls from models that write Qwen-style `<function=…>` XML now run instead of failing as malformed. (#2617)
- When Smart UI is off and you ask for it, the agent tells you where to turn it on instead of printing raw HTML. (#2618)
- Each model now keeps its own tool-call protocol even when several share a server, and the app can change it. (#2621)
- New Model & Effort picker with a smooth effort slider and labelled Fast mode, a tool-call protocol choice for your own models, and a composer footer with project, files and agents. (#2622)
## [v0.109.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.109.0) - 2026-10-29

### Features
- The Settings page centers its content, has a clear Back button to the chat you came from, and opens without an animation. (#2606)
- Holding ⌘W in the terminal panel closes only the focused terminal, and the panel closes only with the last one. (#2607)
- The chat now comes back by itself when the subagent you are reading finishes. Ended agents stay listed under their chat, and returning to a long chat after reading an agent keeps it at its latest messages. (#2610)
- Add `rustcode storage status` and a versioned user-data migration checkpoint that later releases use to upgrade stored data once on first start. (#2609)
- Fix a near-full chat dropping its oldest turn on several requests in a row instead of compacting. (#2612)
- The macOS composer now has one Model & Effort picker, which also holds a labeled Fast mode switch for ChatGPT-plan models, including on a new chat before its first message. (#2611)
- When Fast mode is on and a response is served at the default tier instead, RustCode now says so once instead of only logging it. (#2615)
- Finished replies in the macOS chat render in a native text view, the groundwork for selecting text across messages. (#2614)
## [v0.108.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.108.0) - 2026-10-28

### Fixes
- Local Liquid AI (LFM) models' tool calls now run instead of being dropped, and textual tool prompts no longer inline every MCP schema. (#2588)
- Fix a subagent hang when cancelling during a tool call; subagents now stop at configurable round and time limits and return their partial findings when they fail. (#2590)
- MCP tools no longer churn in the request mid-session, which keeps the provider prompt cache warm. Tool output that names MCP servers no longer pulls their tools in, and the agent is told where session logs live. (#2589)
- Compacting a chat now actually shrinks a long single turn, updates the context meter, and shows a compaction card with the summary in the macOS chat. (#2591)
## [v0.107.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.107.0) - 2026-10-28

### Features
- Subagents in the app no longer hang on approvals you could not see. Stopping one now reports when it has ended, and a child that has stopped making progress is flagged. (#2566)
- Subagents and git status can work in another directory, and the agent suggests moving a session that keeps working outside its workspace. (#2567)
- Title and compaction requests no longer leave empty folders in the session store, and existing ones are cleaned up. (#2568)
- No more false loop warning when several different parallel calls return the same empty result. (#2569)
- Binary MCP tool results are saved to the session's artifacts folder and shown as a path with a short preview instead of base64. (#2570)
- Recap requests no longer leave empty folders in the session store, and existing ones are cleaned up. (#2572)
- Working subagents stay in the sidebar under their chat when you switch chats, the chat's row takes you straight back from a subagent, and the subagent view has a full composer. (#2573)
- Scrolling up while a reply streams no longer jumps back to the end when the pointer is beside the composer, and the agents-working pill sits in the composer instead of over the chat. (#2574)
- The plan-usage hover panel now shows above the composer, and the composer has no border and animates as one piece. (#2575)
- The chat, composer and timeline adapt to narrow windows: the timeline stays visible in a slimmer form and the composer's controls stay on one line. (#2578)
- Settings now opens as a full page with its own sidebar instead of a popup over the chat, and Esc or Back returns to the chat as you left it. (#2579)
- MCP tools you named or used stay available after a long chat is compacted, and unrelated tools no longer slip in on common words like "chat" or "message". (#2580)
- Compacted chats no longer carry stray code fragments as modified files or status messages as project rules, and the session log no longer reports skipped compactions as compactions. (#2581)
- New fast mode for ChatGPT-plan models: `/fast` or the bolt beside the effort pill asks for faster responses, which use your plan's usage faster. (#2583)
- Provider errors now show the provider's own message with its HTTP status and profile instead of raw stream diagnostics. (#2585)
## [v0.106.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.106.0) - 2026-10-28

### Features
- Fix agent rows in a reopened chat staying on "Starting agent…" after the agent finished. (#2537)
- Show an interactive view written while Smart UI is off as a card with a Turn On Smart UI button, instead of its HTML. (#2538)
- Show What's New centered over the window, to be closed before going on, like Settings. (#2539)
- Make a failing MCP tool discovery call say how to fix it, so the agent stops retrying and does not report the server's tools as missing. (#2541)
- Smoother chat while a reply with links is streaming. (#2542)
- Fix MCP tools with optional arguments failing on OpenAI/ChatGPT models, which retried a rejected value until the turn stopped. (#2544)
- Refresh a signed-in model account's model list from Settings › Connections. (#2546)
- Show your ChatGPT plan's 5-hour and weekly usage in the composer and the context panel, read from a signed-in Codex CLI. (#2547)
- Compaction on ChatGPT and Claude plan models now writes a model handoff summary instead of a keyword record. (#2549)
## [v0.105.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.105.0) - 2026-10-28

### Features
- The macOS app can send a message to another chat without switching to it (+ › Send Message to Another Chat…). (#2534)
- The macOS app can add MCP servers (Settings → MCP Servers → Add Server) and remove them (right-click), and they start right away. (#2523)
- MCP servers added or removed while a session runs now take effect on the next message, and MCP tool search no longer fails with "invalid argument 'summary'". (#2522)
- MCP tool discovery shows each small server's tool names even next to large servers, accepts `select:` queries, and starts a configured server that isn't running when asked for its tools. (#2529)
- The message box now scrolls through long drafts with the mouse wheel or trackpad and keeps the cursor in view. (#2532)
- Quotes in replies now show as one tidy quoted block instead of a bar glyph on every line. (#2533)
- The ⌘K chat switcher now opens closer to the middle of the window and fits compact windows. (#2531)
- The What's New card now appears after updating from versions older than 0.104.0. (#2530)
- Fixed chats sometimes opening with only their latest message visible. (#2524)
## [v0.104.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.104.0) - 2026-10-28

### Features
- The macOS app now shows a short What's New card after it updates, with the full release notes one click away. (#2518)
- Dragging from the blank space beside a message now starts selecting its text. (#2519)
- Project memory now matches whole words, skips filler words, flags stale and conflicting facts, and is saved and read from the same project folder in every session. (#2513)
- The archive button in the sidebar no longer shows a count badge. (#2517)

### Fixes
- Fixed long sessions losing old messages and their prompt cache every round when the context was nearly full. (#2512)
- Fixed the agent re-listing MCP tools on every turn after it had already found them. (#2514)

### Performance
- The macOS app no longer starts every MCP server for chats that never run a turn; servers start on the first message. (#2515)

### Diagnostics
- Session logs name the engine version, subagent requests log their cost and cache use, pending background commands no longer count as failed tools, and memory selection is logged without fact contents. (#2509, #2511, #2513)
## [v0.103.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.103.0) - 2026-10-27

### Features
- macOS: chats idle for a week are archived automatically, and chats archived for a month move to the Trash; pinned chats are never touched. (#2493)
- macOS: working subagents appear as nested chats under their parent in the sidebar and open in the main chat area; the agents pill no longer overlaps the composer. (#2494)
## [v0.102.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.102.0) - 2026-10-27

### Features
- Compaction keeps the goal, constraints, decisions and next steps through repeated passes without the summary growing each time. (#2486)
- Experimental Smart UI (opt-in in Settings): the agent can reply with small interactive views that run in a sandbox with no network, file or app access. (#2487)
## [v0.101.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.101.0) - 2026-10-27

### Features
- Settings has a Connections page: connect GitHub through the GitHub CLI with a one-time code, and see or sign out of your GitHub and model accounts. (#2479)
- The agent can read GitHub issues, pull requests, diffs, CI checks and failed run logs through your GitHub CLI sign-in, without an approval prompt. (#2480)
- Opening a turn's steps near the top of a chat no longer leaves blank space below them, and steps now fade in as a turn or tool run opens. (#2482)
## [v0.100.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.100.0) - 2026-10-27

### Features
- ↑ in an empty composer now edits the newest queued message first. (#2473)
- Replies are written out at a steady pace instead of in bursts, and tables fade in as they are written. (#2474)
- Tool calls show icons, including the site icon for pages read; sending a message and new chat rows animate into place. (#2477)
- A long turn no longer stops with "context budget exceeded": old tool output in the turn is shortened so it can keep working. (#2476)
## [v0.99.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.99.0) - 2026-10-27

### Features
- The Mac sidebar toolbar puts the sidebar toggle first, then Archived, then New Chat. (#2464)
- The Mac chat shows when each message was sent and each reply written, with Copy always visible below every message. (#2465)
- The Mac app's update button floats over the chat list instead of taking a row above the project bar. (#2466)
- Mac chat links show the pointing hand, the full address on hover and Copy Link on right-click, and bare URLs read as a short host and path. (#2467)
- Esc clears a text selection in the Mac chat. (#2468)
- Tool calls in the Mac chat show as compact single lines, and an opened turn's work can be hidden from a bar that stays at the top while you read it. (#2469)
- The Mac app can move an open chat to another project folder from the title bar's folder menu. (#2470)
- New text in a streaming Mac reply fades in again, as the Streaming text setting intends. (#2471)
## [v0.98.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.98.2) - 2026-10-26

### Fixes
- Move Archived into the sidebar toolbar and unify the sidebar footer background. (#2452)
- Align chat-link cursor hit areas with rendered text and normalize underline thickness. (#2453)
- Checking a finished background task returns the end of its output instead of resending all of it. (#2456)
- The Mac app again shows every message and round of a running turn, and the header's context meter updates after each round. (#2454)
## [v0.98.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.98.1) - 2026-10-26

### Fixes
- Show one Archived control beside the sidebar heading, line chat titles up with state shown at the row's end, and make the project folder look like a button. (#2446)
- Put the Copy button below messages, clear of their text, and clear a chat text selection by clicking blank space. (#2447)
- Show the pointing hand reliably over chat links, including in tables, and draw link underlines lighter so bold and heading links no longer look heavy. (#2448)
- Keep your place while scrolling back through long chats: older history now loads without jumps, flashes or the view leaping toward the end. (#2449)
## [v0.98.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.98.0) - 2026-10-26

### Features
- The sidebar's divider now lines up with the sidebar's edge. (#2436)
- Tables with short separators, and code blocks inside list items, now render properly in the macOS chat. (#2437)
- macOS: a working turn shows one step at a time; a finished turn shows just its answer under a `Worked for …` row that opens to every step on a rail. (#2438)
## [v0.97.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.97.0) - 2026-10-26

### Features
- Show the reasoning effort as `xHigh`, not `Xhigh`. (#2419)
- Show the context/usage meter from the start of a session, not only after the first turn ends. (#2420)
- Make ⌘⏎ steer the running turn immediately instead of queueing. (#2421)
- Recall the last message with ↑ in the macOS composer to edit and resend it. (#2422)
- Stop the message Copy button flickering when the pointer moves onto it. (#2423)
- Give the macOS chat more horizontal room on wide windows. (#2424)
- Make the macOS sidebar resizable and remember its width. (#2425)
- Move the macOS sidebar's Archived control up beside the Chats heading instead of a detached row. (#2426)
- The chat you are working in moves to the top of the sidebar as soon as its turn starts, not only when it ends. (#2427)
- The pointer turns into a pointing hand over links in the chat. (#2428)
- macOS: while a turn runs, its current round shows each tool call with its status and output, and finished rounds fold into one row you can open. (#2429)
- New chats get a short, generated title from your first request, in the macOS app and the terminal, without delaying the reply. (#2430)
## [v0.96.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.96.0) - 2026-10-25

### Features
- macOS: a working turn reads like a conversation: the model's text, one row per group of actions, and one live line saying what it's doing; a finished turn folds to `Worked for …`. (#2408)
## [v0.95.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.95.1) - 2026-10-25

### Fixes
- macOS: Check for Updates works again; it no longer hits GitHub's API rate limit. (#2406)
- TUI: the effort picker offers every level the model lists, such as xhigh and max. (#2405)
## [v0.95.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.95.0) - 2026-10-25

### Features
- macOS: a running turn shows every step of its pipeline, each finished call as one line, instead of folding finished rounds. (#2402)
- macOS: quieter tool rows, and each pipeline step marked with a tinted icon for its kind (message, thought, read, edit, run, MCP…), red when failed and pulsing while running; thoughts are one line. (#2403)
## [v0.94.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.94.1) - 2026-10-25

### Fixes
- Folded tool steps in the Mac chat name a lone call directly, and a folded edit shows its file and added/removed lines. (#2398)
- The Mac chat's turn timeline shows one activity per thought, a quieter first reasoning line, file names and line totals on grouped edits, and project-relative paths on Read cards. (#2399)
- Fixed the Mac chat sometimes going blank after switching from a long chat and scrolling. (#2400)
## [v0.94.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.94.0) - 2026-10-25

### Features
- macOS: at Low/Edits verbosity a finished turn shows its first message, one `N tool calls · M steps` row, and its answer. The row opens to the turn's steps as a pipeline: what the model said, its thoughts and each run of calls on one rail. A running turn shows only its current round, and a turn you opened while it ran folds back when it ends. (#2363, #2377, #2379, #2381, #2385, #2387, #2394)
- macOS: a preview's screenshots stay inside the turn instead of appearing as your message. (#2387)
- macOS: a turn stopped by a provider or connection error ends with a Continue button. (#2375)
- macOS: a chat picked right after launch opens instead of showing empty. (#2383)
- macOS: with the sidebar hidden, the session timeline sits at the window's left edge. (#2371)
- Preview: the agent sees the page's runtime errors and a desktop-width screenshot, and stops waiting when nobody answers its access request. (#2392)
- TUI: at Low verbosity, what the model says before each tool call shows as one sentence; the reply stays whole. (#2365)
- Turns on a ChatGPT login no longer stop when the connection to OpenAI drops before a reply starts; the request is retried. (#2367)
- The Mac no longer idle-sleeps while RustCode is working on a turn. (#2373)
- Long sessions compact their history instead of silently dropping old messages when the context fills. (#2369)
- A tool's screenshot is never taken for your request when history is compacted. (#2396)
## [v0.93.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.93.0) - 2026-10-24

### Features
- A dropped provider stream during the model's text no longer stops the turn; it continues on its own. (#2351)
- Edits whose line numbers are slightly off no longer fail when they add lines around existing code. (#2347)
- The model can use the localhost preview again; calls no longer fail with "unknown argument `summary`". (#2349)
- After compaction the model keeps the latest test and build results and no longer sees already-fixed errors as open. (#2353)
- At Low and Edits tool verbosity the macOS app shows a turn's first thought only, so long turns are much shorter. (#2358)
- The macOS session timeline sits centred beside the chat and its ticks are wider. (#2356)
- The first request of each new turn reuses the provider's prompt cache instead of re-reading most of the conversation. (#2355)
- At low and edits verbosity the terminal shows a turn's first thought only, so long turns are much shorter. (#2359)
## [v0.92.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.92.2) - 2026-10-23

### Fixes
- The agent can now open the live preview in the Mac app; before, it was told about the tool but never given it. (#2341)
- Chats with screenshots send much smaller requests. (#2342)
- The agent no longer runs a failing TypeScript check before a project's dependencies are installed. (#2343)
- The session timeline is now a compact stack of ticks, with the turn being read drawn widest. (#2338, #2340)
- The copy button grows to fit "Copied" instead of always reserving that width. (#2337)
- Long replies are no longer folded behind Show more; only your own long messages are. (#2339)
## [v0.92.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.92.1) - 2026-10-23

### Fixes
- A remote session left waiting on an approval after the phone disconnects now stops after 10 minutes instead of running forever, and never overwrites history saved by the app in the meantime. (#2331)
- The iPhone app says whether a saved session is open in the RustCode app or a terminal on your Mac, instead of always saying a terminal. (#2332)
- Pairing the same iPhone again replaces its device record instead of adding another, and `rustcode remote devices` shows when each device was last seen. (#2333)
- Subagents now run on the parent's exact model profile and get the parent's tools (web search, MCP, skills); a bad `model` fails at spawn with a short list of valid choices. (#2335)
## [v0.92.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.92.0) - 2026-10-23

### Features
- macOS: the terminal opens in the folder of the chat on screen, ⌘O chooses the folder for this chat, and ⇧⌘O chooses the default project without moving open chats. (#2313)
- Fix `/compact` renaming the chat (or hiding it from the session list), most visibly in the macOS app. (#2314)
- Changing the model or reasoning effort in the TUI (or in one chat) no longer changes the default model new macOS chats start with. (#2315)
- Restore the compact three-row composer and put the model, mode and approval back in the footer, with cleaner narrow-width degradation. (#2316)
- macOS: click an image in the viewer to zoom it to half the window's width and click again to fit; the zoom buttons are gone (⌘+, ⌘−, ⌘0 and pinch still work) and Copy and Close are round buttons with hover and press feedback. (#2317)
- Thinking blocks label OpenAI reasoning summaries as "Reasoning summary" (live and in reopened chats), keep "Thought for Ns" for real reasoning, and never show empty or relabelled-prose thinking blocks. (#2318)
- iOS: copy a session's ID from the session list's long-press menu, the chat's More actions, or a VoiceOver action. (#2319)
- The iPhone no longer opens a chat a second time while it is open in the Mac app or a terminal, and it explains why. A chat that fails to load shows the reason with Try Again instead of a blank screen. (#2320)
- The Mac app shows how long each turn took and when it finished, counts up while working, adds a Copy button under replies, opens message menus on right-click over text, and underlines links. (#2321)
- The chat no longer jumps back to earlier turns when you scroll up while a reply or tool calls are coming in. (#2322)
- macOS: long chat messages fold to 12 lines behind Show more / Show less. (#2326)
- macOS: a timeline beside the chat marks each of your messages, highlights the one you're reading, and jumps to a turn when you click its dot. (#2328)
- Subagents now appear in the chat where they were started, with live status, and a click opens one. While agents work, a small pill over the composer shows how many; the agents strip above the composer is gone. (#2329)
## [v0.91.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.91.0) - 2026-10-22

### Features
- The mode and model sit inside the chat input again, padded above and below. (#2296)
- Tool calls say what they do in the app and terminal, and the app folds every run of calls, a lone call included, under a row naming the tools. (#2297)
## [v0.90.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.90.1) - 2026-10-22

### Features
- v0.90.0 was tagged but never published: its release build failed. This release carries everything listed below.
- The macOS app can start subagents again. (#2282)
- Typing right after opening the model picker filters models instead of the sidebar's chat search. (#2283)
- Diffs in the macOS app show one line-number column, GitHub style, instead of paired old/new numbers. (#2284)
- The TUI shows mode, approval, model and reasoning effort in the footer, and the completion popup lines up with the composer. (#2285)
- The macOS app lists a chat's subagents above the composer; open one to read its conversation, send it instructions, or cancel it. (#2286)
- A model that rejects images despite its profile no longer fails every later message: older images are described by the vision model or left out. (#2287)
- TUI verbosity: low folds runs of tool calls into one clickable `N tool calls` row and shows only the running call; edits does the same but keeps file diffs open. (#2288)
- The macOS app folds tool calls by verbosity like the terminal: low shows one "N tool calls" row and only the running call, edits keeps file diffs open, set in Settings. (#2289)
- In the macOS app, a chat no longer stays busy while a background command it started runs; the reply continues when the command finishes. (#2290)
- The macOS app's message menu has Retry, which re-runs a message in place (after confirming when later messages would be removed) instead of appending a duplicate. (#2291)
- The macOS chat input suggests the next word as you type (Apple's on-device predictions; Tab accepts), with a switch in Settings. (#2292)
## [v0.90.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.90.0) - 2026-10-22

### Features
- The macOS app can start subagents again. (#2282)
- Typing right after opening the model picker filters models instead of the sidebar's chat search. (#2283)
- Diffs in the macOS app show one line-number column, GitHub style, instead of paired old/new numbers. (#2284)
- The TUI shows mode, approval, model and reasoning effort in the footer, and the completion popup lines up with the composer. (#2285)
- The macOS app lists a chat's subagents above the composer; open one to read its conversation, send it instructions, or cancel it. (#2286)
- A model that rejects images despite its profile no longer fails every later message: older images are described by the vision model or left out. (#2287)
- TUI verbosity: low folds runs of tool calls into one clickable `N tool calls` row and shows only the running call; edits does the same but keeps file diffs open. (#2288)
- The macOS app folds tool calls by verbosity like the terminal: low shows one "N tool calls" row and only the running call, edits keeps file diffs open, set in Settings. (#2289)
- In the macOS app, a chat no longer stays busy while a background command it started runs; the reply continues when the command finishes. (#2290)
- The macOS app's message menu has Retry, which re-runs a message in place (after confirming when later messages would be removed) instead of appending a duplicate. (#2291)
- The macOS chat input suggests the next word as you type (Apple's on-device predictions; Tab accepts), with a switch in Settings. (#2292)
## [v0.89.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.89.0) - 2026-10-22

### Features
- **macOS:** ⌘⏎ steers a running turn: the message is read after the current step instead of waiting for the turn to end. ⏎ still queues, and ⇧⏎/⌥⏎ start a new line. With an empty composer, ⌘⏎ sends the oldest queued message now, and each queued message has a Send Now button. A steer the turn ends before reading runs next as its own turn. The engine gains `_rustcode/steer` for ACP clients, with unread steers returned in `_meta.rustcode.unusedSteers` (#2268, #2271).
- **macOS:** images open in an in-app viewer instead of Quick Look or the browser: chat images and images waiting in the composer, fitted to the window, with pinch, ⌘+/⌘−/⌘0 and double-click zoom, Copy Image, and Esc or a click beside the image to close. Only the file's name is shown (#2272).
- **macOS:** after each turn its chat moves to the top of the sidebar, which lists loaded chats by latest activity (#2266). The running spinner and the waiting-for-answer badge move to the leading edge of sidebar rows, and a chat that finished while another was open shows a blue dot until opened (#2265).

### Fixes
- **macOS:** scrolling stays smooth while a reply streams. The window no longer recomputes its draggable region on every frame (it now moves from its top bars), and scroll geometry no longer redraws the whole chat view; main-thread time while streaming drops by about 40% (#2270).
- **macOS:** each chat keeps its own unsent draft, including attachments, instead of the draft following you into another chat (#2264).
- **macOS:** ⌘W closes the active terminal tab, and the panel only with its last tab (#2263). Switching chats closes the terminal panel (#2262).
- **macOS:** inline diffs keep a change split by a blank line or lone brace together as one block, removals before additions (#2267).
- Web search tool cards and activity lines show the searched query instead of the word `query` (#2261).
## [v0.88.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.88.1) - 2026-10-21

### Fixes
- **TUI:** the composer is drawn as a borderless panel band instead of a rounded frame, with a blank row between the input and its settings row. The settings row now names the agent mode (`Build` or `Plan`) and always shows how tool actions are handled (`ask`, `auto-approve`, or `read-only` in Plan mode); clicking it cycles the posture. The unframed footer names `Plan · read-only` and `auto-approve` (#2246).
- A `manage_task` `wait` streams the waited task's output tail while it is still blocked, so its card shows progress instead of sitting empty for the whole command (#2249).
## [v0.88.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.88.0) - 2026-10-21

### Features
- **Compaction:** automatic compaction and `/compact` keep the same recent history: a 20k-token tail, capped at 30% of the window and snapped to a complete turn. An earlier summary is carried forward verbatim instead of being re-summarized, so detail no longer drifts away pass after pass. The LLM summarizer runs only at a turn boundary; mid-turn pressure uses the local deterministic record. `/compact` on a single long turn says there is nothing to compact instead of spending a request that changes nothing (#2241).
- **Compaction stats:** the last compaction's trigger, mode, before/after tokens and count reach every frontend through `/context` and `_meta.rustcode.context` v2, with a Compacted history category in the TUI context modal and the macOS context sheet (#2235). The dead prune passes are gone and compaction metrics name the branch that fired (#2234).
- **TUI:** the composer is drawn in a rounded frame with one permission token (`ask`, `auto-approve` or `plan`) and the model on its last row; click the token to cycle it. The footer keeps branch, folder and context usage (#2243).
- A turn that keeps changing the workspace is no longer stopped by the 5M turn token budget: a productive window is renewed, up to a 20M ceiling. Read-only or spinning turns still stop at 5M (#2237).
- `rg`/`grep` finding nothing is reported as "(no matches)" rather than a failed command; `rg -rn` gets a note that `-r` means `--replace`; `replace_string_in_file` and `str_replace` resolve to `replace_file_content` (#2242).
- A finished background task is recorded in history once, answering its call, instead of also as orphan prose sent to the model twice (#2244).
- **macOS:** chat cards stop spinning when their turn ends or the engine goes away, and empty running cards are not drawn (#2239). Diff gutters print an unchanged line number once (#2238). Tables accept a column name in `sort_by`, and an unknown sort column no longer drops the table (#2240).
## [v0.87.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.87.0) - 2026-10-21

### Features
- **macOS:** Settings sets a default project folder for new chats, and a single chat can start in another folder from the title menu, the + menu, File > New Chat in Folder… or a sidebar chat, without changing the default (#2220).
- **macOS:** in-chat visualizations are discoverable: `/visualize`, Help > Charts, Tables & Trees in Chat…, the composer's + menu and an empty-chat link open a guide with insertable example requests. New `line_chart`, `stat_list` and `checklist` widgets, each with limits, VoiceOver summaries and a source fallback; every widget gains Copy as Text (#2222).
- **macOS:** keybind settings get Liquid Glass styling, Esc to cancel a recording, per-row reset, Reset All and Undo. ⌘W no longer closes the window; it is a configurable Close Terminal shortcut. The ⌘K switcher focuses its search at once, Tab toggles All/Project, and rows and corners are tidied (#2225).
- **macOS:** the end of the chat stays visible while streaming wide code blocks or very long replies and when sending a follow-up, instead of going blank (#2227).
- **macOS:** a prompt ending in a dead-key backtick is sent whole and clears the composer; toasts are centered over the chat; ⌘P pins and unpins the open chat at once without moving the draft (#2226).
- **TUI:** Markdown in thinking previews renders instead of showing literal markers; `/effort`, `/thinking` and the effort picker update the active model profile when several profiles share a server URL, so the shown effort matches the one used (#2221).
- Engine logs from concurrent turns are written to their own session's `debug.log` instead of whichever session started last (#2223).
- Docs: design plan for cross-session agent orchestration (#2224).
## [v0.86.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.86.0) - 2026-10-21

### Features
- **macOS:** `/remote` opens a native remote access popup: QR code, the pairing code and gateway address with copy controls, an expiry countdown, connected devices and share status; *New code* issues a fresh pairing offer and *Turn off* unbinds. The engine owns the sharing bridge over ACP, so a paired device runs its prompts and commands as real turns of the shared chat; typed before a session exists, `/remote` explains itself instead of prompting the model (#2206).
- **macOS:** long diff lines scroll horizontally inside the chat's diff card instead of being clipped (#2206).
## [v0.85.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.85.0) - 2026-10-20

### Features
- **macOS:** Settings gains a Keybinds section: each app shortcut is a click-to-record keycap, with search and per-row or Reset All recovery; assigning a shortcut another action uses unbinds it there, and macOS or text-editing shortcuts are refused. Menus, tooltips and the preview and terminal controls follow a remap at once (#2202).
## [v0.84.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.84.1) - 2026-10-20

### Fixes
- The agent opens pages in the macOS live preview: `localhost_preview` `navigate` opens the panel, and a new dev server port is no longer refused because the panel shows another one.
- Detached commands keep their output in the task log, so `manage_task logs` shows a dev server's URL or an early failure.
- Absolute project paths containing `sandbox` or `artifacts` are no longer redirected into the session directories; `run_command` exposes `$RUSTCODE_SANDBOX`.
- The macOS app launches the engine with the usual tool directories on PATH, and MCP servers get them too, so `glab`, `multica` and `bunx` are found.
- Edit tools advertise fewer alias fields, cutting empty arguments from every edit.
## [v0.84.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.84.0) - 2026-10-20

### Features
- macOS: ⌘K opens a Liquid Glass chat switcher: search titles, projects, models and message text, ↑↓ to move, ⏎ to open, Esc to close; follows the sidebar's All/Project scope (#2195).
- macOS: Page Up/Page Down scroll the chat by a screen, Home/End jump to the first and latest message; the chat's scrollbar can be dragged and its track clicked again (#2193).
- macOS: the sidebar loads chats a page at a time as you scroll (1,132 chats: 1.3 s → under 70 ms to first list), with search, the archive and pinned chats still covering the whole history (#2194).
- macOS: long chats open on their newest 150 messages and load older ones as you scroll up, keeping your place; scrolling and streaming in a 3,000-message chat are about twice as fast (#2197).
- macOS: added and removed lines in diffs are drawn as one square block instead of rounded pills (#2196).
## [v0.83.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.83.1) - 2026-10-20

### Fixes
- macOS: opened chats draw at once instead of showing blank until scrolled; each chat gets its own scroll view that opens at its latest message.
- macOS: a chat pinned while open no longer stays highlighted after another chat is opened.
## [v0.83.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.83.0) - 2026-10-20

### Features
- macOS: Discord Rich Presence in the native app, opt-in per ACP client and limited to the project folder name and Working/Idle; toggle in Settings → General (#2179).
- macOS: streamed replies fade in from a light blur, with a Settings toggle and Reduce Motion fallback (#2181).
- macOS: opened chats no longer show blank or partial history when switching chats mid-load or when updates trail a response.
- macOS: sidebar times follow the last message and keep updating, so idle chats stop reading "now"; sturdier row spinner and selection (#2178).
- macOS: Settings, rename, /context and command answers close on a click outside or Esc; larger Settings, plain SF Symbol section icons, calmer approval and question cards (#2180).
## [v0.82.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.82.0) - 2026-10-20

### Features
- **Skills & MCP:** `/skills` is a toggle panel and `/mcp` shows live connection state; macOS Settings gains Skills and MCP Servers sections with search and switches. Toggles persist in the user `config.toml` and apply from the next message (#2174)
- **Engine:** the macOS app's engine imports the login-shell environment, so MCP servers and tool shells see variables exported in zsh startup files (#2173)
- **macOS:** read-only slash commands (`/status`, `/usage`, `/context`, `/mcp`…) answer during a running turn and `/cancel` stops it at once instead of waiting in the chat queue (#2171)
- **macOS:** switching the sidebar between All and Project always shows the right sessions (#2167)
- **macOS:** the transcript no longer goes blank after opening a session or sending, and scrolling up right after Latest no longer gets pulled back to the end (#2170)
- **macOS:** ⌘F search stays responsive while typing in long chats (#2169)
- **macOS:** toasts use Liquid Glass, swipe to dismiss, and the close button no longer resizes them on hover (#2168)
## [v0.81.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.81.1) - 2026-10-20

### Fixes
- **macOS:** the chat's scrollbar keeps a steady size and position while scrolling, can be dragged, and pages on a track click (#2157)
- **macOS:** the Latest button hides once you scroll back to the end and uses interactive Liquid Glass (#2157)
- **macOS:** scrolling during a streaming reply no longer gets pulled back down or jitters at the end (#2158)
## [v0.81.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.81.0) - 2026-10-19

### Features
- **macOS:** start a composer message with `!` to run it in the open project's terminal (`! ls`); `!!` sends a message starting with `!`, busy terminals are refused, and the agent never issues or sees these commands (#2154)
- **macOS:** a chat's sidebar spinner stops as soon as its turn ends, no longer waiting on the session list refresh or sticking after overlapping turns (#2152)
- **macOS:** pinning with ⌘P or a list refresh no longer leaves a stale hover that made several sidebar rows look selected; hover is fainter than selection (#2153)
- **macOS:** a message sent while a new chat is still starting now queues behind the first instead of running at once (#2155)
- **Docs:** contributing guide, security policy and issue templates; Parallel Search and Sem MCP setup examples (#2145, #2109, #2146)
## [v0.80.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.80.0) - 2026-10-19

### Features
- **macOS:** native terminal panel per project: `/terminal`, ⇧⌘T or View → Show Terminal; real PTY shells (SwiftTerm) with tabs, relaunch, 10,000-line scrollback; localhost links open in the preview (#2142)
- **macOS:** the agent can inspect, screenshot, navigate, click, fill, select and scroll the localhost preview after you allow it for the chat; restricted to the approved localhost origin and revocable from the preview toolbar (#2143)
- **macOS:** the Latest button follows the real distance from the bottom and lands there in one click; long streamed replies no longer blank the transcript (#2141)
- **macOS:** long chats resize and scroll smoothly (#2135)
- **Engine:** send images natively to a model named as its own `vision_model` (#2134)
## [v0.79.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.79.1) - 2026-10-19

### Fixes
- **macOS:** keep long chats smooth — typing, scrolling and streaming no longer redraw the whole window; rendered Markdown, code and images are cached; streamed text fades in; the copy button no longer jumps; **Latest** scrolls to the true bottom in one press; thinking blocks show live and fold to "Thought for Ns" (#2132)
- **Engine:** stream ChatGPT reasoning summaries so thinking blocks appear in the TUI, macOS and iOS (#2131)
- **Engine:** switching a long, image-heavy session to a text-only model no longer stalls for minutes re-describing every past image, and cancelled analyses are no longer cached (#2130)
- **Install:** validate checksums without awk regex intervals (#2129)
## [v0.79.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.79.0) - 2026-10-18

### Features
- Linux ARM64 (aarch64) release binaries: `install.sh`, `rustcode --update` and `/update` pick them on ARM Linux such as Arch Linux ARM; `install.sh` refuses musl systems with a build-from-source hint (#2126)
- engine: sessions no longer get stuck on "Native tool history is incomplete" after an ACP background task finishes; affected sessions recover on the next prompt, and the stop notice no longer says "Image analysis unavailable" (#2124)
- terminal: the footer below the input sits on the transcript background instead of the panel color (#2125)
## [v0.78.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.78.0) - 2026-10-18

### Features
- `rustcode export [id]` and `/export` save a session as a zip in Downloads (history, logs, telemetry and an EXPORT.md guide) to analyze on another machine (#2113)
- terminal: the footer shares the input's background and shows reasoning effort, Plan mode and auto-approve (#2113)
- terminal: `/verbosity`, `/effort`, `/thinking` and `/theme <name>` confirm in the footer instead of a panel (#2113)
- terminal: `edits` verbosity no longer shows "0/N expanded" above the input until you toggle something (#2113)
- terminal: in a question, Up from the first line of your own answer returns to the options and keeps what you typed (#2113)
- terminal: new `scroll_lines` setting for rows per mouse wheel event; docs cover kitty's `touch_scroll_multiplier` on Wayland (#2113)
- terminal: `/scroll-speed` opens a picker for lines per mouse wheel tick (1–20), saved as `scroll_lines` (#2117)
- terminal: click a visualization table header to sort by that column, and click tree nodes to collapse or expand them (#2120)
- engine: turns that hit the output or reasoning limit keep going while continuations add output, without the 1024-token recovery clamp; reasoning that fills the cap is retried with more room, and a real stop names the limit and the setting to change (#2122)
- engine: cached prompt reads count at 1/10 toward the turn token budget, repeated re-verification rounds trigger recovery, and budget stops say they are not a round limit (#2119)
- remote: `/remote` and `rustcode remote pair` count the pairing challenge down live and say how it ended; docs add discovery guidance and a security posture section (#2121)
- macOS: the app gains a localhost preview panel (#2115); release builds are signed and notarized once Developer ID secrets are configured (#2116)
## [v0.77.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.77.1) - 2026-10-17

### Fixes
- Turn-stop notices such as "turn stopped — cancelled" no longer appear as chat messages: macOS shows them as toasts, the terminal in its footer, and iOS as short sentences (#2111)
- macOS: the chat no longer scrolls past its end or jumps when the Latest button appears, and tool output no longer adds a second scroller (#2111)
- macOS: slash commands answer instantly without showing Working (#2111)
## [v0.77.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.77.0) - 2026-10-17

### Features
- macOS: slash commands answer in native toasts and sheets instead of chat cards (#2107)
- macOS: toasts for pin, archive, trash, copy and auto-approve, and for other chats finishing or waiting on you; click to open (#2107)
- macOS: system notifications when a turn fails or the engine stops in the background (#2107)
- macOS: syntax highlighting in diffs and code blocks, and diff text selectable across lines (#2107)
- macOS: opening a chat shows its history at once instead of line by line (#2107)
- engine: ACP approvals offer "Always allow" for commands and count toward user wait time (#2108)
- engine: skill routing ignores everyday words, so unrelated skills no longer load (#2108)
- engine: Swift, Xcode, make, gradle, maven and more count as verification (#2108)
- engine: context notices no longer point ACP clients at /new (#2108)
## [v0.76.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.76.2) - 2026-10-17

### Fixes
- macOS: only one chat in the sidebar lights up on hover (#2105)
- macOS: ⌘P pins or unpins the open chat (#2105)
- macOS: a new chat shows in the sidebar as soon as you send its first message (#2105)
- macOS: smoother chat scrolling; code blocks, diffs and tool output no longer catch the scroll wheel (#2105)
- macOS: autoscroll follows chat-app conventions: scrolling up lets go, reaching the end or sending a message follows again (#2105)
- engine: search_web no longer ships a built-in Exa key, adds `site:` only for a real domain, and no longer panics on non-ASCII snippets (#2105)
- engine: batched web searches and URL reads run in parallel (#2105)
## [v0.76.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.76.1) - 2026-10-17

### Fixes
- macOS: a new chat (⌘N) no longer shows another chat's running turn, and its messages no longer get queued onto that chat (#2102)
- macOS: the chat keeps following new output unless you scroll up (#2102)
- macOS: the Latest button no longer blocks scrolling under it and jumps to the true end (#2102)
## [v0.76.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.76.0) - 2026-10-17

### Features
- macOS: see what each tool runs and watch a running command's output live, instead of a bare "Run" spinner (#2100)
- macOS: scrolling up while the model writes no longer jumps back to the bottom; a Latest button returns to the newest message (#2100)
- macOS: "Working…" stays up for the whole turn, including while a reply streams and between queued turns (#2100)
- macOS: new Auto-approve tools (YOLO) setting, plus "Approve All from Now On" on permission cards (#2100)
## [v0.75.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.75.0) - 2026-10-17

### Features
- macOS app: slash commands match the TUI: `/diff`, `/changelog`, `/sandbox`, `/quota`, `/agents`, `/workspace`, `/compact`, `/recap`, `/summarize`, `/goal`, `/fork`, the login commands, and `/clear` (new chat); `/context` shows the full breakdown and `/context <tokens>` sets the window; `/status`, `/mcp` and `/tasks` give fuller reports. Unknown or terminal-only commands get a reply instead of reaching the model ([#2092](https://github.com/LHagfoss/rustcode/pull/2092)).
- macOS app: `/context` or a click on the usage capsule opens a native context sheet with a per-category bar, legend and reserves ([#2096](https://github.com/LHagfoss/rustcode/pull/2096)).
- TUI: the agent can show bar charts, tables and trees in chat with versioned `rustcode-viz` blocks; malformed or oversized blocks show their source ([#2094](https://github.com/LHagfoss/rustcode/pull/2094)).
- macOS app: native visualization blocks with hover values on charts, sortable tables and collapsible trees ([#2098](https://github.com/LHagfoss/rustcode/pull/2098)).
- Remote: `/remote status` and `rustcode remote status` show bind mode, addresses, devices, shared sessions and log paths, and name the command that recovers a stale address ([#2091](https://github.com/LHagfoss/rustcode/pull/2091)).
## [v0.74.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.74.1) - 2026-10-16

### Fixes
- macOS app: Markdown tables fit the reply width and wrap long cells instead of scrolling sideways ([#2083](https://github.com/LHagfoss/rustcode/pull/2083)).
- macOS app: "Working…" stays until the first visible reply text, Shift+Enter (or Option+Enter) inserts a newline, and chat rows show the model on the left and the time on the right ([#2083](https://github.com/LHagfoss/rustcode/pull/2083), [#2085](https://github.com/LHagfoss/rustcode/pull/2085)).
- macOS app: a sidebar row no longer stays highlighted after its context menu closes or it is pinned ([#2084](https://github.com/LHagfoss/rustcode/pull/2084)).
- ACP `session/list` reports each chat's model, including chats that never changed it ([#2085](https://github.com/LHagfoss/rustcode/pull/2085)).
## [v0.74.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.74.0) - 2026-10-16

### Features
- macOS app: notifications when a chat finishes or needs approval while the app is in the background, a Dock badge for waiting requests, and a Settings toggle ([#2073](https://github.com/LHagfoss/rustcode/pull/2073)).
- macOS app: context and token usage in the chat header, per chat; the engine sends ACP usage updates after each turn and on load ([#2076](https://github.com/LHagfoss/rustcode/pull/2076)).
- Search chat contents from the macOS sidebar, backed by the new `rustcode sessions search <query> [--all] [--json]` ([#2077](https://github.com/LHagfoss/rustcode/pull/2077)).
- macOS app: each file edit shows its diff in the tool card, with Reveal in Finder and Open; the ACP server now sends diff content for edit tools ([#2078](https://github.com/LHagfoss/rustcode/pull/2078)).
- Remote: starting a new session from a paired device always gets an answer within 15 s, slow owners are picked up on retry, and new sessions are limited to folders in home or `[remote] roots` ([#2079](https://github.com/LHagfoss/rustcode/pull/2079)).
## [v0.73.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.73.0) - 2026-10-16

### Features
- Each chat remembers its model and reasoning effort: reopening a chat in the macOS app, resuming in the TUI, or attaching from a device no longer resets effort to Off ([#2066](https://github.com/LHagfoss/rustcode/pull/2066)).
- `rustcode defaults` shows and sets the model and effort new sessions start with; macOS Settings › New Chats uses it, and new chats start from those defaults ([#2066](https://github.com/LHagfoss/rustcode/pull/2066)).
- macOS app: queue messages while a turn runs, shown above the composer with edit and remove, sent in order when the turn ends ([#2068](https://github.com/LHagfoss/rustcode/pull/2068)).
- TUI: clearer steering and queue preview with headers, numbering and spacing ([#2068](https://github.com/LHagfoss/rustcode/pull/2068)).
- macOS app: checks for updates every minute while running, and downloads updates in seconds instead of stalling ([#2061](https://github.com/LHagfoss/rustcode/pull/2061), [#2065](https://github.com/LHagfoss/rustcode/pull/2065)).
- macOS app: toolbar spacing, glass All/Project toggle, slimmer search, new Settings icon, and chat titles hide image markers ([#2062](https://github.com/LHagfoss/rustcode/pull/2062)).
- Release builds reuse caches and release PRs skip redundant checks ([#2060](https://github.com/LHagfoss/rustcode/pull/2060)).
## [v0.72.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.72.0) - 2026-10-16

### Features
- macOS app: paste screenshots with ⌘V, drop or attach images, see them in your messages and in reopened chats (click for Quick Look); image-only lines in replies render inline ([#2056](https://github.com/LHagfoss/rustcode/pull/2056)).
- ACP: `session/prompt` accepts image blocks and the engine advertises `promptCapabilities.image`; a model without vision support gets an actionable error ([#2056](https://github.com/LHagfoss/rustcode/pull/2056)).
- macOS app: Liquid Glass search bars, native All/Project segmented control, and Finder-style toolbar clusters with Archived, Find, Pin and Show in Finder ([#2056](https://github.com/LHagfoss/rustcode/pull/2056), [#2058](https://github.com/LHagfoss/rustcode/pull/2058)).
- iOS app now lives in `ios/` and ships with every release under the RustCode version (`rustcode-ios-unsigned.xcarchive.zip`, `rustcode-ios-simulator.zip`); `ios/Scripts/install-on-iphone.sh` installs it ([#2057](https://github.com/LHagfoss/rustcode/pull/2057)).
## [v0.71.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.71.1) - 2026-10-16

### Features
- macOS app: swipe a chat to pin, rename, archive or move it to the Trash, and right-click chats and messages for more actions (copy, edit and resend, send again, quote, run again) ([#2053](https://github.com/LHagfoss/rustcode/pull/2053)).
- macOS app: Cmd+N opens an empty draft that becomes a session when the first message is sent; archived chats get their own view; Cmd+B toggles the sidebar ([#2053](https://github.com/LHagfoss/rustcode/pull/2053)).
- macOS app: reasoning effort in the composer, with a thinking switch and a slider over the model's levels; arrow keys in the +, model and effort pickers; Liquid Glass on the composer and its panels ([#2053](https://github.com/LHagfoss/rustcode/pull/2053)).
- ACP sessions expose an `effort` config option for profiles with reasoning choices ([#2053](https://github.com/LHagfoss/rustcode/pull/2053)).
## [v0.71.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.71.0) - 2026-10-16

### Features
- ACP clients get slash commands: the engine advertises them with `available_commands_update` and runs host commands (`/status`, `/help`, `/usage`, `/memory`, …) without involving the model ([#2044](https://github.com/LHagfoss/rustcode/pull/2044)).
- macOS app: every session from every project in the sidebar, slash suggestions and command chips, command and summary cards, selectable replies, switching between open chats, red Stop button ([#2044](https://github.com/LHagfoss/rustcode/pull/2044), [#2045](https://github.com/LHagfoss/rustcode/pull/2045)).
- macOS app is built on macos-26 so the release ships the native Icon Composer icon ([#2043](https://github.com/LHagfoss/rustcode/pull/2043)).
- Remote: devices can list saved sessions and resume one in a detached owner ([#2048](https://github.com/LHagfoss/rustcode/pull/2048)).
- Each ACP session saves history only under its own id; new ACP sessions never reuse the shared last-active session ([#2051](https://github.com/LHagfoss/rustcode/pull/2051)).
- Detached owners fail fast instead of waiting 120 s on a credential prompt ([#2047](https://github.com/LHagfoss/rustcode/pull/2047)).
- Home-wide traversals get a short timeout and steering, and command timeouts are no longer counted as transport failures ([#2049](https://github.com/LHagfoss/rustcode/pull/2049)).
- Unannotated MCP lookups count as reads, upstream 4xx answers are tool results, and a skill already in context is not loaded again ([#2050](https://github.com/LHagfoss/rustcode/pull/2050)).
## [v0.70.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.70.0) - 2026-10-16

### Features
- macOS app: redesigned to match the iOS app, with Markdown replies, folded thinking, tool cards and a floating composer ([#2041](https://github.com/LHagfoss/rustcode/pull/2041)).
- macOS app: proper app icon, gray sidebar without divider, in-window Settings (⌘,) ([#2041](https://github.com/LHagfoss/rustcode/pull/2041)).
- macOS app: composer + menu to attach files and folders, filterable model picker, drag and drop files ([#2041](https://github.com/LHagfoss/rustcode/pull/2041)).
- macOS app: ⌘F find in chat and chat search in the sidebar ([#2041](https://github.com/LHagfoss/rustcode/pull/2041)).
- macOS app: checks GitHub for new releases and updates itself in place ([#2041](https://github.com/LHagfoss/rustcode/pull/2041)).
## [v0.69.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.69.0) - 2026-10-15

### Features
- New `edits` verbosity: tool calls stay one row while file edits (write, edit, replace, delete) start open. Ctrl+D toggles all file edits; Ctrl+O still toggles everything.
- Composer text selection now matches the transcript: double-click word, triple-click line, shift-click extend, right-click copy, and the footer shows the copy hint.
- Model prose and file-edit tool blocks no longer light up on hover; they remain copy targets.
- Turn 2 keeps the prompt cache: the first-turn session-title guidance no longer changes the system prompt.
- Common JS checks (npm/pnpm/yarn/bun lint, test, build, typecheck; eslint, tsc, vitest, jest) count as verification, so passing edit turns are recorded as completed.
## [v0.68.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.68.0) - 2026-10-15

### Features
- Native macOS app (SwiftUI): released as `rustcode-macos-aarch64-swift-app.zip`; runs the bundled engine over ACP, with per-session permission and question cards, session history and model picker ([#2036](https://github.com/LHagfoss/rustcode/pull/2036)).
- ACP clients can answer `ask_question`: every question in a chain is asked once, and RustCode-aware clients can send several options or free text ([#2036](https://github.com/LHagfoss/rustcode/pull/2036)).
- The deterministic compaction record is hidden in the TUI, kept in recent-turn requests, and no longer records cancel notices, successful reads or "continue" as instructions, failures or tasks ([#2036](https://github.com/LHagfoss/rustcode/pull/2036)).
- `multi_replace_file_content` relocates a moved target that occurs once and never re-applies an insert-around edit; new `rustcode sessions path <id>` ([#2036](https://github.com/LHagfoss/rustcode/pull/2036)).
- The remote gateway is named after the Mac's computer name ([#2016](https://github.com/LHagfoss/rustcode/pull/2016)).
## [v0.67.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.67.4) - 2026-10-15

### Fixes
- Check the shell before skills and MCP discovery: skills and MCP tools are now on-demand only, local machine state checks the shell first.
## [v0.67.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.67.3) - 2026-10-14

### Fixes
- Sessions opened from the iOS app keep their YOLO setting across owner restarts; new `yolo` config key starts every new session in YOLO (#2024)
- Remote clients no longer receive empty thinking blocks, and tool rows always carry a description (#2023)
- A derived reasoning budget of 0 no longer cuts every round at its first reasoning token (#2023)
## [v0.67.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.67.2) - 2026-10-14

### Features
- Remote session rows include when each session was last active, shown in the iOS app.
## [v0.67.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.67.1) - 2026-10-14

### Fixes
- Sessions opened from the iOS app now start your configured MCP servers, and `list_mcp_tools` names configured servers that are not running.
- Tool rows keep their title while a turn runs in remote clients.
- macOS: provider credentials live in `~/.config/rustcode/credentials.json` (owner-only) instead of Keychain, so updates no longer ask for your password once per secret. Set `RUSTCODE_CREDENTIAL_STORE=keychain` to keep Keychain.
## [v0.67.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.67.0) - 2026-10-13

### Features
- Keep working after a background command starts: the model is called again at once instead of waiting for the command, and `ctrl+b` moves a running foreground command to the background without stopping it ([#2002](https://github.com/LHagfoss/rustcode/pull/2002)).
- Make `/remote` a switch for the whole machine: every session open in a terminal is shared with paired devices by itself, a pairing code is shown only while no device is paired (`/remote pair` shows one on request), and `/remote off` turns remote access off for every session ([#2010](https://github.com/LHagfoss/rustcode/pull/2010)).
- Show short confirmations such as "New chat started", a mode switch or update progress in the footer instead of appending them to the transcript, and open help in a panel on `?` ([#2006](https://github.com/LHagfoss/rustcode/pull/2006)).
- Edit every text field with the same keys: word and line motions, `ctrl+k`, and a scrolling MCP form field; word-right now stops at the end of a word and Home/End act on the current line ([#2012](https://github.com/LHagfoss/rustcode/pull/2012)).
- Keep the last composer column free so the cursor follows the text at the right edge ([#2004](https://github.com/LHagfoss/rustcode/pull/2004)).
- Run a repeated shell command again after other work instead of returning a stale result, stop saving answered turns as failed, accept `*` for all MCP servers and an empty `edits` list beside a single edit ([#2008](https://github.com/LHagfoss/rustcode/pull/2008)).
- Add a `read_url` tool that returns the readable text of a web page, and let `rustcode -p` finish successfully on a plain answer ([#2014](https://github.com/LHagfoss/rustcode/pull/2014)).
## [v0.66.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.66.0) - 2026-10-13

### Features
- Expire a paired device 30 days after pairing: its token is then refused and the device pairs again with a fresh code. Until then it reconnects without one. `rustcode remote devices` shows when each pairing expires, and Bonjour now announces the gateway identifier so the iOS app can find a Mac that moved to another address ([#1997](https://github.com/LHagfoss/rustcode/pull/1997)).
- Hover and click one tool call instead of its whole block, copy a call and its output with a right-click, and keep the timing line out of a hovered message ([#1999](https://github.com/LHagfoss/rustcode/pull/1999)).
## [v0.65.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.65.1) - 2026-10-13

### Fixes
- Stream live output for commands that needed approval, in the terminal and on a connected phone: the approved path now uses the same tool runner as unprompted calls ([#1994](https://github.com/LHagfoss/rustcode/pull/1994)).
## [v0.65.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.65.0) - 2026-10-13

### Performance
- Remove the audio and video tools (`generate_sound_effect`, `generate_music`, `inspect_audio`, `inspect_media`, `validate_video_project`, `render_video`), the `[audio]` config section and `docs/media.md`; the built-in tool list goes from 47 to 41 and an existing `[audio]` section is ignored ([#1983](https://github.com/LHagfoss/rustcode/pull/1983)).
- Stop marking history as rewritten on every model round, which made remote clients resync the whole transcript each round ([#1980](https://github.com/LHagfoss/rustcode/pull/1980)).
- Wait briefly for the image upload lock instead of failing a phone upload when the lock is momentarily busy ([#1988](https://github.com/LHagfoss/rustcode/pull/1988)).
- Keep frames fast after expanding a tool output: the `N/M expanded` readout is cached instead of rendering every tool result per frame, about 38 ms down to 0.5 ms per frame in a 400-round session ([#1987](https://github.com/LHagfoss/rustcode/pull/1987)).
- Highlight only the new lines of a streaming code block, about 5 ms down to 1 ms per frame for a 600-line block ([#1989](https://github.com/LHagfoss/rustcode/pull/1989)).
- Skip scrollback rendering when native scrollback is off, so a resize or resume no longer re-renders the whole transcript only to drop it ([#1981](https://github.com/LHagfoss/rustcode/pull/1981)).
- Stop reading the themes directory on every frame while an approval or question prompt is open ([#1979](https://github.com/LHagfoss/rustcode/pull/1979)).
- Reuse the `@` file listing for a second instead of walking the project on every frame the popup is open ([#1990](https://github.com/LHagfoss/rustcode/pull/1990)).
- Cut per-round engine work: cache the built-in tool schema block per policy, count the prompt once when estimating tokens, compile the speculative tool-argument patterns once, and skip the MCP catalogue scan for built-in tool calls ([#1986](https://github.com/LHagfoss/rustcode/pull/1986), [#1985](https://github.com/LHagfoss/rustcode/pull/1985), [#1984](https://github.com/LHagfoss/rustcode/pull/1984), [#1982](https://github.com/LHagfoss/rustcode/pull/1982)).
- Drop unused dependencies (`reqwest-middleware` and its duplicate `reqwest` 0.12, `tokio-stream`) and unused TUI render cache types and color accessors ([#1977](https://github.com/LHagfoss/rustcode/pull/1977), [#1991](https://github.com/LHagfoss/rustcode/pull/1991)).
## [v0.64.11](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.11) - 2026-10-12

### Fixes
- Fix lag in hover, text selection and scrolling: the event loop no longer lists the session store on every pass, which parsed up to 50 transcripts per input event; a hovered message is now lit as one block across its blank paragraph rows ([#1975](https://github.com/LHagfoss/rustcode/pull/1975)).
## [v0.64.10](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.10) - 2026-10-12

### Features
- Light the chat message under the pointer in the TUI and copy it with a right-click; make slash and file completion rows hover and click targets; remove the stray spine row between tool rounds in a Ran block ([#1971](https://github.com/LHagfoss/rustcode/pull/1971)).
- Add a `/context` usage panel and composer chip to the desktop app, Session, Shortcuts and About settings pages, a chat list sorted by last activity with relative times and a spinner on the running chat, and a Latest pill that jumps on press without clicking the row behind it ([#1972](https://github.com/LHagfoss/rustcode/pull/1972)).
## [v0.64.9](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.9) - 2026-10-12

### Fixes
- Keep scrolling and text selection responsive during large live tool output by borrowing output buffers and retaining a 16 ms input polling window ([#1966](https://github.com/LHagfoss/rustcode/pull/1966)).
- Stop MCP discovery loops by returning callable names and input schemas consistently and counting rediscovery of known tools as no progress ([#1965](https://github.com/LHagfoss/rustcode/pull/1965)).
## [v0.64.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.8) - 2026-10-12

### Fixes
- Show the tool action label when a TUI tool row has no target ([PR #1961](https://github.com/LHagfoss/rustcode/pull/1961))
## [v0.64.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.7) - 2026-10-12

### Features
- Bound workspace snapshot walks that stalled oversized roots: discovery is capped at 50k entries with pruned-subtree frontiers, root children stay tracked, churn-triggered rediscovery on truncated workspaces is held, and evidence and read-cache call sites share one fresh generation per turn (#1955)
- Prefer the NetBird interface for an auto gateway bind via `[remote] prefer_interface` or `rustcode remote serve --prefer-interface`, with a LAN-first fallback (#1956)
- Toggle the gateway and sharing with `on`/`off`: `/remote on` and `rustcode remote on`/`off`, plus telemetry when a phone session is created at the home directory and trusted-mode docs for device-driven sessions (#1957)
- Adopt phone-created sessions in the terminal with `/adopt <session-id>`, which stops the detached owner safely and opens the session like a resume (#1958)
- Ask a terminal to share an open-but-unshared session from a device with `request_share`: the terminal announces presence only, approves with y or declines with n, and the outcome resolves on registration or timeout and is replayable with `get_request_status` (#1959)
## [v0.64.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.6) - 2026-10-12

### Features
- Show session-total tokens and the context budget in the footer, `/stats` and `/session` (#1949)
- Complete slash-command arguments and tighten Tab and Enter in the composer (#1951)
- Follow address changes in the remote gateway without a restart, report why a remote session could not be created, and stop session owners that never register (#1952)
- Stop re-sending a finished response to devices that attach mid-stream (#1952)
- Show RustCode as the Discord presence name (#1950)
## [v0.64.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.5) - 2026-10-10

### Features
- Support real JPEG and PNG attachments from paired mobile clients, with bounded resumable uploads and existing multimodal model handling.
- Validate image contents and keep uploads scoped to the authenticated device and session.
## [v0.64.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.4) - 2026-10-10

### Features
- Keep the remote gateway running as a per-user macOS background service, restart it after executable upgrades, and preserve paired devices and independent session owners.
- Start background sessions from the phone in Home or a chosen host folder, with authenticated directory browsing and durable creation receipts.
- Advertise nearby gateways over Bonjour for same-Wi-Fi discovery.
- Share the complete TUI command catalog with mobile clients, expand executable session commands, and expose low/medium/high choices for capability-enabled legacy effort profiles.
- Prevent duplicate or unnamed tool result rows when attaching during a running command.
## [v0.64.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.3) - 2026-10-10

### Fixes
- Support remote slash commands for session status, usage, model and effort settings, and titles without submitting model prompts.
- Let remote clients cancel an entire questionnaire with the same behavior as terminal Escape, bound to the current question identity.
- Advertise remote command capabilities and document gateway restart requirements after upgrades.
## [v0.64.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.2) - 2026-10-10

### Improvements
- Bound command-panel and task-log scrolling to visible wrapped content, including after resize, and preserve one-row mouse-wheel scrolling.
- Expose host-backed model and reasoning-effort controls to remote clients with idle-only changes and reliable request receipts.
- Include session turn counts when known and thinking-token usage with explicit estimate metadata for the iOS app.
## [v0.64.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.1) - 2026-10-10

### Fixes
- Make `/remote` pairing scan-first: automatically select the Mac's Wi-Fi/Ethernet address while preserving explicit LAN, loopback, and NetBird configuration. [#1927](https://github.com/LHagfoss/rustcode/pull/1927)
- Give the `/remote` QR panel the terminal's full height and render a stable black-on-white code with a quiet zone. [#1919](https://github.com/LHagfoss/rustcode/pull/1919)
- Export authoritative turn identity, work/thought durations, end timestamps, and completed/cancelled/failed outcomes for the iPhone app, preserving timing through harness resumes and teardown. [#1921](https://github.com/LHagfoss/rustcode/pull/1921), [#1922](https://github.com/LHagfoss/rustcode/pull/1922)
- Preserve reconnect snapshot identity and real message timestamps; improve empty-session titles and document the app UI contract. [#1923](https://github.com/LHagfoss/rustcode/pull/1923), [app UI guide](https://github.com/LHagfoss/rustcode/blob/main/docs/remote-protocol/APP-UI.md)
## [v0.64.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.64.0) - 2026-10-09

### Features
- `/remote` shares the running terminal session with a paired iPhone: it finds or starts the gateway, shows a QR code, the address and a pairing code, and the phone can then watch the session live, send, queue and steer prompts, cancel a turn and answer questions and approvals. `/remote status` lists attached devices and `/remote off` stops sharing. The terminal owns the session and must stay open; a phone disconnecting does not cancel work. Not yet exercised with a real phone over a network (#1907, #1913, #1914)
- `rustcode remote serve|pair|devices|revoke|status|stop` runs the gateway and manages paired devices. It listens on loopback until given `--bind` (or `[remote] bind` in the user config) with a LAN or NetBird address. Device tokens are stored hashed and can be revoked; traffic on a plain LAN is not encrypted, so NetBird is the recommended path. See `docs/remote.md` (#1912, #1914)
- The remote wire protocol is published as JSON Schema with example frames under `docs/remote-protocol/` for clients to build against (#1911)
- `/login openai` and `/login github-copilot` take `--headless` to show the sign-in URL or device code instead of opening a browser, and `--browser` to require one. With neither, the browser is skipped when the session is reached over SSH or has no display, and the URL is shown if the browser cannot be opened (#1905)
- Pressing Esc retires the stopped turn's identity, and a terminal answer to a question or approval that was already resolved is dropped with a notice instead of being applied (#1913)
## [v0.63.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.63.0) - 2026-10-09

### Features
- The model can register an MCP server and use it in the same session: the new `manage_mcp_servers` tool adds, lists, starts and removes servers through the same validation as `rustcode mcp add`, asks for approval on every operation, and never prints header or environment values. Use `rustcode mcp add` for real secrets, since values the model passes stay in the session history (#1901)
- Sub-agents: `spawn_agent` takes an `agent_type` (`default`, `explorer`, `worker`) and returns JSON with the agent id, model and status; `followup_task` and `send_agent` take `interrupt: true` to stop a running child and redirect it; the transcript row names the child, its task and its model instead of "agent task" (#1902)
- `write_to_file` accepts a complete file of any size up to the 40 KiB streamed-argument limit instead of rejecting content over 16 KiB after the model had generated it, and a rejected call is reported once (#1898)
- `write_file_chunk` takes `more: true` on every chunk but the last, and no compiler check runs on a file that is still being written. Several writes in one response are checked once, and diagnostics are shown next to a successful write instead of marking it as failed (#1899)
- Fixed the stall watchdog resetting a live turn to Idle when a question or approval was answered after more than five minutes (#1893)
- Time spent waiting for the user is reported as `user_wait_us` in turn metrics and `rustcode bench --report`, and is no longer counted as tool time. The `Working` row clock and `Worked for` now leave that wait out (#1897)
- `remember` stores a note that names where a credential comes from (an environment variable, `$VAR`, a file path) and refuses only a literal credential; the refusal names the rule that matched and is a validation error. Words such as `task-` or `risk-` no longer match the `sk-` rule (#1895)
- Fixed the finish gate asking for verification again after an edit to a file outside the project; the block message names the path that made verification stale (#1896)
- Fixed the hover highlight staying on the wrong rows, or painting over a question panel, when the layout changed height under a still pointer (#1900)
## [v0.62.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.62.0) - 2026-10-09

### Features
- Every tool row carries what the tool returned, not only commands and file edits: file reads, searches, listings, MCP calls, skill loads and task waits can be opened with `ctrl+o`, `ctrl+shift+o` or a click, and show a five-row preview at `low` verbosity. `high` still shows the rows alone (#1883)
- The `Working` row shows the elapsed time and the tokens for the whole turn, from the submitted prompt until the agent finishes, including tool and approval time; `Worked for` reads the same clock (#1882)
- Fixed an expanded file-edit diff being double-spaced, with an empty row after every line (#1881)
- Fixed secret redaction rewriting code and documentation the model reads: `-H "Authorization: Bearer $T"` was shown as `Authorization: [REDACTED]` without its closing quote and `token: Option<String>,` as `token: [REDACTED]`, so edits against that text could not match the file. References (`$VAR`, `<placeholder>`) and unquoted code are now shown as written; literal credentials are still redacted (#1877)
- A `background=true` command that finishes within 1.5 seconds returns its result directly instead of a task ID that costs another request to read, and `manage_task` no longer fails when an empty `task_ids` is sent with a valid `task_id` (#1880)
- A run resumed by a finished background task keeps the same tool list as the turns around it instead of losing the sub-agent tools, and a recap or prompt-improver request no longer resets the MCP tools offered to the main conversation (#1878)
- Each request logs a `provider.request_prefix` event with how much of it repeats the previous request, to tell a changed request from a provider-side prompt-cache miss (#1879)
## [v0.61.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.61.0) - 2026-10-09

### Features
- MCP servers can be managed from the command line: `rustcode mcp add <name> <url-or-command>` with repeatable `--header` (remote) and `--env` (stdio), plus `rustcode mcp list` and `rustcode mcp remove`. Header and environment values are stored in the user `config.toml` and `mcp list` prints only their names (#1865)
- On Windows, `run_command` runs in PowerShell 7 when `pwsh` is installed, else Windows PowerShell 5.1, else `cmd.exe`, instead of always `cmd /C`. The model is told which shell it has and to write PowerShell, the transcript labels the call `PowerShell` or `Cmd`, and commands with quotes or backslash paths ask for confirmation. Not yet exercised on a Windows machine (#1873)
- Fixed a command being approved as read-only when a `#` comment hid a second command on the next line; the same text also slipped past deny rules (#1874)
- The MCP tools offered to the model no longer change at the start of every turn or after a rejected tool call, so the cached prompt prefix survives between turns; tools found by `list_mcp_tools` stay available, and a new message still brings in the tools it asks for (#1870)
- Fixed a call to an MCP tool by the `mcp__<server>__<tool>` name that `list_mcp_tools` returns being rejected as an unknown tool (#1867)
- On Claude Code CLI profiles the model can call a tool that was offered after the conversation started, through a `call_tool` relay, instead of failing with "No such tool available" until the next turn (#1872)
- Wheel scrolling repaints at about 60 frames a second instead of about 21 while input keeps arriving, and each frame is written as a synchronized update so a scroll is not shown half-painted (#1868)
## [v0.60.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.60.0) - 2026-10-08

### Features
- Several workspace-changing tool calls in one response now run one after another in the order given, up to `max_mutating_calls_per_response` (default 4), instead of one per response with the rest held for a later round. Set `allow_tool_batching = false` on a model profile to restore one per response (#1860)
- Fixed held tool calls being dropped when the model answered in text instead of starting another tool round: they now run and their results reach the model before the turn can finish (#1858)
- Fixed tool rows vanishing while a batch was still running and the whole batch reappearing at once: a call that has ended keeps its row with `✓` or `✗` until the batch is recorded (#1862)
- Narration or a thought between two tool rounds is shown as text again, and the next round opens its own `Ran` block, instead of the line appearing as a row inside the block. MCP rows are labelled `Mcp` (#1856)
- The `Tool calls were queued by the scheduler and will run automatically.` notice is gone; held calls show up when they run (#1863)
## [v0.59.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.59.1) - 2026-10-08

### Fixes
- One tool block for a run of work: calls in flight no longer get a `Running` heading of their own, the `Ran` heading keeps a static muted bullet instead of turning green or red, and a step that says a line or only thinks between two rounds stays inside the block with its line as a row. `esc interrupt` moves to the `Working` row (#1854)
- The `Working` row is pinned directly above the composer; it no longer follows the last transcript row, jumps while scrolling or disappears (#1852)
- Fixed an MCP status poll being suppressed as a duplicate side effect when repeated with the same arguments in one turn: it runs again once other work ran in between, and a suppressed replay no longer counts as new information (#1853)
## [v0.59.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.59.0) - 2026-10-08

### Features
- Sub-agents report back: the root is told when a child it started finishes, without having to call `wait_agent`. `wait_agent` can wait for the first of several children (`ids`) and for up to an hour. `spawn_agent` advertises `model` and rejects an unknown profile instead of silently using the parent's (#1835, #1843)
- Compacted context can be recovered on demand: the new read-only `zoom_context` tool lists and opens the original messages behind a compaction summary, so an exact correction, command or tool argument is not lost (#1847)
- Panels are clickable: the `esc` hint closes a panel, list rows can be clicked to choose them, and both light up under the pointer, as does the back-to-bottom control (#1833)
- One tool block per batch of work: calls still running are listed under the same `Ran` heading as the ones before them, and every row carries one state glyph: `○` running or waiting, `✓` finished, `✗` failed, stopped or cancelled. The `waiting` suffix and the `Queued` indicator are gone, and MCP rows lead with `MCP` like built-in rows (#1833, #1841)
- ChatGPT-plan requests now reach the prompt cache: about 80% of prompt tokens were cached from the second request of a turn in a measured run, up from about 1% (#1840)
- Fixed Responses models (for example ChatGPT-plan `gpt-5.6`) returning nothing on planning prompts: tool rounds no longer cap hidden reasoning at 8192 output tokens, and the failure message names the output budget (#1832)
- Fixed attached images never reaching the model: a prompt with an image was sent without its text or its picture (#1846)
- Fixed MCP tools found by `list_mcp_tools` never becoming callable, which left the model searching in a loop. Naming a server now keeps the tools the request points at, and discovery queries match by word (#1837)
- A model the provider does not offer is reported in words with a pointer to `/model`, and request logs show the real tool count (#1839)
- Long notices wrap under their own text, question options show `✓` instead of `:white_check_mark:`, a recap is followed by a blank row, and the context panel says `esc` like the others (#1833, #1842)
- `scripts/release.sh` no longer waits on a prompt nobody can see when run without a terminal (#1838)
## [v0.58.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.58.0) - 2026-10-07

### Features
- Show tool calls as one block: headed `Running` while a batch is in flight and `Ran` once it finishes, with a row per call that names it (`Bash cargo test`, `Read src/main.rs`, `Edit src/lib.rs (+12 -3)`) and puts its state behind it (#1820).
- Remove the `Explored`, `Edited`, `Wrote` and `Called` headings, the `$` prefix, the tree connectors and the folded count line (#1820).
- Never draw a call that finishes within 200 ms as running (#1820).
- Make verbosity the default for tool output only: `high` shows the calls alone, `low` a five-row preview; `ctrl+o` opens and closes output at either setting (#1820).
- Light a tool block under the pointer and open or close its output on click (#1820).
- Keep the row under the transcript to the state of the turn (`Generating`, `Thinking`, `Working`, `Queued`); it no longer names a tool (#1820).
- Call background commands tasks: count them in the footer (`2 tasks · 1 done`), open the tasks panel by clicking the counter or with `/tasks`, and remove the block above the composer. `/ps` remains as an alias (#1820).
## [v0.57.16](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.16) - 2026-10-07

### Fixes
- Clear finished background commands from the block above the composer once the model has read them or the turn ends, instead of listing them as unread until the next prompt (#1810).
- Wake the model for a background command that finishes mid-turn and was not waited on, and stop adding a result it already read through `manage_task` `wait` to history a second time (#1810).
- Show the running tool call in the transcript again at high verbosity; the row by the composer carries only the state of the turn (#1810).
- Click a tool row to toggle the calls like `ctrl+o` (#1810).
- Stop walking a large workspace on every request: context preparation cost 1.3 to 5.9 seconds a round in a home directory or large monorepo (#1812).
- Treat a path with an escaped space and `find -exec` of a plain inspection command (`du`, `ls`, `wc`) as read-only, so disk scans no longer count against the workspace-change limit (#1812).
- Keep function words in a prompt ("and", "of", "can") from filling the MCP tool menu with unrelated tools (#1814).
- Read each stored credential from the operating system store once per process instead of on every request, and allow 120 seconds for a password prompt before the turn fails (#1816).
- Add `scripts/macos-stable-signature.sh`, which signs the binary with a local certificate so macOS Keychain stops asking for the login password after every update; `rustcode update` and `scripts/install.sh` reuse the certificate (#1816).
- Correct the session directory path in `AGENTS.md` (#1817).
## [v0.57.15](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.15) - 2026-10-07

### Features
- Fold each batch of tool calls into one count line at high verbosity ("Ran 5 shell commands, read 2 files"); `ctrl+o` opens the calls, then their output, then folds them again. File edits keep their rows (#1807).
- Name the running call on a row under the running indicator at high verbosity instead of in a live transcript cell (#1807).
- Ask the model for one short sentence before each batch of tool calls (#1807).
- Show the provider's own message and error code when a ChatGPT-plan stream fails, instead of "Responses API request failed" (#1807).
- Retry `server_is_overloaded` twice with a short backoff before stopping the turn (#1807).
- Keep the MCP tool menu pinned between the preflight and the provider request, so every round of a turn sends the same menu (#1807).
- Stop a generic verb in an MCP tool name ("check", "read", "clear") from admitting the tool on its own (#1807).
## [v0.57.14](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.14) - 2026-10-07

### Features
- Add a `claude` provider that serves requests through the locally installed Claude Code CLI; connect with `/login claude` (#1802).
- Scroll the transcript one line per mouse wheel event (#1801).
- Complete a turn whose only edit was a scratch file it created and then deleted, instead of reporting it as unverified (#1803).
- Stop treating "reply with ..." in a prompt as an unfinished external action (#1803).
- Keep the MCP tool menu pinned across the rounds of a turn after a lazily started server registers, and keep the ChatGPT-plan tool declaration in one position (#1803).
- Report the provider's own cache numbers in `provider_cache_status` for non-OpenRouter providers (#1803).
- Shorten the README and move its reference sections into `docs/` (#1804).
## [v0.57.13](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.13) - 2026-10-07

### Fixes
- Fix a TUI freeze when copying a composer selection (#1793).
- Turn a pasted image file path into an `[Image #N]` attachment (#1793).
- Print the exit wordmark in the active theme's colors (#1793).
- Separate tool rounds in a tool chain with a spine row so tool-only turns stay readable (#1797).
- Accept an empty `path` in `git_diff` (#1797).
- Classify unseen line ranges of an already-read file as fresh reads (#1781).
- Label a first command failure `failure`; keep `repeated_failure` for recurrences (#1782).
## [v0.57.12](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.12) - 2026-10-07

### Fixes
- Drop the LIVE and RECENT headings from background task rows; each task is one muted row, and a finished result leaves once the model has consumed it.
- Keep scrolling fast far up a long session: committed blocks and tool chains stay cached, and queued wheel input is applied before a frame is painted.
- Hold the running indicator across the gap between tool rounds so the transcript no longer jumps while the model works.
- Add the catppuccin-mocha theme and make it the default for new configs; the previous default stays available as default.
## [v0.57.11](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.11) - 2026-10-07

### Features
- Add the opt-in /pi prompt improver: the active model rewrites each prompt that starts a turn, and the transcript shows the original and the rewrite.
- Keep the TUI responsive during slow MCP tool calls by releasing the registry lock before awaiting the call.
- Order typed slash-command completions alphabetically; a bare slash keeps the curated menu order.
- Show the context window to the nearest thousand, such as 128k, instead of rounding to the nearest 100k.
- Remove the old activity-row renderer and its tests.
- Pause the desktop app build in releases and pull requests; releases publish the terminal binaries only.
## [v0.57.10](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.10) - 2026-10-07

### Fixes
- Chain tool rounds with nothing visible between them under one heading, with a properly closed tree.
- Preview the selected slash command as ghost text as soon as it is typed, and keep it in step with the arrow keys.
- Report a normal but unverified finish as unverified instead of a failed recovery.
- Tell the model that shell commands already start in the task working directory.
- Log per-phase timings for request preparation.
- Remove about 4,100 lines of dead code, orphaned source files and unused imports; the production build is warning-free.
- Keep engine tests from writing into the checkout and fix a pid-file race in the subagent cancellation test.
## [v0.57.9](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.9) - 2026-10-06

### Fixes
- Group several live tool calls under one Running or Queued heading with tree children, matching the finished Ran group.
- Keep a state word and the model name on the bottom running row instead of a bare spinner beside the token total.
- Stop a committed thought from painting twice while an argument-less MCP call is starting, and label MCP tool rows.
- Show the context window in round figures such as 1M and 200k in the welcome banner and the context panel.
- Remove the block-letter wordmark from the welcome banner.
## [v0.57.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.8) - 2026-10-06

### Fixes
- Enable subagent delegation by default, with reliable session and configuration opt-outs.
- Recover Responses API output-token exhaustion with preserved partial answers and one bounded continuation.
- Recognize terminal prose-only review answers, track completed rereads, and report bounded completion diagnostics.
- Wrap queued steering previews and show the first five diff lines with Ctrl+O expansion.
- Show consistent text diffs for writes, edits, deletes, copies, and chunk writes within existing authorization boundaries.
- Recognize Windows console hosts without TERM and verify Windows Terminal marker detection.
- Use matching Conventional Commit titles for release commits and pull requests.
## [v0.57.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.7) - 2026-10-06

### Fixes
- Reduce successful Cargo verification output in model context while preserving captured output artifacts and diagnostics.
- Make broad repository research progressive with paginated project maps, Markdown outlines, and evidence-preserving history compaction.
- Guide concise, evidence-backed review answers with useful comparison tables and explicit coverage gaps.
- Clarify context used versus remaining and distinguish configured reserves from measured usage.
- Keep detached task completion silent by default, retain captured logs and terminal reasons, and support waiting for the first task in a set.
- Separate live tasks and recent outcomes with clearer command labels, semantic status, unread-result counts, and batch summaries.
- Align AskQuestion editing shortcuts with the composer; preserve multiline paste and whitespace, and keep the cursor visible in long answers.
- Repair GitHub release-note generation and PR labeling, and document the repository search workflow.
## [v0.57.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.6) - 2026-10-06

### Fixes
- Keep filtered model picker rows, highlights, navigation and confirmation in sync.
- Use Left to enter the selected agent and Right to close the agent picker or return to the parent.
- Keep active tool and background-command summaries compact with consistent running markers and distinguishable command tails.
- Remove an extra blank row above the startup wordmark.
- Add an isolated `/test` visual demo with a Tab question preview and Escape return, without model or shell execution.
## [v0.57.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.5) - 2026-10-06

### Fixes
- Keep background task commands and agent navigation hints complete at narrow terminal widths ([#1734](https://github.com/LHagfoss/rustcode/pull/1734)).
## [v0.57.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.4) - 2026-10-06

### Fixes
- Simplify queued/running tool, task, and subagent displays while retaining progress and interrupt hints ([#1732](https://github.com/LHagfoss/rustcode/pull/1732)).
- Keep long turns responsive during queued work and tool waits; move workspace refresh off the application lock ([#1732](https://github.com/LHagfoss/rustcode/pull/1732)).
- Remove live tool-round limits from legacy sessions and resumed turns, preserving token safety and cancellation ([#1732](https://github.com/LHagfoss/rustcode/pull/1732)).
- Fix picker navigation, list scrolling, cursor movement, and keyboard/bracketed paste in panels ([#1732](https://github.com/LHagfoss/rustcode/pull/1732)).
## [v0.57.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.3) - 2026-10-05

### Fixes
# v0.57.3

- Syntax highlight edit previews again: `.ts`, `.tsx`, `.mts`, `.cts` and `.jsx` now resolve to the bundled JavaScript syntax instead of falling back to plain text, because the default syntax set ships no TypeScript definition.
- Paint write/edit diff bands across the full row width, including the `⋮` hunk separator, so short lines no longer leave unpainted columns on the right.
- Keep queued and live tool rows on one line: absolute paths contract to `~/…` and over-long targets keep their informative tail with an ellipsis.
- Stop repeating state in the row above the composer, and give background task rows the same connector, marker and spine vocabulary as every other tool list.
## [v0.57.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.2) - 2026-10-05

### Fixes
# v0.57.2

- Merge the duplicated running indicators: a foreground command now shows one `• Running $ <command> · 12s · esc interrupt` row, and the row above the composer keeps only the state and turn token total.
- Restore the inward side lines on tool groups: child rows carry `├`/`└` tree connectors beside their state markers, and command/tool output hangs under a continuous `│` spine across wrapped rows.
- Keep narrow terminals clean: indicator and tool rows are composed and truncated to the viewport width, with a compact `esc` cancel hint when the full one does not fit.
## [v0.57.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.1) - 2026-10-05

### Features
# v0.57.1

- Add native GitHub Copilot login and model selection (`/login github-copilot` via GitHub CLI credential or own `RUSTCODE_COPILOT_CLIENT_ID` app) with Chat/Responses/Messages transports.
- Keep streaming transcript output reachable while scrolled with anchored growth and jump-to-live behavior.
- Show active foreground commands with elapsed time and bounded output preview, including quiet-work state.
- Replace multi-tool branch glyphs with aligned status markers and stable grouped lists.
## [v0.57.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.57.0) - 2026-10-04

### Features
- Press `←` on an empty prompt to open the agent contexts picker; `→` or `Enter` opens the highlighted context and `←` or `Esc` closes it ([#1716](https://github.com/LHagfoss/rustcode/pull/1716)).
- Press `Alt+←` / `Alt+→` on an empty prompt to switch to the previous or next agent context, wrapping through main in spawn order ([#1716](https://github.com/LHagfoss/rustcode/pull/1716)).
- Scroll the whole transcript of a selected agent; the view no longer stops at its last eight messages or goes blank when scrolled ([#1716](https://github.com/LHagfoss/rustcode/pull/1716)).
- Opened with `←`, the agent contexts picker starts on the context currently on screen ([#1716](https://github.com/LHagfoss/rustcode/pull/1716)).
## [v0.56.17](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.17) - 2026-10-04

### Fixes
- Batch safe GitHub inspection commands without unnecessary workspace-write deferrals.
- Accept numeric subagent IDs and allow read-only children to run guarded inspection commands.
- Keep scheduler and child activity notices to one clipped row, with readable labels and fewer synthetic failure rows.
- Treat cached successful compiler checks as passed and avoid marking read-only child spawns as workspace edits.
- Fixes: https://github.com/LHagfoss/rustcode/pull/1714
## [v0.56.16](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.16) - 2026-10-04

### Fixes
- Asking for subagents in the prompt ("use 1 sub agent to ...", "spawn an agent ...") now enables the subagent tools for that task without running `/delegate` first (#1711).
- When subagents are disabled, the system prompt says so and names `/delegate`, so the model no longer searches MCP for a spawn tool (#1711).
- `/delegate on` keeps subagents enabled for the whole session; `/delegate off` disables them (#1711).
## [v0.56.15](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.15) - 2026-10-04

### Features
- Add structured turn performance reports and bounded parallel inspections with ordered results.
- Cache workspace snapshots, incremental symbols and valid successful verification; preserve evidence provenance across session restore.
- Add isolated persistent agent trees, explicit context inheritance, bounded messaging and cancellation-safe lifecycle control.
- Improve skill intent routing and reduce first-selection stalls in large legacy transcripts.
- Document measured performance and remaining live-terminal/provider reproduction limits.
## [v0.56.14](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.14) - 2026-10-04

### Features
- Draw a bar per subscription limit window in `/usage`, showing the used share and reset time from the quota data a ChatGPT plan reports; the usage link stays until the provider has reported any ([#1697](https://github.com/LHagfoss/rustcode/pull/1697)).
- Add `/refresh` to reload the provider account's model catalog, as the picker entry for `/account refresh` ([#1697](https://github.com/LHagfoss/rustcode/pull/1697)).
- Type searchable picker queries into the composer row instead of the panel header, and blank the composer row for panels without typed input ([#1697](https://github.com/LHagfoss/rustcode/pull/1697)).
- Record observed rate-limit response headers in the `provider.response_headers` event ([#1697](https://github.com/LHagfoss/rustcode/pull/1697)).
## [v0.56.13](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.13) - 2026-10-03

### Fixes
- Fix ChatGPT subscription profiles stopping every prompt at the context preflight: they now take their context window from the model catalog, with a 272k fallback for profiles saved without one ([#1694](https://github.com/LHagfoss/rustcode/pull/1694)).
- Log each model the ChatGPT catalog returns, with its visibility and whether it was kept, as a `provider.model_catalog` event ([#1694](https://github.com/LHagfoss/rustcode/pull/1694)).
- Show only the current turn's output tokens (thinking and answer) beside the running model instead of a total that grew with the whole conversation ([#1695](https://github.com/LHagfoss/rustcode/pull/1695)).
- Paste clipboard images faster on macOS by reading PNG data in-process rather than launching `osascript` ([#1695](https://github.com/LHagfoss/rustcode/pull/1695)).
- Keep the leading `/` when deleting a slash command name with Option+Backspace so the command picker stays open ([#1695](https://github.com/LHagfoss/rustcode/pull/1695)).
- Point to `/account refresh` from the `/account` panel and command description ([#1695](https://github.com/LHagfoss/rustcode/pull/1695)).
## [v0.56.12](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.12) - 2026-10-03

### Fixes
- Fix ChatGPT account refresh failing when OpenAI returns an ID token without a nonce.
- Preserve mandatory nonce matching for fresh browser sign-ins and all existing token signature and account identity checks.
## [v0.56.11](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.11) - 2026-10-03

### Features
- Load every displayable ChatGPT account model instead of only the first model, with short profile names and safe preservation of custom settings and selection.
- Refresh connected model catalogs with `/login openai` or `/account refresh`, and inspect connections with `/accounts` and `/account`.
- Show provider/account identity and provider usage guidance in `/status` and `/usage`, keeping local token totals separate from subscription quota and API billing.
- Restore safe authentication commands to arrow-key recall and improve command help and compact status/usage panels.
## [v0.56.10](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.10) - 2026-10-03

### Features
- Add configurable provider login with `/login`, `/auth status`, and `/logout` in terminal and native frontends.
- Support OpenAI Sign in with ChatGPT for eligible subscription-backed Responses models, with account-aware model selection and automatic token refresh.
- Store provider tokens and imported API keys in the native operating system credential manager, with endpoint binding and no plaintext fallback.
- Preserve existing API-key configurations and document custom providers, multiple accounts, and reconnect workflows.
## [v0.56.9](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.9) - 2026-10-03

### Fixes
- Keep forwarded Cmd+C copy-only so repeated copying from the session panel cannot trigger double-press exit; preserve Ctrl+C exit behavior ([#1679](https://github.com/LHagfoss/rustcode/pull/1679)).
- Add `/discord` with explicit on/off/status commands, live worker control, and persistent project/global settings ([#1681](https://github.com/LHagfoss/rustcode/pull/1681)).
- Show short tool-work durations, including `<1s`, on completion separators ([#1680](https://github.com/LHagfoss/rustcode/pull/1680)).
- Refresh cached skill-routing metadata after `/skills`, `list_skills`, or `use_skill` discovers an edited, added, or removed skill ([#1683](https://github.com/LHagfoss/rustcode/pull/1683)).
## [v0.56.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.8) - 2026-10-03

### Features
- Show cumulative input and output tokens beside the running model, including provisional live output estimates, provider continuations and retried requests ([#1672](https://github.com/LHagfoss/rustcode/pull/1672)).
- Reduce MCP discovery context with compact server summaries, bounded tool search and per-server listings; carry matching tools into native requests within schema limits ([#1673](https://github.com/LHagfoss/rustcode/pull/1673)).

### Fixes
- Generate a catch-up recap after 10 minutes idle following completed work, including a focused terminal; require new progress before repeating ([#1670](https://github.com/LHagfoss/rustcode/pull/1670)).
- Preserve the last terminal frame when a dirty or height-changing redraw fails, then recover on the next successful frame ([#1674](https://github.com/LHagfoss/rustcode/pull/1674)).
- Cancel an automatic recap when user activity resumes or idle guards change while the recap provider is pending ([#1676](https://github.com/LHagfoss/rustcode/pull/1676)).
## [v0.56.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.7) - 2026-10-02

### Fixes
- Hide raw Read, MCP and Skills results while retaining command output, compact status rows and human answers. ([#1664](https://github.com/LHagfoss/rustcode/pull/1664))
- Show Codex-style inline file diffs with correct source line numbers, change counts, syntax highlighting, wrapping and hunk separators; capture real before/after diffs for file writes and keep recorded edits visible at High verbosity. ([#1665](https://github.com/LHagfoss/rustcode/pull/1665))
## [v0.56.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.6) - 2026-10-02

### Features
- Add reusable Markdown prompt templates with `/prompts` and `/prompt <name> [arguments]`, editable composer staging, workspace overrides, and user-template config sync ([#1659](https://github.com/LHagfoss/rustcode/pull/1659)).
- Add `/pwd` and `/diff` panels for local workspace inspection without a model request, including bounded asynchronous Git output and stale-result protection ([#1660](https://github.com/LHagfoss/rustcode/pull/1660)).
- Reuse wrapped command output for collapsed previews and expand hints; a local release benchmark measured 49% lower rendering time for long command previews ([#1658](https://github.com/LHagfoss/rustcode/pull/1658)).
## [v0.56.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.5) - 2026-10-02

### Fixes
- Balance table column widths and use labeled records when a narrow terminal cannot fit a readable grid ([#1656](https://github.com/LHagfoss/rustcode/pull/1656)).
- Keep the chat spinner visible throughout active work, add spacing above it, and preserve user-message background padding ([#1656](https://github.com/LHagfoss/rustcode/pull/1656)).
- Truncate completed command headers to one line while retaining the exit status and expandable output ([#1656](https://github.com/LHagfoss/rustcode/pull/1656)).
## [v0.56.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.4) - 2026-10-02

### Features
- Add per-request thinking decisions through an opt-in oMLX decider endpoint, with immediate application and fallback to normal behavior ([#1647](https://github.com/LHagfoss/rustcode/pull/1647)).
- Resolve local JSON Schema references and validate enum arguments for MCP tools ([#1648](https://github.com/LHagfoss/rustcode/pull/1648)).
- Let Ctrl+C copy a selection and still arm the second press to exit ([#1649](https://github.com/LHagfoss/rustcode/pull/1649)).
- Advance the terminal tab spinner using a shared animation clock ([#1650](https://github.com/LHagfoss/rustcode/pull/1650)).
- Anchor the running indicator directly below transcript activity ([#1651](https://github.com/LHagfoss/rustcode/pull/1651)).
- Add idle catch-up recaps and completion timing below finished turns ([#1623](https://github.com/LHagfoss/rustcode/pull/1623), [#1622](https://github.com/LHagfoss/rustcode/pull/1622)).
- Improve tool previews, expansion hints, model-match highlighting, and feedback placement ([#1611](https://github.com/LHagfoss/rustcode/pull/1611), [#1628](https://github.com/LHagfoss/rustcode/pull/1628), [#1630](https://github.com/LHagfoss/rustcode/pull/1630), [#1638](https://github.com/LHagfoss/rustcode/pull/1638), [#1645](https://github.com/LHagfoss/rustcode/pull/1645)).
- Support copying in informational panels, improve selected-text scrolling, and cache pinned selection projections ([#1614](https://github.com/LHagfoss/rustcode/pull/1614), [#1619](https://github.com/LHagfoss/rustcode/pull/1619), [#1613](https://github.com/LHagfoss/rustcode/pull/1613)).
- Stabilize streaming frames and isolate render panics ([#1636](https://github.com/LHagfoss/rustcode/pull/1636), [#1635](https://github.com/LHagfoss/rustcode/pull/1635)).
- Bound per-server MCP schemas and surface server startup failures ([#1637](https://github.com/LHagfoss/rustcode/pull/1637)).
- Preserve cancelled-stream progress and safe shell reads in durable replay, check skills before live app probes, clear idle watchdog clocks, and make the additional API-key header opt-in ([#1634](https://github.com/LHagfoss/rustcode/pull/1634), [#1629](https://github.com/LHagfoss/rustcode/pull/1629), [#1610](https://github.com/LHagfoss/rustcode/pull/1610), [#1642](https://github.com/LHagfoss/rustcode/pull/1642), [#1640](https://github.com/LHagfoss/rustcode/pull/1640)).
## [v0.56.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.3) - 2026-10-01

### Fixes
- Hold the transcript scroll position while the model streams instead of yanking the view to the bottom, with a `↓ Back to bottom` affordance that reports unseen activity (#1595)
- Stop leaving the whole session transcript in terminal scrollback on exit; `preserve_transcript_scrollback` opts back in (#1587)
- Bound expanded tool bodies and stop appending ~100 un-truncated lines per tool call; expansion is now render state, so collapsing is its exact inverse (#1593)
- Ctrl+O now toggles every truncated tool call at once, with Ctrl+Shift+O stepping one entry and an `N/M expanded` readout (#1594)
- Route slash-command panels through the panel label/value helpers so label columns align and over-threshold values are marked, and add fuzzy subsequence matching with highlighted matches to slash-command and palette search (#1588)
- Stop silently dropping 16 of 37 built-in tools, tell the model which tools were withheld and how to reach them, stop the built-in menu drifting mid-session, and only emit the Delegation Policy when delegation tools exist (#1589)
- Pin the native MCP tool selection for the turn so the selected set, and the prompt prefix cache, survive lazily-started MCP servers (#1591)
- Hold over-budget tool calls and execute them automatically in a later round instead of telling the model to reissue them (#1590)
- Stop re-sending cancelled-turn records to the provider on every later turn, and stop reporting stale progress metrics on a cancelled turn (#1592)
## [v0.56.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.2) - 2026-10-01

### Fixes
- fix(engine): prefer focused entry for ctrl+o toggle (#1583) — closes #1581. Second press collapses what the first expanded even after newer tool output arrives.
- perf(tui): skip unchanged selection frames and memoize wrap (#1585) — closes #1582. Static frames reuse the last scan (200 unpinned capture frames 88ms → 41ms); scrolling unchanged. Wrap counts memoized; committed cache 4 → 32.
- fix(tui): render full tool bodies when expanded (#1584) — closes #1580. Expanded Command/Tool bodies render in full, not the 6-line window (now COLLAPSED_TOOL_BODY_MAX_LINES); homogeneous command batches render full with no dangling truncation marker.
- mcp: live OAuth verification (#1536) remains open for the human browser-login step; automated mcp:: suite 41 passed.
## [v0.56.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.56.1) - 2026-10-01

### Features
- Remote MCP OAuth now re-plans from the registered grant, falls back from rejected device flow to a loopback browser callback, and preserves resource scopes (#1569, #1553).
- Untrusted-by-default sandbox flips to trusted default with YOLO override honored everywhere including headless and ACP; denial messages name the effective permission (#1574, #1546).
- Command overlays and panels stay stable while a turn runs; informational commands render through panels (#1575).
- Tool groups collapse precisely (Ctrl+O targets visible candidates) and low-verbosity mode shows compact edit diffs (#1576).
- Scrolled selections keep their copy text when extended; copy binding advertised as ctrl+c on every platform (#1577).
- Panic cleanup is scoped to the terminal owner, so a recovered background panic restores normal exit erasing (#1578).
- Engine policy paths hardened (no unchecked unwraps on sandbox classification) and runtime contracts documented (#1573).
## [v0.55.11](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.11) - 2026-09-24

### Improvements
- Add ACP session listing, loading, persisted titles, and reliable prompt continuation.
- Enforce Linux and macOS shell sandbox modes, with visible permissions and confirmed one-command network or scoped filesystem access.
- Improve shell approval rules, context usage and history reporting, and command and question UI.
- Bound log storage and remove the unused Laya sidecar.
## [v0.55.10](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.10) - 2026-09-23

### Fixes
- Added live steering so corrections can be applied during the active turn while follow-up prompts stay queued.
- Improved loop recovery to recognize useful progress, and hardened native tool call/result handling and cancellation.
- Aligned footer context usage with the detailed context view.
- Removed unused code and test-only APIs to eliminate compiler warnings.
## [v0.55.9](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.9) - 2026-09-23

### Fixes
Keep partial assistant responses visible while live tools are running.
## [v0.55.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.8) - 2026-09-22

### Features
- Added a rustcode-managed scheduler daemon for reliable scheduled work.
- Added opt-in Laya policy integration.
- Fixed wakeup turns stalling during native schema preparation by computing schemas outside the application-state lock.
- Improved deferred-call handling and reasoning recovery diagnostics.
- Preserved queued prompts after turn cancellation.
- Added a repository link to Discord Rich Presence.
## [v0.55.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.7) - 2026-09-22

### Fixes
- Fixed context usage overlay spacing, close affordance, and bottom padding.
- Made Shift+Tab reliably switch Build/Plan mode while preserving Tab completion.
- Normalized Shift+Tab auto-confirm handling in approval prompts with visible YOLO feedback.
## [v0.55.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.6) - 2026-09-21

### Fixes
Session reliability fixes:

- Opening or resuming a session now stays idle; pending work continues only after `/continue` or `--continue`.
- Improved task diagnostics and edit-tool guidance, including correct handling of benign Hugging Face authentication warnings.
## [v0.55.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.5) - 2026-09-21

### Fixes
- Recover stalled provider SSE streams with a transport-level read timeout and a recoverable idle-stream error.
- Restore the startup banner help prompt: `run this command to get help: /help`.
## [v0.55.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.4) - 2026-09-21

### Fixes
- Refine command recommendation styling for clearer selection and input text.
- Compact rejected-tool diagnostics in the transcript and prevent failed external actions from being reported as completed.
## [v0.55.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.3) - 2026-09-21

### Fixes
- Prevent interactive shell environment probing from stealing the terminal process group on macOS.
- Skip redundant startup shell probing when configured provider credentials are already inherited.
## [v0.55.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.2) - 2026-09-20

### Features
- Add `manage_task wait` for bounded, cancellable background-task waits.
- Keep productive turns running while background tasks complete and flush completions at turn boundaries.
- Add a live background-task progress chip with running count, elapsed time, command labels, and management hints.
## [v0.55.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.1) - 2026-09-20

### Features
-
## [v0.55.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.55.0) - 2026-09-20

### Features
- Chained `ask_question` prompts, opencode-style: one call can carry several questions with headers, labeled options with descriptions, and per-question multi-select; answer each in turn, move with tab, submit all with enter (#1277).
- `ask_question` answers now render in the transcript as `Asked <question> → <answer>` (chains summarize every pair), including cancellations (#1275).

### Fixes
- Compound start/verify/stop shell scripts that manage their own background jobs (`wait`, `$!` + `kill`) no longer auto-detach into a 0-byte result; bare trailing-`&` servers still do (#1276).
- Stall watchdog recovers ownerless queued prompts instead of wedging forever; orchestrator lease is Drop-guarded and finish lines carry queue/status forensics (#1278).
## [v0.54.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.54.0) - 2026-09-20

### Features
# v0.54.0 — Features

Agent-harness upgrades inspired by fortunto2/rust-code, shipped as a 4-PR stack
(#1270 → #1271 → #1272 → #1273) plus a refactor pass.

- Doctor & onboarding: new `rustcode doctor [--fix]` checks config, git/rg/tmux
  binaries with install hints, and skill directories.
- Git tools: `git_status` / `git_diff` (read-only, plan-mode safe) and
  `git_add` / `git_commit` (confirmation-gated, broad staging refused).
- Skills: `triggers` / `keywords` / `priority` frontmatter with relevance
  routing; `list_skills` is priority-sorted.
- Tool registry: fuzzy name resolution with "did you mean" suggestions and
  progressive `filter_tools_by_query` discovery.
- Memory: 7-day tentative GC plus confidence-first budgeted rendering.
- Provider resilience: ordered `fallback_chain` (primary → small → rest).
- Headless autonomy: `rustcode -p ... --loop N` with `<loop:done/>` and a
  10-iteration circuit breaker.
- OpenAPI: `openapi_call` turns any OpenAPI 3.x spec into a validated call.
- Delegation: `delegate_task` hands `.tasks/*.md` to external CLIs
  (claude/gemini/codex/opencode/rustcode).
- Benchmarking: `rustcode bench` scores turns (0–100, A–D grades).
- Symbols: fuzzy-ranked `find_symbol` results (top 20).
- Refactor: shared truncate/normalize helpers, lint fixes, no behavior change.
## [v0.53.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.53.1) - 2026-09-20

### Fixes
- Welcome and command output panels now use more consistent padding, responsive widths, and command-specific titles.
- `/usage` preserves a clear gap before monthly usage statistics, and the welcome panel includes a friendlier `/help` hint.
## [v0.53.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.53.0) - 2026-09-19

### Features
- Discord Rich Presence now falls back to a privacy-safe workspace name and shows compact output/total token usage checkpoints.
- Pressing Escape during a streaming turn preserves queued prompts and automatically continues them in FIFO order after cancellation.
- Welcome-panel slash-command hints now align in a stable table-like column, including `/help`, with narrow-terminal-safe fallbacks.
## [v0.52.20](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.20) - 2026-09-19

### Features
- Add `rustcode sync --pull` and `rustcode sync --push` while preserving the existing sync subcommands and default pull-then-push behavior.
- Add macOS-first Discord Rich Presence through local desktop IPC, with session-aware activity, background retries, and `rustcode discord --setup|--status|--enable|--disable` controls.
- Harden agent-loop ownership and watchdog recovery, bound continuation replay, stop over-budget requests with actionable checkpoints, and improve stream finalization plus usage/session attribution.
## [v0.52.19](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.19) - 2026-09-19

### Fixes
- Restore responsive live streaming and reliable final assistant transcript handoff, including token usage preservation.
- Place welcome-banner slash-command hints beside their associated values and keep the layout safe on narrow terminals.
## [v0.52.18](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.18) - 2026-09-18

### Fixes
- Protocol-aware vision requests: Responses endpoints get input[] instead of a chat-completions body (#1236).
- Welcome banner: restored top padding, grouped session/model, one aligned slash-command row (#1239).
- Batch reliability: chunked-write steering past ~4 KiB (#1233), summaries for thinking-only finales (#1234), control-first scheduling with reads following (#1230), checkout worktree rule (#1229), stall watchdog recovering dead turns (#1226).
## [v0.52.17](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.17) - 2026-09-18

### Fixes
- Actionable error when vision_model matches no configured profile: names the bad value and lists valid options (#1236).
## [v0.52.16](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.16) - 2026-09-18

### Fixes
- Log panics and orchestrator task death to debug.log so silent mid-stream hangs leave evidence (#1226).
- Batch MCP tools advertising the standard readOnlyHint as read-only instead of serializing them one per round (#1230).
## [v0.52.15](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.15) - 2026-09-17

### Fixes
- Fix pending background `run_command` receipts rendering as `failed`; they now show `running` until the real outcome arrives, and cancelled background tasks show `cancelled` (#1221).
- Collapse repeated `/ps` polls and Tab mode-switch notices in durable history instead of appending near-duplicates (#1222, #1223).
- Clarify the status hint to `esc interrupts stream only` while background terminals run.
## [v0.52.14](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.14) - 2026-09-17

### Fixes
- **Thinking estimate:** Derive the client reasoning estimate once from total kept reasoning chars instead of summing per-SSE-delta ceils, which overcounted token-piece streams ~20-30% and cut thinking early ([#1218](https://github.com/LHagfoss/rustcode/pull/1218); fixes [#1216](https://github.com/LHagfoss/rustcode/issues/1216))
- **Search noise:** Prune vendored dirs (`node_modules`, `target`, `dist`, ...) from recursive glob/grep unless explicitly targeted, so broad queries in non-git workspaces no longer dump dependency trees into every round's prefix ([#1219](https://github.com/LHagfoss/rustcode/pull/1219); fixes [#1217](https://github.com/LHagfoss/rustcode/issues/1217))
## [v0.52.13](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.13) - 2026-09-17

### Fixes
- **Thinking budget:** Scale the derived default thinking budget with the output cap (`(context/8)` capped by `completion/2`) instead of the flat 2048 clamp, keeping the client-side cut as a high backstop; set `thinking_budget = 8192` on the `deepseek-v4.1-flash` default profile ([#1213](https://github.com/LHagfoss/rustcode/pull/1213); fixes [#1211](https://github.com/LHagfoss/rustcode/issues/1211))
- **Shell file tracking:** Detect shell redirection targets (`>`, `>>`, heredocs) in `run_command` and record them into `metadata.changed_paths`, plus one workflow prompt line preferring write tools for file creation ([#1214](https://github.com/LHagfoss/rustcode/pull/1214); fixes [#1212](https://github.com/LHagfoss/rustcode/issues/1212))
## [v0.52.12](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.12) - 2026-09-16

### Features
- Batch every valid read-only tool call per model round while keeping one workspace mutation per round (more for explicitly trusted profiles); providers receive parallel-tool-call hints with the scheduler as the safety boundary.
- Count stagnation once per model round so a bulk read round cannot trip the no-progress budget; any meaningful result clears the streak.
- Emit at most one recovery guidance per round: parked call-repetition warnings yield to evidence recovery and failure replans.
- Persist long-turn segment state across restarts and auto-continue pending productive segments on session resume.
- Require ledger-confirmed stagnation before the same-files reasoning-loop flag so verifying long tasks are not misflagged.
- Report provider prefix-cache decisions in turn summaries for long-task cache-stability visibility.
- Pair reused native tool ids with their nearest announcement, keep native calls out of text normalization, scope anonymous transport-failure streaks per tool, hold runtime notices outside Responses call transactions, and enable the dead compaction-anchor test.
## [v0.52.11](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.11) - 2026-09-16

### Fixes
- Fixed Responses API tool-call transaction ordering when assistant continuity text accompanies multiple tool calls, preventing deferred tool outputs from being rejected by the provider.
## [v0.52.10](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.10) - 2026-09-16

### Fixes
- Make productive tool turns resumable across bounded segments while retaining a hard total-round safety ceiling.
- Detect repeated provider, MCP, and unavailable-dependency infrastructure failures across tool names and stop with an actionable recovery message.
- Default provider requests to one tool call at a time, with explicit trusted-model batching opt-in and compatibility fallback for providers that reject the optional field.
- Keep the welcome/status panel inside narrow terminal widths and preserve its existing layout output at normal widths.
## [v0.52.9](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.9) - 2026-09-16

### Fixes
- Keep native tool-call history valid when duplicate tool results are compacted, preventing provider `TOOL_CALLS_MISSING_RESULTS` failures during long sessions.
- Scope relative reads and writes to the ACP task directory, enforce workspace and symlink boundaries, and add explicit empty-project bootstrap guidance.
- Remove pasted provenance markers from model-facing context and keep task/workspace context consistent during schema selection and tool execution.
## [v0.52.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.8) - 2026-09-15

### Fixes
- Improved session status visibility with live session IDs, branch updates, and explicit tool intent.
## [v0.52.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.7) - 2026-09-15

### Fixes
- Refresh the composer workspace path and Git branch while RustCode is running.
- Recover safely from interrupted provider SSE streams without replaying ambiguous tool calls.
- Add bounded, actionable Git worktree ownership and cleanup diagnostics.
## [v0.52.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.6) - 2026-09-15

### Fixes
- Hardened malformed model tool-call recovery with clearer parse diagnostics and typed examples.
- Stabilized MCP tool selection with deterministic zero-relevance behavior and measured schema budgets.
- Added repeated-verification recovery guidance and provider cache metric/status telemetry.
## [v0.52.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.5) - 2026-09-15

### Fixes
- Preserve tool-call/result transactions when projecting provider history, preventing `TOOL_CALLS_MISSING_RESULTS` failures after multi-file edits.
- Add OpenRouter session affinity and normalize cache usage telemetry (`cached_tokens`, cache writes, and discounts) for provider diagnostics.
- Keep cache and tool-history behavior covered by regression tests.
## [v0.52.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.4) - 2026-09-15

### Features
## Features

- Add first-class OpenCode Zen support via the OpenAI Responses API, including the built-in paid `opencode-muse-spark-1.3` profile with native function calling. Free Zen models remain restricted to the OpenCode client session.
## [v0.52.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.3) - 2026-09-15

### Fixes
- Gate automatic idle recaps on completed model turns and true idle entry.
- Persist deterministic first-turn session titles with optional model refinement.
- Make large textual file writes bounded, resumable, and safe across interrupted streams.
## [v0.52.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.2) - 2026-09-14

### Fixes
- Expose the active tool protocol and capabilities to the model.
- Support chunked, resumable large-file edits.
- Recover from stalled streams and no-progress turns.
- Make tool-result status explicit, including truncation and write completion.
## [v0.52.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.1) - 2026-09-14

### Fixes
- Improved local-model context budgeting, tool scheduling, and continuation limits.
- Distinguish truncated tool arguments, clipped results, and successful writes.
- Reduced tool-schema and context overhead while preserving skill-routing compatibility.
- Made skill routing turn-safe.
## [v0.52.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.52.0) - 2026-09-14

### Features
- Added safe first-turn session title generation through a dedicated bounded tool.
- Preserved resumable recovery for truncated textual and native provider tool streams.
- Prevented incomplete or duplicate side-effecting tool calls after stream failures.
## [v0.51.47](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.47) - 2026-09-14

### Fixes

- Preserve bounded typed tool-call checkpoints when a native provider stream fails mid-call, without dispatching or replaying incomplete arguments.
## [v0.51.46](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.46) - 2026-09-14

### Fixes
- Added bounded grounded recovery after malformed or truncated write/read sequences, preserving successful artifacts without replaying side-effecting calls.
- Rejected reasoning leakage from incomplete textual mutation payloads while allowing complete literal content to remain writable.
## [v0.51.45](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.45) - 2026-09-13

### Fixes
- Recover truncated textual tool calls after provider stream failures with bounded, resumable checkpoints and no duplicate side effects.
- Standardize live activity headings on `Explored` and `Ran` while retaining concrete child operation details.
- Let the verified `kat-coder` profile adapt tool output up to its configured 16k budget while preserving conservative fallback limits.
## [v0.51.44](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.44) - 2026-09-13

### Fixes
- Prevent shell background operators from leaving cancelled tool turns stuck.
- Preserve actionable MCP server diagnostics when startup fails.
- Show file-edit activity with an operation-specific live label.
## [v0.51.43](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.43) - 2026-09-13

### Fixes
- Enforce one textual tool call per model response and keep deferred calls out of active activity display.
- Hide internal deferred-call recovery notices from the transcript.
- Use a compact 2x3 spinner glyph set without bracket wrapping.
## [v0.51.42](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.42) - 2026-09-12

### Fixes
## Fixes

- Retry a single mid-stream provider response-body decoding failure instead of aborting the turn.
- Preserve non-retry behavior for malformed SSE, invalid UTF-8, cancellation, and generic I/O failures.
## [v0.51.41](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.41) - 2026-09-12

### Fixes

- Stop repeating oversized textual tool-call responses when a complete call is already available before a truncated suffix.
- Show speculative tool activity as `Calling` until execution begins.
## [v0.51.40](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.40) - 2026-09-12

### Fixes
 - Show incremental native tool activity while streamed tool arguments are incomplete.
 - Simplify animated terminal-title spinners by removing square-bracket wrapping.
## [v0.51.39](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.39) - 2026-09-11

### Fixes
- Migrate legacy session directories that contain only sandbox or artifact state.
## [v0.51.38](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.38) - 2026-09-11

### Fixes

- Partition sessions by date with UUIDv7-shaped IDs and provide safe legacy migration with per-session diagnostics.
- Share Rust dependency and native build-script caches across CI jobs.
- Hide textual tool-call envelopes from the human-facing transcript.
## [v0.51.37](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.37) - 2026-09-11

### Fixes
- Stabilized composer sizing and input routing across multiline, paste, and cancellation states.
- Kept large pasted content compact in recaps while preserving full provider payloads.
- Fixed stale inline-terminal rows and added the requested Braille activity spinner.
## [v0.51.36](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.36) - 2026-09-11

### Fixes
- Replay covered view_file subranges with the cached numbered source slice instead of a status-only notice.
- Keep exact read replay compact while preserving truthful inspection metadata and offset/range safety.
- Render cancelled turns as a left-aligned User Stopped separator without the X marker.
## [v0.51.35](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.35) - 2026-09-11

### Fixes
- Prevent complete file reads from re-emitting their full body during unchanged replay.
- Reuse cached complete evidence for covered `view_file` subranges while preserving typed inspection metadata and durable history.
- Add regressions for exact and subrange read-loop recovery.
## [v0.51.34](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.34) - 2026-09-10

### Fixes
- Make `view_file` completeness explicit so complete reads are not mistaken for truncated output and reread in a recovery loop.
- Preserve clear distinctions between complete requested ranges and genuinely capped reads.
## [v0.51.33](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.33) - 2026-09-10

### Fixes
- Normalize equivalent read-only shell/file inspections so unchanged evidence is reused instead of re-executed.
- Strengthen complete-read and manual-preview workflow guidance to stop inspection loops and respect user-controlled preview ports.
- Add regression coverage for inspection progress and static-app validation.
## [v0.51.32](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.32) - 2026-09-10

### Fixes
- Stabilize request-prefix reuse and runtime context projection across long recovery turns.
- Remove arbitrary productive tool-call and tool-round ceilings while preserving progress and cancellation safety guards.
- Improve multiline composer scrolling, cancellation cleanup, interrupt status placement, and sanitized exploration summaries.
## [v0.51.31](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.31) - 2026-09-10

### Fixes
- Disabled hidden reasoning during bounded recovery so local models can take a concrete tool action after a no-progress loop.
- Simplified per-turn dynamic context by removing repeated repository-map and duplicate checkpoint state while preserving recent tool evidence, compaction, and cache reuse.
- Added deterministic regression coverage for growing tool-call loops, multi-round history projection, and sanitized context diagnostics.
## [v0.51.30](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.30) - 2026-09-10

### Fixes
- Keep sequential tool rounds under one transcript group across incremental commits.
- Prevent incidental shell failures from blocking completion when no requested verification command failed.
- Normalize conversation recaps and keep interrupt guidance in the composer footer.
## [v0.51.29](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.29) - 2026-09-10

### Fixes
- Execute one tool call per model round, including read-only inspection, to keep local-model context and tool state coherent.
- Preserve structured tool-call/result pairing, sequential per-call compiler checks, and bounded over-eager tool responses.
- Use one consistent divider color across conversation recap and work separators.
## [v0.51.28](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.28) - 2026-09-10

### Fixes
- Bound request-local context to durable dialogue plus the previous and active turns while preserving complete stored history.
- Keep append-only provider prefix-cache reuse across runtime context updates and rebase only when history or cache state changes.
- Replace automatic idle recap transcript echoes with a deterministic, bounded task/status recap and exclude recaps from memory and compaction inputs.
## [v0.51.27](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.27) - 2026-09-10

### Fixes
- Count failed tool results as no progress even when the command or error text is novel, preventing failure-only recovery loops from consuming the turn budget.
- Preserve successful progressive reads, searches, verification, and multi-step recovery behavior.
## [v0.51.26](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.26) - 2026-09-09

### Fixes
- Bound repeated equivalent command-failure loops while preserving progressive reads, edits, compaction, and recovery.
- Redact common credential assignments before tool output is persisted or shown to the model.
## [v0.51.25](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.25) - 2026-09-09

### Chores
- Maintenance release; no functional changes since v0.51.24.
## [v0.51.24](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.24) - 2026-09-09

### Features
- Bound recovery context growth and preserve progressive tool reads.
- Add request-prefix cache diagnostics and deterministic recovery decisions.
- Reduce Groq request context overhead with compact tool prompts and provider error handling.
## [v0.51.23](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.23) - 2026-09-09

### Fixes
- Fix Groq Compound compatibility by omitting unsupported thinking-control fields from generic provider requests.
- Improve release CI handling for delayed required-check registration.
## [v0.51.22](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.22) - 2026-09-09

### Fixes
- Prevent repeated equivalent side-effecting tool calls from replaying within the same user turn while keeping reads and failed calls retryable.
- Make recovery less forceful: bounded read-only inspection and diagnostics remain valid, and truncated tool-call responses fail closed with actionable retry results.
- Preserve typed instruction provenance so base/developer instructions are reconstructed once and runtime/tool evidence remains correctly ordered.
## [v0.51.21](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.21) - 2026-09-08

### Fixes
- Normalize nullable and union MCP schemas for provider-compatible tool definitions.
- Validate `anyOf` and `oneOf` arguments at runtime, including nullable fields and numeric bounds.
- Treat progressive non-overlapping file reads as new inspection evidence while retaining exact-read loop protection.
## [v0.51.20](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.20) - 2026-09-08

### Fixes
- Keep volatile runtime context at the end of each provider request so tool rounds preserve a stable cacheable prefix.
- Preserve a compact continuity breadcrumb across think-only tool turns.
- Detect semantically equivalent native and shell file rereads and recover bounded read-only loops earlier.
## [v0.51.19](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.19) - 2026-09-08

### Fixes
Preserve source/read completeness independently from model-payload truncation.

Safely re-execute failed, truncated, and unreplayable repeated reads instead of returning metadata-only evidence.

Bound runtime-context deltas and reduce duplicate context accumulation across tool rounds.
## [v0.51.18](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.18) - 2026-09-08

### Fixes
- Preserve provider request prefixes across tool rounds to improve KV-cache reuse and reduce repeated context prefill.
- Add safe detached server/watcher execution and prevent nested shell background jobs from hanging RustCode output pipes.
- Keep detached processes tracked, killable, and cleaned up with their session.
## [v0.51.17](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.17) - 2026-09-08

### Fixes
- Added a native `list_mcp_tools` inventory for reliable MCP discovery.
- Kept explicitly requested MCP server and tool schemas available across tool rounds.
- Prevented native file reads of dotenv secret files while allowing safe example files.
## [v0.51.16](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.16) - 2026-09-08

### Fixes
- Group MCP and custom tool activity under the generic Running label while preserving invocation details.
- Qualify MCP tool names in activity displays for clearer tool attribution.
## [v0.51.15](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.15) - 2026-09-07

### Fixes
- Replay bounded cached bodies for unchanged native reads.
- Keep read-only loop recovery advisory with tools available.
- Allow harmless shell inspection while preserving safety enforcement.
- Clarify model-facing loop guidance and cached-read recovery.
## [v0.51.14](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.14) - 2026-09-07

### Fixes
- Compact replayed file reads to prevent duplicate context growth.
- Route shell-based file inspections to native tools.
- Make workspace-change loop recovery task-aware.
- Add provider-aware context accounting and visible checkpoints.
## [v0.51.13](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.13) - 2026-09-07

### Features
- Preserve structured MCP tool results for downstream consumers.
- Record redacted per-session model and configuration snapshots for debugging and reproducibility.
## [v0.51.12](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.12) - 2026-09-07

### Fixes
- Group single MCP tool activity under the `Running`/`Ran` UI headings.
- Correct `view_file` continuation guidance to use schema-valid range arguments.
- Reject semantically duplicate `view_file` reads before parallel execution.
## [v0.51.11](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.11) - 2026-09-07

### Fixes
- Fixed JSON Schema type-union validation for MCP and provider tool schemas.
- Removed obsolete `.superpowers/` tracking from the repository.
## [v0.51.10](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.10) - 2026-09-07

### Fixes
- Pass configured environment variables to MCP server processes and capture their stderr in debug logs.
- Fail pending MCP requests promptly when a server exits, with regression coverage for environment forwarding and early shutdown.
## [v0.51.9](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.9) - 2026-09-07

### Fixes
- Harden provider turn handling so mismatched tool-call results return explicit validation errors instead of panicking.
- Handle subagents removed between state locks without panicking, and steer file-content searches toward the bounded grep tool.
- Prevent system notices from retriggering automatic conversation recaps after idle time.
- Compact the `/context` usage modal to remove excess blank space while preserving its category breakdown.
- Make release automation dry runs side-effect free through the tag and publishing phases.
## [v0.51.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.8) - 2026-09-07

### Fixes
# v0.51.8 — Fixes

- Fix batch validation attribution so schema errors map to the failing tool-call ID only (#982).
- Raise the mutating batch limit from 1 to 4 and exempt read-only shell inspection from the budget (#983).
- Bound loop recovery to 3 rounds, gate cross-turn diagnostics on real stagnation, and keep autonomous turns on round/token budgets (#984).
- Keep prompt history immutable under pruning pressure via head-compaction and render-time dedup (#985).
- Drop emoji prefixes from YOLO notices and info text.
## [v0.51.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.7) - 2026-09-06

### Fixes
- Avoid false loop escalation when a tool batch also produces fresh evidence; clarify shell-call limits in recovery feedback.
- Preserve useful historical tool evidence with recoverable full-output artifacts and truthful completeness metadata; delay pruning until context pressure and correct reclaimed-token accounting.
- Treat nonzero compiler exits as failures even without structured diagnostics, and cancel verification processes and their descendants on cancellation or timeout.
- Prevent UTF-8 truncation panics in runtime context fragments and combined context tails.
## [v0.51.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.6) - 2026-09-06

### Fixes
- Keep large-paste framing out of session and terminal titles, including older saved titles.
- Align the Conversation recap divider with the left edge of other separators while preserving its text gutter.
- Avoid duplicate compiler checks and stale diagnostics after edits; preserve build-verification uncertainty.
- Show models the effective tool-call limit, warn near the round budget, and retain the complete explanation when unfinished work stops.
- Moved the live tokens-per-second metric from the activity line into the composer footer.
- Rendered YOLO mode toggles as clear, centered in-chat status separators.
## [v0.51.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.5) - 2026-09-04

### Fixes
- Recover truncated tool-call continuations by escalating the output budget from 8K to 16K when the response is incomplete.
- Bound profile-specific tool output limits and malformed tool-call history.
- Improve cancelled-turn display and harden release automation verification.
## [v0.51.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.4) - 2026-09-04

### Fixes
- **Context modal spacing:** Restore top padding for the left token grid while retaining model summary spacing ([#937](https://github.com/LHagfoss/rustcode/issues/937); [#957](https://github.com/LHagfoss/rustcode/pull/957)).
- **Conversation recap alignment:** Add standard left message padding to recap dividers and content while preserving narrow-terminal wrapping ([#954](https://github.com/LHagfoss/rustcode/issues/954); [#956](https://github.com/LHagfoss/rustcode/pull/956)).

## [v0.51.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.3) - 2026-09-03

### Fixes
- **Idle recap cleanup:** Remove leaked model reasoning, bound automatic recaps by words without visible ellipsis truncation, suppress empty reasoning-only recaps, and match the `Worked for` separator layout ([#952](https://github.com/LHagfoss/rustcode/pull/952)).

## [v0.51.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.2) - 2026-09-03

### Fixes
- **Codex-style idle conversation recaps:** Show automatic summaries as compact one- or two-sentence `Conversation recap` blocks, keep them out of model context, and preserve detailed manual `/summarize` output ([#950](https://github.com/LHagfoss/rustcode/pull/950)).

## [v0.51.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.1) - 2026-09-03

### Fixes
- **Idle conversation recaps:** Give automatically generated idle summaries a deterministic `Conversation recap` heading without changing manual summaries ([#947](https://github.com/LHagfoss/rustcode/pull/947)).

## [v0.51.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.51.0) - 2026-09-03

### Features
- **Isolated task workspaces:** Add opt-in named Git worktrees with durable descriptors, resume/status/handoff flows, scoped execution context, and guarded cleanup for tasks and delegated agents ([#943](https://github.com/LHagfoss/rustcode/issues/943); [#944](https://github.com/LHagfoss/rustcode/pull/944)).
- **Idle session summaries:** Automatically summarize unchanged chats after user inactivity and persist the result as a normal assistant message ([#945](https://github.com/LHagfoss/rustcode/pull/945)).

## [v0.50.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.50.2) - 2026-09-02

### Fixes
- **Read-only tool batching:** Preserve read-only calls when mutation limits apply, group mixed tool batches and adjacent tool-only turns coherently, and avoid unnecessary recovery from expected shell wrapper statuses ([#938](https://github.com/LHagfoss/rustcode/issues/938); [#939](https://github.com/LHagfoss/rustcode/pull/939)).

## [v0.50.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.50.0) - 2026-09-02

### Fixes
- **Completed review preservation:** Preserve completed read-only inspection work when final synthesis encounters a reasoning loop, while requiring explicit provider terminal state and content-bearing evidence before accepting completion ([#932](https://github.com/LHagfoss/rustcode/pull/932); tracked by [#931](https://github.com/LHagfoss/rustcode/issues/931)).

## [v0.40.9](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.9) - 2026-09-02

### Fixes
- **Incomplete recovery status:** Keep bounded recovery, unrecoverable tool calls, and detected corruption loops explicitly incomplete instead of reporting false success ([#926](https://github.com/LHagfoss/rustcode/pull/926); tracked by [#921](https://github.com/LHagfoss/rustcode/issues/921)).
- **Provider budgets:** Separate reasoning, output, and effective context budgets, preserving legacy reasoning settings while honoring explicit provider context limits ([#928](https://github.com/LHagfoss/rustcode/pull/928); tracked by [#922](https://github.com/LHagfoss/rustcode/issues/922)).
- **Durable compaction:** Persist compaction boundaries and retained-history anchors across requests and session reloads for reliable long-context operation ([#927](https://github.com/LHagfoss/rustcode/pull/927); tracked by [#923](https://github.com/LHagfoss/rustcode/issues/923)).
- **Inspection results:** Unify inspection result metadata, preserve actionable omitted ranges, and narrow inspection tool exposure across equivalent tools ([#929](https://github.com/LHagfoss/rustcode/pull/929); tracked by [#924](https://github.com/LHagfoss/rustcode/issues/924)).

## [v0.40.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.8) - 2026-09-01

### Fixes
- **Tool result contracts:** Mark bounded model-visible tool results explicitly incomplete and preserve that state through replay, compaction, subagents, and ACP output ([#915](https://github.com/LHagfoss/rustcode/issues/915)).
- **Inspection-loop recovery:** Detect repeated cross-tool inspection of unchanged targets across distinct model turns without interrupting progressive range reads or successful verification ([#916](https://github.com/LHagfoss/rustcode/issues/916)).

## [v0.40.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.7) - 2026-09-01

### Fixes
- **Agent recovery:** Clear stale pre-verification reasoning after authoritative or novel command evidence so a successful verification-to-summary transition is not misclassified as a reasoning loop, while retaining repeated-verification protection ([#911](https://github.com/LHagfoss/rustcode/pull/911); tracked by [#910](https://github.com/LHagfoss/rustcode/issues/910))

## [v0.40.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.6) - 2026-09-01

### Features
- **Generation limits:** Allow ordinary requests to use provider defaults, select output-token fields by provider capability or explicit profile override, and keep tool and recovery rounds safely bounded ([#907](https://github.com/LHagfoss/rustcode/pull/907); tracked by [#904](https://github.com/LHagfoss/rustcode/issues/904))
- **Mutation policy:** Make the per-response mutation limit configurable per model profile while retaining the safe default of one and enforcing the same policy for root and subagent turns ([#908](https://github.com/LHagfoss/rustcode/pull/908); tracked by [#905](https://github.com/LHagfoss/rustcode/issues/905))

### Fixes
- **Provider streaming:** Reassemble native streamed tool calls by index or stable ID, record bounded sanitized structural traces, and reject length-stopped calls before execution ([#902](https://github.com/LHagfoss/rustcode/pull/902); tracked by [#896](https://github.com/LHagfoss/rustcode/issues/896), [#897](https://github.com/LHagfoss/rustcode/issues/897), and [#901](https://github.com/LHagfoss/rustcode/issues/901))
- **Tool contracts:** Persist explicit completeness for file, search, glob, and directory results; preserve it through replay and truncation without inferring state from rendered text ([#902](https://github.com/LHagfoss/rustcode/pull/902), [#906](https://github.com/LHagfoss/rustcode/pull/906); tracked by [#898](https://github.com/LHagfoss/rustcode/issues/898) and [#903](https://github.com/LHagfoss/rustcode/issues/903))
- **Tool execution safety:** Bound agent-generated tool calls and serialize workspace-changing operations so each mutation is grounded in the prior result ([#894](https://github.com/LHagfoss/rustcode/pull/894), [#895](https://github.com/LHagfoss/rustcode/pull/895))
- **Quota parsing:** Restore resilient provider quota deserialization for schema drift ([#893](https://github.com/LHagfoss/rustcode/pull/893))

### Performance
- **Context and prompt efficiency:** Unify compaction, use provider usage at valid turn boundaries, retain deterministic file inventories, reduce the base prompt, and recover safe conventional tool aliases ([#891](https://github.com/LHagfoss/rustcode/pull/891), [#902](https://github.com/LHagfoss/rustcode/pull/902); tracked by [#899](https://github.com/LHagfoss/rustcode/issues/899) and [#900](https://github.com/LHagfoss/rustcode/issues/900))
- **Runtime allocations:** Reduce symbol parsing, streaming, and quota-deserialization allocation overhead ([#892](https://github.com/LHagfoss/rustcode/pull/892))

### Documentation
- **Runtime architecture:** Document task lifecycle and ACP runtime behavior ([#890](https://github.com/LHagfoss/rustcode/pull/890))

## [v0.40.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.5) - 2026-08-31

### Features
- **Background tasks:** Add a dependency-neutral task manager and route background command lifecycle through it with session-scoped subscriptions, durable completion delivery, ACP turn resumption, and stale-event isolation ([#884](https://github.com/LHagfoss/rustcode/pull/884), [#886](https://github.com/LHagfoss/rustcode/pull/886), [#887](https://github.com/LHagfoss/rustcode/pull/887), [#888](https://github.com/LHagfoss/rustcode/pull/888); tracked by [#881](https://github.com/LHagfoss/rustcode/issues/881))

### Fixes
- **Process cancellation:** Terminate complete Unix process groups and Windows process trees when foreground commands are cancelled or time out, while preserving exactly one final tool result ([#880](https://github.com/LHagfoss/rustcode/pull/880))

### Performance
- **Build measurement:** Add a safe Cargo build-boundary benchmark harness and high-resolution timing so clean, warm, and focused rebuild costs can be measured reliably ([#875](https://github.com/LHagfoss/rustcode/pull/875), [#878](https://github.com/LHagfoss/rustcode/pull/878))

### Refactoring
- **Workspace decomposition:** Establish a thin library entry point and extract stable domain types, executable-path helpers, session persistence, tool-protocol parsing, filesystem tools, command execution, lifecycle state, and loop detection into focused workspace crates ([#871](https://github.com/LHagfoss/rustcode/pull/871), [#872](https://github.com/LHagfoss/rustcode/pull/872), [#873](https://github.com/LHagfoss/rustcode/pull/873), [#874](https://github.com/LHagfoss/rustcode/pull/874), [#876](https://github.com/LHagfoss/rustcode/pull/876), [#877](https://github.com/LHagfoss/rustcode/pull/877), [#879](https://github.com/LHagfoss/rustcode/pull/879), [#883](https://github.com/LHagfoss/rustcode/pull/883), [#885](https://github.com/LHagfoss/rustcode/pull/885); continuation of [#869](https://github.com/LHagfoss/rustcode/issues/869))

### CI & Release
- **Build coverage:** Drop Intel macOS artifacts, keep Apple Silicon macOS as the supported native target, make native portability checks advisory, and enable sccache-backed CI builds ([#868](https://github.com/LHagfoss/rustcode/pull/868), [#870](https://github.com/LHagfoss/rustcode/pull/870))
- **Workspace CI:** Ensure changes under extracted crates and CI helpers run the required Linux test and cross-platform portability coverage ([#882](https://github.com/LHagfoss/rustcode/pull/882))

## [v0.40.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.4) - 2026-08-31

### Fixes
- **Streaming recovery:** Classify SSE failures, retry recoverable body-phase interruptions once from a coherent checkpoint, and preserve verified completions when a later transport failure occurs ([#866](https://github.com/LHagfoss/rustcode/pull/866))
- **Cross-tool verification loops:** Recognize equivalent file inspection through native tools and shell commands while grounding recovery in unchanged file revisions ([#865](https://github.com/LHagfoss/rustcode/pull/865))

### Performance
- **Phase-aware bootstrap:** Reduce tool-schema overhead in empty and near-empty workspaces, promote the full schema after source creation, and nudge planning-only bootstrap turns toward action ([#864](https://github.com/LHagfoss/rustcode/pull/864))

## [v0.40.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.3) - 2026-08-30

### Fixes
- **Verified final completion:** Accept substantive final prose as completion after fresh successful verification and a green build gate, while keeping stale, failed, thought-only, and forced-recovery responses blocked ([#856](https://github.com/LHagfoss/rustcode/pull/856), [`40a06d4`](https://github.com/LHagfoss/rustcode/commit/40a06d4))

## [v0.40.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.2) - 2026-08-30

### Fixes
- **Verification loops:** Count a successful verification command as fresh evidence only once per unchanged workspace generation, normalize equivalent command variants, and redirect repeated green checks toward unverified user-visible behavior ([#854](https://github.com/LHagfoss/rustcode/pull/854), [`aa56e6d`](https://github.com/LHagfoss/rustcode/commit/aa56e6d))

## [v0.40.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.1) - 2026-08-30

### Fixes
- **Agent recovery:** Keep recovery instructions aligned with the active tool protocol, strip serialized tool syntax from loop-escalation finals, and coalesce repeated loop warnings so useful context remains visible ([#852](https://github.com/LHagfoss/rustcode/pull/852), [`916fe0a`](https://github.com/LHagfoss/rustcode/commit/916fe0a))
- **Command verification:** Propagate failures from every Unix pipeline stage so commands such as `cargo test | tail` cannot falsely report successful verification ([#852](https://github.com/LHagfoss/rustcode/pull/852), [`916fe0a`](https://github.com/LHagfoss/rustcode/commit/916fe0a))
- **Skill routing:** Recognize distinctive components of compound skill names, allowing prompts such as “release” to route to `release-automation` without injecting the full skill into context ([#852](https://github.com/LHagfoss/rustcode/pull/852), [`916fe0a`](https://github.com/LHagfoss/rustcode/commit/916fe0a))
- **Portable UI tests:** Isolate global theme state in render snapshots and keep the shell helper available on Windows ([#852](https://github.com/LHagfoss/rustcode/pull/852), [`916fe0a`](https://github.com/LHagfoss/rustcode/commit/916fe0a))

## [v0.40.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.40.0) - 2026-08-30

### Features
- **Reasoning model tool rounds:** Cap tool-enabled requests at 8192 tokens for reasoning-capable models while preserving the full completion budget for final prose turns ([`completion_token_limit`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **Headless background tasks:** The raw CLI now supports background command completion via wakeup callbacks, injecting terminal results into session history and resuming the turn ([`raw_cli.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **MCP schema relevance filtering:** Builtin native tools are now filtered by context terms — coding requests keep only the core coding tools, while media/audio requests surface the relevant specialized tools ([`tools/schema.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **oMLX reasoning controls:** Pass `enable_thinking` through `chat_template_kwargs` for oMLX-compatible servers that expose reasoning controls this way ([`stream_request.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **Tool choice required for recovery:** When a thinking-disabled recovery turn needs a tool call, the request now sets `tool_choice: "required"` to force action emission ([`stream_request.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))

### Fixes
- **Deadlock in compiler verification:** Release the application-state mutex before running the potentially slow compiler check in `handle_tool_response`, then reacquire for history/status updates ([`turn/tools.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **Loop detection false resets:** Behavioral history now only resets on actual workspace edits, not on reads alone — prevents local models from accumulating "I will edit now" hesitations indefinitely ([`loop_detect/reasoning.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **Continuation amplification:** Reduced max provider continuations from 5 to 2 to avoid amplifying incomplete structured tool calls, especially from local models that restart large writes from byte zero ([`runner.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **No-progress escalation:** Reduced max consecutive no-progress turns from 8 to 6 for faster escalation when the agent is spinning ([`network.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **Reasoning budget finish reason:** Treat `reasoning_budget` the same as `reasoning_loop` in recovery and cut-off detection ([`text.rs`, `recovery.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))

### Performance
- **MCP schema selection:** Added a relevance threshold (6) and core-tool fallback ordering to prevent generic descriptions from pulling large MCP suites into unrelated requests ([`tools/schema.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **Recovery thinking budget:** Recovery turns now use a small deterministic 1024-token thinking budget instead of the full configured budget, keeping recovery focused on action emission ([`stream_request.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))

### Refactoring
- **System prompt:** Streamlined the system prompt — removed redundant rules, consolidated workflow guidance, and tightened the skills section ([`tools/schema.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))
- **Tool descriptions:** Improved `view_file` and `grep` descriptions to better guide model behavior around the 800-line cap and ripgrep usage ([`filesystem.rs`, `search.rs`](https://github.com/LHagfoss/rustcode/commit/757559f))

## [v0.39.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.39.0) - 2026-08-28

### Features
- **ACP:** Restructure the Agent Client Protocol server around explicit session state, permissions, and model configuration boundaries ([#840](https://github.com/LHagfoss/rustcode/pull/840), [`61e78ff`](https://github.com/LHagfoss/rustcode/commit/61e78ff))
- **Streaming UI:** Show live response decode speed while a model is generating ([#848](https://github.com/LHagfoss/rustcode/pull/848), [`8512828`](https://github.com/LHagfoss/rustcode/commit/8512828))

### Fixes
- **Session History:** Make `/history` session selection replace the active transcript, clear session-specific UI and subagent state, and reject late streaming updates from the previous session ([#849](https://github.com/LHagfoss/rustcode/pull/849), [`4443cec`](https://github.com/LHagfoss/rustcode/commit/4443cec))
- **UI State:** Omit inactive overlay payloads from render snapshots so closed pickers cannot retain stale presentation state ([#842](https://github.com/LHagfoss/rustcode/pull/842), [`2000be7`](https://github.com/LHagfoss/rustcode/commit/2000be7))

### Performance
- **Provider Requests:** Reduce allocation amplification while constructing provider request payloads ([#841](https://github.com/LHagfoss/rustcode/pull/841), [`636fbb5`](https://github.com/LHagfoss/rustcode/commit/636fbb5))

### Refactoring
- **Runtime:** Split turn execution, application runtime, context compaction, command authorization, and tool execution support into focused ownership boundaries ([#843](https://github.com/LHagfoss/rustcode/pull/843), [#844](https://github.com/LHagfoss/rustcode/pull/844), [#845](https://github.com/LHagfoss/rustcode/pull/845), [#846](https://github.com/LHagfoss/rustcode/pull/846), [#847](https://github.com/LHagfoss/rustcode/pull/847))

## [v0.38.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.38.1) - 2026-08-28

### Fixes
- **Audio Generation:** Treat signal-killed local audio backends as non-retryable and avoid reporting output paths for failed generation attempts ([#837](https://github.com/LHagfoss/rustcode/pull/837), [`9828d44`](https://github.com/LHagfoss/rustcode/commit/9828d44))
- **Harness:** Stop cross-tool read loops when models repeatedly inspect the same file region through different native and shell readers ([#838](https://github.com/LHagfoss/rustcode/pull/838), [`720c93d`](https://github.com/LHagfoss/rustcode/commit/720c93d))

## [v0.38.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.38.0) - 2026-08-27

### Features
- **Background Terminals:** Show live background-terminal status and commands in the footer, with `/ps` to inspect tasks and `/stop` to terminate them ([#834](https://github.com/LHagfoss/rustcode/pull/834), [`6918f36`](https://github.com/LHagfoss/rustcode/commit/6918f36))

### Fixes
- **UI:** Move the Ctrl+C exit confirmation to the left side of the footer while preserving context usage on the right ([#835](https://github.com/LHagfoss/rustcode/pull/835), [`9254083`](https://github.com/LHagfoss/rustcode/commit/9254083))

### Refactoring
- **UI:** Split the main rendering module into focused components ([#830](https://github.com/LHagfoss/rustcode/pull/830), [`d18a7b`](https://github.com/LHagfoss/rustcode/commit/d18a7b))
- **UI:** Split modal rendering into focused modal families ([#831](https://github.com/LHagfoss/rustcode/pull/831), [`8df4b77`](https://github.com/LHagfoss/rustcode/commit/8df4b77))
- **App:** Split action handlers by responsibility ([#832](https://github.com/LHagfoss/rustcode/pull/832), [`087339f`](https://github.com/LHagfoss/rustcode/commit/087339f))
- **App:** Split state models and tests into focused modules ([#833](https://github.com/LHagfoss/rustcode/pull/833), [`8763898`](https://github.com/LHagfoss/rustcode/commit/8763898))

### CI & Release
- **CI:** Avoid compiling the full test suite twice in the required Linux job ([#829](https://github.com/LHagfoss/rustcode/pull/829), [`7ea4f25`](https://github.com/LHagfoss/rustcode/commit/7ea4f25))

## [v0.37.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.37.0) - 2026-08-27

### Features
- **Video:** Show the render plan and live progress while rendering video projects ([#821](https://github.com/LHagfoss/rustcode/pull/821), [`41a28a9`](https://github.com/LHagfoss/rustcode/commit/41a28a9))

### Fixes
- **Video:** Pad short clip audio so rendered clips keep their declared duration ([#818](https://github.com/LHagfoss/rustcode/pull/818), [`55e3057`](https://github.com/LHagfoss/rustcode/commit/55e3057))
- **Video:** Share project media preflight across render steps to avoid redundant checks ([#817](https://github.com/LHagfoss/rustcode/pull/817), [`9062448`](https://github.com/LHagfoss/rustcode/commit/9062448))
- **Video:** Bound FFmpeg process output capture to prevent unbounded memory growth ([#820](https://github.com/LHagfoss/rustcode/pull/820), [`2110cd8`](https://github.com/LHagfoss/rustcode/commit/2110cd8))
- **UI:** Time out the double Ctrl-C exit confirmation so a stuck terminal can always exit ([#822](https://github.com/LHagfoss/rustcode/pull/822), [`2370511`](https://github.com/LHagfoss/rustcode/commit/2370511))
- **IO:** Share the atomic output replacement path across writers ([#819](https://github.com/LHagfoss/rustcode/pull/819), [`71496e4`](https://github.com/LHagfoss/rustcode/commit/71496e4))

### Refactoring
- **Tools:** Split tool schema parsing from dispatch for clearer ownership ([#827](https://github.com/LHagfoss/rustcode/pull/827), [`7d2932b`](https://github.com/LHagfoss/rustcode/commit/7d2932b))
- **Network:** Split request loop detection into dedicated modules ([#826](https://github.com/LHagfoss/rustcode/pull/826), [`575d5cd`](https://github.com/LHagfoss/rustcode/commit/575d5cd))
- **Config:** Split session persistence into its own module ([#825](https://github.com/LHagfoss/rustcode/pull/825), [`b88f3ea`](https://github.com/LHagfoss/rustcode/commit/b88f3ea))

### CI & Release
- **CI:** Add cross-platform pull request coverage ([#823](https://github.com/LHagfoss/rustcode/pull/823), [`5087e81`](https://github.com/LHagfoss/rustcode/commit/5087e81))
- **Release:** Verify platform artifacts before publishing ([#824](https://github.com/LHagfoss/rustcode/pull/824), [`b12e16f`](https://github.com/LHagfoss/rustcode/commit/b12e16f))

## [v0.36.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.36.0) - 2026-08-24

### Features
- **Native Video Editing:** Add declarative tools for inspecting media, validating video projects, and rendering video through project-local FFmpeg processes ([#815](https://github.com/LHagfoss/rustcode/pull/815), [`fecef33`](https://github.com/LHagfoss/rustcode/commit/fecef33))

## [v0.35.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.35.4) - 2026-08-24

### CI & Reliability
- **CI:** Run Rust checks on pull requests to catch formatting and test regressions before merge ([#810](https://github.com/LHagfoss/rustcode/pull/810), [`cb97534`](https://github.com/LHagfoss/rustcode/commit/cb97534))
- **Agent Tasks:** Preserve background task outcomes so completed work is not lost ([#808](https://github.com/LHagfoss/rustcode/pull/808), [`500d650`](https://github.com/LHagfoss/rustcode/commit/500d650))

## [v0.35.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.35.3) - 2026-08-23

### Tests & Release Hygiene
- **UI Snapshots:** Resolve the rendered-version golden fixture from the package version so release bumps do not require manual snapshot edits ([`0e11a12`](https://github.com/LHagfoss/rustcode/commit/0e11a12))

## [v0.35.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.35.2) - 2026-08-23

### Local Models & Recovery
- **Local Models:** Reduce the implicit completion reserve for local profiles to 4096 tokens while preserving explicit per-profile overrides ([`89579d6`](https://github.com/LHagfoss/rustcode/commit/89579d6))
- **Response Recovery:** Retry one empty model completion with a concise answer-focused prompt instead of immediately failing the turn ([`a447080`](https://github.com/LHagfoss/rustcode/commit/a447080))
- **Reasoning Recovery:** Return a concise terminal response after repeated reasoning loops instead of launching another verbose wrap-up request ([`5d9c340`](https://github.com/LHagfoss/rustcode/commit/5d9c340))

## [v0.35.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.35.1) - 2026-08-23

### Skills & Performance
- **Skill Routing:** Route named skills through a fast path to reduce discovery overhead ([`5bcf281`](https://github.com/LHagfoss/rustcode/commit/5bcf281))
- **Skill Routing:** Stabilize the skill-routing cache for consistent repeated lookups ([`ab14758`](https://github.com/LHagfoss/rustcode/commit/ab14758))

## [v0.35.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.35.0) - 2026-08-23

### Performance
- **UI/Rendering:** Render from immutable state snapshots to release the state lock during draw, eliminating lock contention between the event loop and render path ([#799](https://github.com/LHagfoss/rustcode/pull/799))
- **UI/Rendering:** Share history, streamed response, and subagent history snapshots via `Arc` to avoid redundant clones on every frame ([#799](https://github.com/LHagfoss/rustcode/pull/799))
- **History:** Track history revisions across turns and deduplicate queued snapshot writes, avoiding redundant disk I/O on unchanged history ([#789](https://github.com/LHagfoss/rustcode/pull/789), [#790](https://github.com/LHagfoss/rustcode/pull/790))
- **History:** Make message pruning linear instead of quadratic by sorting and deduplicating in a single pass ([#792](https://github.com/LHagfoss/rustcode/pull/792))
- **Network:** Cache serialized tool schemas and request bytes across turns to avoid repeated JSON serialization ([#793](https://github.com/LHagfoss/rustcode/pull/793))
- **MCP:** Stabilize MCP tool selection to avoid unnecessary schema recomputation ([#794](https://github.com/LHagfoss/rustcode/pull/794))
- **Streaming:** Reduce periodic streaming redraws by coalescing rapid frame requests ([#795](https://github.com/LHagfoss/rustcode/pull/795))
- **Tools:** Await web search tool calls concurrently instead of sequentially ([#798](https://github.com/LHagfoss/rustcode/pull/798))

### Build
- **Release Profile:** Optimize release binary linking for faster startup and smaller binary size ([#797](https://github.com/LHagfoss/rustcode/pull/797))
- **Dependencies:** Trim unused async runtime features to reduce compile times and binary weight ([#796](https://github.com/LHagfoss/rustcode/pull/796))

### Fixes
- **Compaction:** Exclude stubbed tool outputs from the compaction context window cap to prevent over-truncation ([#791](https://github.com/LHagfoss/rustcode/pull/791))

## [v0.34.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.34.3) - 2026-08-23

### Tools & Safety
- **Tools/File Editing:** Preserve exact line ending conventions (CRLF vs LF) and mixed newline styles faithfully across file modifications in `replace_file_content` and `write_to_file` ([#787](https://github.com/LHagfoss/rustcode/pull/787))
- **Tools/Safety & Bounds:** Bound unbounded read/search paths and memory consumption across tool execution, context generation, and output capture ([#787](https://github.com/LHagfoss/rustcode/pull/787))

## [v0.34.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.34.2) - 2026-08-22

### Fixes & UI
- **Commands/Update:** Run `/update` in a dedicated background task to prevent blocking the UI loop and ensure smooth live status feedback ([#784](https://github.com/LHagfoss/rustcode/pull/784))
- **UI/Modals:** Preserve and restore active streaming/working status when modal overlays close, keeping native terminal progress indicators active while processing responses ([#785](https://github.com/LHagfoss/rustcode/pull/785))

## [v0.34.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.34.1) - 2026-08-22

### Fixes
- **MCP/Startup Warnings:** Log background MCP server warnings and errors cleanly to debug logs and defer user-facing summary to application exit instead of interrupting active terminal rendering ([#782](https://github.com/LHagfoss/rustcode/pull/782))

## [v0.34.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.34.0) - 2026-08-22

### Features
- **UI/Terminal Progress:** Add native terminal task progress bar reporting via OSC 9;4 for supported terminal emulators ([#780](https://github.com/LHagfoss/rustcode/pull/780))
- **UI/Mode Selection:** Convert `/yolo` command to an interactive mode picker modal ([#765](https://github.com/LHagfoss/rustcode/pull/765))
- **Verification/Skip Non-Code:** Allow task completion without full compiler verification when changes are strictly non-code files (docs, assets, markdown) ([#769](https://github.com/LHagfoss/rustcode/pull/769), [#771](https://github.com/LHagfoss/rustcode/pull/771), [#775](https://github.com/LHagfoss/rustcode/pull/775))

### Tools & Search
- **Tools/View File:** Expand standard `view_file` read window to 800 lines ([#766](https://github.com/LHagfoss/rustcode/pull/766))
- **Tools/Search:** Enforce 32KB output limit on `grep_search` with truncation notifications, and return permitted lines at the file limit cap ([#766](https://github.com/LHagfoss/rustcode/pull/766), [#772](https://github.com/LHagfoss/rustcode/pull/772), [#776](https://github.com/LHagfoss/rustcode/pull/776))
- **Tools/Replace File Content:** Declare all handler-accepted parameter aliases in native tool schema, align with root `old_string`/`new_string`, and require strong prefix proofs for idempotency guards ([#767](https://github.com/LHagfoss/rustcode/pull/767), [#770](https://github.com/LHagfoss/rustcode/pull/770), [#773](https://github.com/LHagfoss/rustcode/pull/773), [#777](https://github.com/LHagfoss/rustcode/pull/777))
- **Tools/Validation:** Accept string-encoded integers leniently for built-in tool arguments ([#778](https://github.com/LHagfoss/rustcode/pull/778), [#779](https://github.com/LHagfoss/rustcode/pull/779))

### Loop Detection & Safety
- **Harness/Loop Detection:** Bucket edit category signatures by `start_line` parsing string formats cleanly, and set `manage_task` authorization safety level to `ProcessControl` ([#768](https://github.com/LHagfoss/rustcode/pull/768), [#774](https://github.com/LHagfoss/rustcode/pull/774))

## [v0.33.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.33.3) - 2026-08-22

### UI & Fixes
- **UI/Composer:** Wrap composer input at natural word boundaries while keeping long uninterrupted tokens hard-wrapped and maintaining precise cursor alignment ([#763](https://github.com/LHagfoss/rustcode/pull/763))
- **UI/User Messages:** Add vertical padding rows to committed user message cards for balanced visual alignment ([#762](https://github.com/LHagfoss/rustcode/pull/762))
- **UI/Tool Calls:** Hide speculative tool placeholders until meaningful arguments or execution starts to avoid empty tool lines ([#761](https://github.com/LHagfoss/rustcode/pull/761))
- **CI/Builds:** Optimize Windows release build profile to speed up CI turnaround ([#760](https://github.com/LHagfoss/rustcode/pull/760))

## [v0.33.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.33.2) - 2026-08-21

### UI & Polish
- **UI/User Messages:** Render committed user messages with composer panel background styling and full transcript width padding ([#757](https://github.com/LHagfoss/rustcode/pull/757))
- **UI/Pasted Chips:** Style pasted image and long-text chips with the theme accent color while preserving inline boundaries ([#758](https://github.com/LHagfoss/rustcode/pull/758))
- **UI/Audio Tools:** Group live audio generation tool calls with the editing tools style and display destination file paths ([#756](https://github.com/LHagfoss/rustcode/pull/756))
- **UI/Separators:** Remove emoji from the new chat separator banner for consistent clean typography ([#755](https://github.com/LHagfoss/rustcode/pull/755))

## [v0.33.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.33.1) - 2026-08-21

### Fixes
- **Providers/Gemini:** Probe Gemini gateway function-calling capability dynamically and normalize native tool schemas by stripping unsupported JSON Schema metadata and converting exclusive minimum bounds ([#753](https://github.com/LHagfoss/rustcode/pull/753))

## [v0.33.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.33.0) - 2026-08-21

### Features
- **Audio/Generation:** Add native local audio generation tools `generate_sound_effect`, `generate_music`, and `inspect_audio` powered by cancellable local MLX backends on Apple Silicon ([#750](https://github.com/LHagfoss/rustcode/pull/750))
- **Audio/Environment:** Propagate deterministic audio backend search PATH to child generation processes and ensure isolated venv compatibility ([#751](https://github.com/LHagfoss/rustcode/pull/751))

## [v0.32.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.32.1) - 2026-08-21

### Fixes & UI
- **UI/Tool Calls:** Omit redundant 'Edit' action words under file editing headers, expand case-insensitive editing tool aliases, and standardize live generic tool headers to `Running` ([#747](https://github.com/LHagfoss/rustcode/pull/747), [#748](https://github.com/LHagfoss/rustcode/pull/748))
- **UI/Layout & Dividers:** Align live transcript left margin at column 0 with committed scrollback, and normalize turn divider vertical padding strictly to 1 blank line ([#747](https://github.com/LHagfoss/rustcode/pull/747), [#748](https://github.com/LHagfoss/rustcode/pull/748))

## [v0.32.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.32.0) - 2026-08-21

### Features & Performance
- **Skills/On-Demand:** Replace static embedded skill catalog with on-demand skill discovery to minimize initial context footprint ([#744](https://github.com/LHagfoss/rustcode/pull/744))
- **Agent/Context Optimization:** Compress core system prompt, gate irrelevant MCP tool schemas with fallback, isolate subagent schemas, and deduplicate stable runtime context fields ([#744](https://github.com/LHagfoss/rustcode/pull/744))
- **Token Budgeting:** Account for native tool schemas directly in token budgeting, context trimming, and usage telemetry ([#744](https://github.com/LHagfoss/rustcode/pull/744))

### Fixes
- **UI/Terminal Resizing:** Purge scrollback and replay committed transcript history cleanly upon terminal resize events to preserve layout and viewport geometry ([#745](https://github.com/LHagfoss/rustcode/pull/745))

## [v0.31.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.31.4) - 2026-08-20

### Fixes
- **Agent/Reasoning Recovery:** Omit reasoning effort and thinking budget when thinking is disabled, disable thinking during reasoning-loop recovery attempts, and omit native tool schemas on forced-final requests ([#741](https://github.com/LHagfoss/rustcode/pull/741))
- **UI/Dividers:** Ensure proper blank line top-padding above turn work and resumed session dividers to avoid clipping into preceding tool output ([#742](https://github.com/LHagfoss/rustcode/pull/742))

## [v0.31.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.31.3) - 2026-08-20

### Fixes
- **Docs/Changelog:** Correct release ordering and remove duplicated legacy headings so release history renders consistently ([#739](https://github.com/LHagfoss/rustcode/pull/739))

## [v0.31.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.31.2) - 2026-08-20

### Fixes
- **UI/Scrollback & Resizing:** Prevent scrollback reflow corruption on wide viewports by omitting trailing empty cells, and ensure accurate coordinate clearing during rapid terminal resize events ([#736](https://github.com/LHagfoss/rustcode/pull/736))
- **Harness/Loop Detection & Budgeting:** Enforce client-side reasoning token budgets on split thought streams, distinguish substantive commands after leading directory changes, and treat runtime, browser, and read evidence as progress ([#737](https://github.com/LHagfoss/rustcode/pull/737))

## [v0.31.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.31.1) - 2026-08-20

### Features & Fixes
- **Config/Thinking:** Support `thinking_budget` in model profiles and request payloads to enforce hard reasoning token caps on supported providers ([#734](https://github.com/LHagfoss/rustcode/pull/734))
- **Harness/Recovery:** Treat failure replans as recovery opportunities rather than terminal states, resetting the loop detector to allow alternative mutation strategies ([#734](https://github.com/LHagfoss/rustcode/pull/734))

## [v0.31.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.31.0) - 2026-08-20

### Features
- **CLI/Self-Update:** Add native cross-platform self-update engine supporting direct GitHub release binary extraction on macOS, Linux, and Windows with Homebrew fallback ([#731](https://github.com/LHagfoss/rustcode/pull/731))
- **Installation:** Add one-line installation scripts for macOS/Linux (`install.sh`) and Windows PowerShell (`install.ps1`) ([#731](https://github.com/LHagfoss/rustcode/pull/731))

## [v0.30.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.30.4) - 2026-08-20

### Fixes
- **UI/Transcript:** Deduplicate speculative live tool projections upon execution, group completed generic tools under a `Ran` heading with indented children, and clean up trailing blank lines ([#727](https://github.com/LHagfoss/rustcode/pull/727))
- **Harness/Tool Calls:** Avoid false-positive malformed tool call detection on standard `json` markdown code blocks in assistant responses ([#729](https://github.com/LHagfoss/rustcode/pull/729))

## [v0.30.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.30.3) - 2026-08-20

### Fixes
- **UI/Streaming:** Keep the composer footer compact while a response is streaming ([#721](https://github.com/LHagfoss/rustcode/pull/721))
- **UI/Context:** Add the missing top padding above the `/context` token grid ([#722](https://github.com/LHagfoss/rustcode/pull/722))
- **Network/Streaming:** Reset the SSE idle timeout whenever partial bytes arrive, preventing active streams without an immediate newline from timing out ([#723](https://github.com/LHagfoss/rustcode/pull/723))
- **Skills:** Preserve UTF-8 boundaries when truncating large skill files and use platform-native separators for extra skill directories ([#724](https://github.com/LHagfoss/rustcode/pull/724))
- **Privacy/Clipboard:** Stop persisting copied clipboard contents in debug logs while retaining byte-count diagnostics ([#725](https://github.com/LHagfoss/rustcode/pull/725))

## [v0.30.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.30.2) - 2026-08-19

### Features
- **UI/Startup:** Show the active model reasoning effort and effective context window in the welcome banner, with `/effort` and `/context` shortcuts ([#718](https://github.com/LHagfoss/rustcode/pull/718))

### Fixes
- **Windows/Terminal:** Ignore unsupported keyboard-enhancement teardown during normal terminal cleanup, avoiding a spurious error on exit ([#719](https://github.com/LHagfoss/rustcode/pull/719))
- **Config/Sync:** Commit the local config snapshot before the first rebase pull when a newly initialized sync repository has no local `HEAD` ([#719](https://github.com/LHagfoss/rustcode/pull/719))

## [v0.30.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.30.1) - 2026-08-19

### Fixes
- **CLI/Self-Update:** Refresh Homebrew tap metadata before upgrading so newly published versions install even when the local tap checkout is stale ([#716](https://github.com/LHagfoss/rustcode/pull/716))

## [v0.30.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.30.0) - 2026-08-19

### Features
- **Config/TOML:** Migrate configuration system to versioned human-editable `config.toml` with atomic writes, format validation, Unix file permissions, and seamless migration from legacy `config.json` / `models.json` ([#713](https://github.com/LHagfoss/rustcode/pull/713))
- **Config/Projects:** Add project configuration precedence with `.rustcode/config.toml` directory hierarchy loading, secure git-ignored project templates via `rustcode init` / `--init`, and safe override resolution ([#714](https://github.com/LHagfoss/rustcode/pull/714))

## [v0.29.8](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.8) - 2026-08-19

### Fixes
- **CLI/Self-Update:** Validate installed Homebrew formula version after upgrade and clear stale TUI frame before running external process ([#711](https://github.com/LHagfoss/rustcode/pull/711))

## [v0.29.7](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.7) - 2026-08-19

### Features
- **UI/Startup:** Enrich startup welcome box with active git branch context and discoverable help shortcut hint ([#708](https://github.com/LHagfoss/rustcode/pull/708))
- **UI/Footer:** Display current branch and working path in the composer footer alongside active model and context token usage ([#709](https://github.com/LHagfoss/rustcode/pull/709))

## [v0.29.6](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.6) - 2026-08-19

### Fixes
- **Harness/Loop Detection:** Record cross-turn reasoning loop evidence once per tool batch instead of per tool result, preventing multi-tool read-only batches from incorrectly triggering false-positive recovery loops ([#706](https://github.com/LHagfoss/rustcode/pull/706))

## [v0.29.5](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.5) - 2026-08-19

### Features
- **CLI/Self-Update:** Add startup update modal with interactive update/skip options, transparent terminal restoration during Homebrew execution, and unified update runner across startup, `/update`, and `--update` ([#704](https://github.com/LHagfoss/rustcode/pull/704))

## [v0.29.4](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.4) - 2026-08-19

### Features
- **UI/Context:** Add `/context` interactive modal with color-coded token block matrix visualization and percentage category breakdown across messages, tools, system prompt, and free space ([#702](https://github.com/LHagfoss/rustcode/pull/702))

## [v0.29.3](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.3) - 2026-08-19

### Features
- **ACP/Config:** Implement session model selection via config options, exposing configured model profiles with category `model` and updating session endpoint dynamically on change ([#700](https://github.com/LHagfoss/rustcode/pull/700))

## [v0.29.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.2) - 2026-08-19

### Fixes
- **ACP/Streaming:** Prevent reasoning leakage in ACP protocol by making event streaming thought-aware, buffering split `<think>` tags, routing thoughts to `AgentThoughtChunk`, and emitting only prose in `AgentMessageChunk` ([#698](https://github.com/LHagfoss/rustcode/pull/698))

## [v0.29.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.1) - 2026-08-19

### Fixes
- **CLI/ACP:** Add `rustcode acp` subcommand alias alongside `--acp` for compatibility with external harness orchestrators ([#696](https://github.com/LHagfoss/rustcode/pull/696))

## [v0.29.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.29.0) - 2026-08-19

### Features
- **Subagents/Supervisor:** Add session-scoped, semaphore-limited subagent supervisor with per-child cancellation, task ownership, asynchronous execution, and notification-backed wait ([#694](https://github.com/LHagfoss/rustcode/pull/694))
- **ACP/Lifecycle:** Improve ACP session lifecycle with per-session prompt serialization, active prompt cancellation, permission requests for mutating tools with `--yolo` automation opt-in, and streaming updates ([#693](https://github.com/LHagfoss/rustcode/pull/693))
- **UI/Greetings:** Redesign startup greeting banner to a compact, clean layout displaying active model, current working directory (`~` abbreviated), and permission mode (`YOLO mode` / `Interactive`) ([#692](https://github.com/LHagfoss/rustcode/pull/692))

## [v0.28.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.28.2) - 2026-08-19

### Performance
- **Sessions/Resume:** Optimize `rustcode -r` / `rustcode --resume` session startup time from seconds down to ~2ms with lazy reverse scanning, zero-copy metadata parsing with `ChatMessageMetaRef`, and direct $O(1)$ by-id resolution ([#689](https://github.com/LHagfoss/rustcode/pull/689))

### Fixes
- **Runtime/Input:** Auto-recover `TuiEventStream` upon event stream closure or error, restoring keyboard responsiveness (including `Ctrl+C` and `Esc`) after macOS sleep/wake cycles ([#690](https://github.com/LHagfoss/rustcode/pull/690))
- **Network/Sockets:** Configure 15-second TCP keepalive on HTTP client to fail fast and detect dropped sockets rather than hanging indefinitely across system sleep ([#690](https://github.com/LHagfoss/rustcode/pull/690))

## [v0.26.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.26.2) - 2026-08-18

### Fixes
- **UI/Layout:** Align right border on `/help` and system info card boxes (`render_status_panel`) and format `?` keyboard shortcut ([#674](https://github.com/LHagfoss/rustcode/pull/674))
- **UI/Startup:** Add prompt prefix `>_ ` to welcome startup banner header to match `/help` and system info card header styling ([#673](https://github.com/LHagfoss/rustcode/pull/673))

### Performance & CI
- **CI/Build:** Optimize build workflow and compiler settings with parallel mold linking and single-codegen unit for fast multiplatform builds ([#672](https://github.com/LHagfoss/rustcode/pull/672))

## [v0.26.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.26.1) - 2026-08-18

### Features
- **CLI/Self-Update:** Add `--update` flag (with `--upgrade` alias), `/upgrade` command alias, interactive `Checking if new release...` status messages, and animated terminal spinner during Homebrew upgrades ([#670](https://github.com/LHagfoss/rustcode/pull/670))

## [v0.26.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.26.0) - 2026-08-18

### Features
- **Memory/Brain Tools:** Add active agent memory tools `remember`, `recall_memory`, and `forget_memory` with multi-scope support (global `~/.config/rustcode/global-memory.json` and project repository memory) ([#666](https://github.com/LHagfoss/rustcode/pull/666))
- **Context & Cache Optimization:** Strictly clamp passive context tail memory budget to ≤192 tokens to maintain 100% prefix KV-cache stability and prevent prompt bloat on local models ([#666](https://github.com/LHagfoss/rustcode/pull/666))
- **UI/Themes:** Add built-in `sky` theme inspired by summer azure sky (`#3894F0`), meadow green (`#88C438`), sunlight gold (`#FFD152`), and transparent terminal background ([#667](https://github.com/LHagfoss/rustcode/pull/667), [#668](https://github.com/LHagfoss/rustcode/pull/668))

### Fixes
- **UI/Typography:** Style markdown bold text, table headers, and H1–H3 headings with the primary accent color ([#665](https://github.com/LHagfoss/rustcode/pull/665))
- **UI/Dividers:** Lighten `turn_separator` and horizontal rules for enhanced legibility on dark terminal backgrounds ([#665](https://github.com/LHagfoss/rustcode/pull/665))
- **UI/Wrapping:** Ensure continuation line indentation (`    `) on wrapped tool calls, command outputs, and command summary trees ([#665](https://github.com/LHagfoss/rustcode/pull/665))
- **UI/Terminal:** Fix terminal resize reflow and screen clearing in `/clear` and `Ctrl+L` ([#664](https://github.com/LHagfoss/rustcode/pull/664))

## [v0.25.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.25.0) - 2026-08-17

### Features
- **Tools/Editing:** Implement Codex-style 4-tier fuzzy matching (`rstrip`, leading/trailing whitespace and indentation `trim`, Unicode smart quote / dash / space normalization, and multi-line block-anchor fallback) for `replace_file_content` and line-anchored edits ([#662](https://github.com/LHagfoss/rustcode/pull/662))
- **Tools/Filesystem:** Default `write_to_file` `overwrite` parameter to `true` when omitted to eliminate 1-turn file creation rejection loops ([#662](https://github.com/LHagfoss/rustcode/pull/662))
- **Tools/Exec:** Relax shell command blockers in `run_command` to allow `cat`, `head`, `tail`, `less`, `more`, and `sed` to execute cleanly as read-only utilities with bounded output ([#662](https://github.com/LHagfoss/rustcode/pull/662))
- **Harness/Diagnostics:** Scope post-edit compiler checks to only run when mutating files within an active Cargo/TS project root, eliminating false diagnostic loops on `/tmp` or sandbox scratch files ([#662](https://github.com/LHagfoss/rustcode/pull/662))

### Fixes
- **UI/Bash:** Fix bash highlight colors, divider padding/color, and working status styling in interactive terminal rendering ([#661](https://github.com/LHagfoss/rustcode/pull/661))

## [v0.24.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.24.0) - 2026-08-17

### Features
- **UI/Themes:** Add Cozy Rain warm dark theme (`#15171A` charcoal, `#EC6E5D` coral, `#3C5865` slate, `#F0E5DE` cream, and `#A6E3A1` green), embedded syntect grammar highlighter, and `rain` / `cozy-rain` presets matching dotfiles ([#659](https://github.com/LHagfoss/rustcode/pull/659))
- **UI/Modals:** Make all picker modals borderless and full-width, unify inner gutters, and fix `/history` row right-padding calculation ([#658](https://github.com/LHagfoss/rustcode/pull/658))

### Fixes
- **UI/Thinking:** Fix live reasoning thinking blocks using total request duration and token counts instead of thought-specific duration and reasoning tokens ([#657](https://github.com/LHagfoss/rustcode/pull/657))

## [v0.23.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.23.1) - 2026-08-17

### Fixes
- **CI/CD:** Fix YAML syntax error caused by unquoted colon in Windows environment setup step in `build.yml` ([#655](https://github.com/LHagfoss/rustcode/pull/655))

## [v0.23.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.23.0) - 2026-08-16

### Features
- **Context Management & Optimization:** Overhaul context management for long-running agent coding sessions. Distinguish theoretical context window ceilings from effective soft context targets and hard limits with safe profile-aware completion reserves ([#653](https://github.com/LHagfoss/rustcode/pull/653))
- **Continuation Reasoning Stripping:** Strip completed `<think>` scratchpads and bound unclosed reasoning traces from continuation assistant messages to eliminate geometric prompt amplification ([#653](https://github.com/LHagfoss/rustcode/pull/653))
- **Structured Session Memory:** Introduce structured Tier B session memory preserving user goals, explicit constraints, key architectural discoveries, modified files, decisions, and failure records across repeated compaction passes without loss ([#653](https://github.com/LHagfoss/rustcode/pull/653))
- **Category-Aware Continuations:** Tailor continuation nudges to interruption reasons (token limit cutoff, reasoning-only, unclosed tool syntax, or stated intent) ([#653](https://github.com/LHagfoss/rustcode/pull/653))
- **Preflight Budgeting & Telemetry:** Implement preflight budgeting and request composition diagnostics with provider token estimation calibration ([#653](https://github.com/LHagfoss/rustcode/pull/653))
- **CI/CD:** Optimize Windows CI runner build performance ([#651](https://github.com/LHagfoss/rustcode/pull/651))

### Fixes
- **UI/Terminal:** Prevent history turn duplication and scrollback corruption on terminal window resize ([#652](https://github.com/LHagfoss/rustcode/pull/652))

## [v0.22.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.22.0) - 2026-08-16

### Features
- **UI/Modals:** Unify all interactive picker modals (`/model`, `/thinking`, `/effort`, `/protocol`, `/verbosity`, `/theme`, and `/command`) to use rounded bordered containers, full-width bullet selection highlighting, and standardized keyboard footers matching the `/history` picker ([#649](https://github.com/LHagfoss/rustcode/pull/649))
- **UI/Reasoning:** Add interactive `/effort` picker modal with live selection of `Low`, `Medium`, `High`, and `Off` reasoning effort levels ([#649](https://github.com/LHagfoss/rustcode/pull/649))

## [v0.21.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.21.0) - 2026-08-16

### Features
- **Config/Reasoning:** Add `/effort` command (`low`, `medium`, `high`, `off`) and forward `reasoning_effort` in OpenAI/oMLX requests to control reasoning token depth without disabling thinking mode ([#647](https://github.com/LHagfoss/rustcode/pull/647) / [`55e7372`](https://github.com/LHagfoss/rustcode/commit/55e7372))
- **Sync:** Harden gitignore rules, autostash rebase recovery, dynamic branch targeting, and interactive `/sync` ([#645](https://github.com/LHagfoss/rustcode/pull/645) / [`d905950`](https://github.com/LHagfoss/rustcode/commit/d905950))

### Fixes
- **UI:** Align multiline and wrapped input buffer lines with 2-space indentation padding ([#646](https://github.com/LHagfoss/rustcode/pull/646) / [`b957839`](https://github.com/LHagfoss/rustcode/commit/b957839))

## [v0.20.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.20.1) - 2026-08-16

### Fixes
- **UI/Terminal:** Preserve shell scrollback history on window resize by removing full terminal scrollback purges from the inline runtime ([#643](https://github.com/LHagfoss/rustcode/pull/643) / [`61aef7f`](https://github.com/LHagfoss/rustcode/commit/61aef7f))
- **UI:** Format verbose loop detector recovery prompts into concise, human-friendly status lines while preserving complete prompt instructions for the model ([#643](https://github.com/LHagfoss/rustcode/pull/643) / [`61aef7f`](https://github.com/LHagfoss/rustcode/commit/61aef7f))

## [v0.20.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.20.0) - 2026-08-16

### Features
- **Network/Context:** Implement assistant reasoning retention policy, stripping raw historical `<think>` scratchpads from provider payloads to drastically reduce prompt pressure and prevent context bloat ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`9a09bca`](https://github.com/LHagfoss/rustcode/commit/9a09bca))
- **Network/Cache:** Keep historical provider messages immutable and attach request-local runtime context tails to maximize provider prompt-cache hit rates across turns ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`9a09bca`](https://github.com/LHagfoss/rustcode/commit/9a09bca))
- **Network/Compaction:** Make compaction cache-aware and reasoning-aware, reliably reclaiming tens of thousands of tokens from historical scratchpads before resorting to hard trimming ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`9a09bca`](https://github.com/LHagfoss/rustcode/commit/9a09bca))
- **Config/Providers:** Add explicit local classification for OpenAI-compatible local engines including oMLX, LM Studio, Ollama, and llama.cpp ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`9a09bca`](https://github.com/LHagfoss/rustcode/commit/9a09bca))
- **Telemetry:** Add metadata-only request lifecycle operational events tracking request start, response headers, and completion token metrics ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`9a09bca`](https://github.com/LHagfoss/rustcode/commit/9a09bca))

### Fixes
- **UI:** Eliminate duplicate chat inputs and ghost lines on viewport shrink after opening/closing modal pickers by unifying measuring passes into a single layout calculation ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`785586b`](https://github.com/LHagfoss/rustcode/commit/785586b))
- **UI/Terminal:** Fix buffer reference swap in `InlineTerminal` on odd frames and pre-clear mutable viewport before scrollback line insertion ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`785586b`](https://github.com/LHagfoss/rustcode/commit/785586b))
- **Network/Text:** Make thought-tag stripping code-fence aware to preserve code blocks containing literal `<think>` tags ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`c7aa89c`](https://github.com/LHagfoss/rustcode/commit/c7aa89c))
- **UI:** Report specific live activity status (`Thinking` during reasoning, `Working` during generation) ([#641](https://github.com/LHagfoss/rustcode/pull/641) / [`c7aa89c`](https://github.com/LHagfoss/rustcode/commit/c7aa89c))

## [v0.19.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.19.0) - 2026-08-15

### Features
- **Agent/UI:** Add the Codex-style interactive harness with typed event routing, coalesced redraws, separated transcript/composer/status surfaces, session controls, and navigable subagent contexts ([#637](https://github.com/LHagfoss/rustcode/pull/637) / [`f7a9c69`](https://github.com/LHagfoss/rustcode/commit/f7a9c69))

### Fixes
- **UI:** Make approval modal panel backgrounds follow the active state palette instead of shared global theme state ([`e585410`](https://github.com/LHagfoss/rustcode/commit/e585410))
- **Config/Tests:** Allocate monotonic session IDs, isolate test configuration by test thread, and make verbosity fixtures explicit for reliable parallel tests ([`d491236`](https://github.com/LHagfoss/rustcode/commit/d491236))

## [v0.18.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.18.2) - 2026-08-15

### Features
- **UI:** Route Ctrl-C through the active turn cancellation flow and move composer status into an external footer ([`cb51614`](https://github.com/LHagfoss/rustcode/commit/cb51614) / [`b70221e`](https://github.com/LHagfoss/rustcode/commit/b70221e))
- **UI:** Highlight shell commands in transcript output ([#621](https://github.com/LHagfoss/rustcode/pull/621) / [`848acbf`](https://github.com/LHagfoss/rustcode/commit/848acbf))

### Fixes
- **UI:** Align transcript and panel surfaces with Codex ([#617](https://github.com/LHagfoss/rustcode/pull/617) / [`248214b`](https://github.com/LHagfoss/rustcode/commit/248214b))
- **UI:** Keep the active model visible in the composer footer and pad worked-for separators ([#618](https://github.com/LHagfoss/rustcode/pull/618), [#619](https://github.com/LHagfoss/rustcode/pull/619) / [`d44d31f`](https://github.com/LHagfoss/rustcode/commit/d44d31f), [`0cb2d52`](https://github.com/LHagfoss/rustcode/commit/0cb2d52))
- **UI:** Expand the live assistant viewport and match the Codex shutdown handoff ([#626](https://github.com/LHagfoss/rustcode/pull/626), [#627](https://github.com/LHagfoss/rustcode/pull/627) / [`246ab8a`](https://github.com/LHagfoss/rustcode/commit/246ab8a), [`9586022`](https://github.com/LHagfoss/rustcode/commit/9586022))
- **UI:** Separate thought previews from tool cells and polish approval surfaces and separators ([#628](https://github.com/LHagfoss/rustcode/pull/628), [#629](https://github.com/LHagfoss/rustcode/pull/629) / [`60a9a23`](https://github.com/LHagfoss/rustcode/commit/60a9a23), [`83f995d`](https://github.com/LHagfoss/rustcode/commit/83f995d))
- **UI:** Bound the inline viewport at startup and resize it dynamically ([#630](https://github.com/LHagfoss/rustcode/pull/630), [#631](https://github.com/LHagfoss/rustcode/pull/631) / [`22580fa`](https://github.com/LHagfoss/rustcode/commit/22580fa), [`261c4ba`](https://github.com/LHagfoss/rustcode/commit/261c4ba))
- **UI:** Keep working status visible and stabilize inline notifications in the transcript ([#632](https://github.com/LHagfoss/rustcode/pull/632), [#633](https://github.com/LHagfoss/rustcode/pull/633), [#634](https://github.com/LHagfoss/rustcode/pull/634) / [`d8e70d7`](https://github.com/LHagfoss/rustcode/commit/d8e70d7), [`bf7e6a4`](https://github.com/LHagfoss/rustcode/commit/bf7e6a4), [`dff6bcf`](https://github.com/LHagfoss/rustcode/commit/dff6bcf))
- **Agent:** Resume after background task completion and coalesce duplicate wakeups ([#620](https://github.com/LHagfoss/rustcode/pull/620), [#622](https://github.com/LHagfoss/rustcode/pull/622) / [`e790852`](https://github.com/LHagfoss/rustcode/commit/e790852), [`84e0b5a`](https://github.com/LHagfoss/rustcode/commit/84e0b5a))
- **UI:** Finalize cancelled turns exactly once ([#635](https://github.com/LHagfoss/rustcode/pull/635) / [`4748a91`](https://github.com/LHagfoss/rustcode/commit/4748a91))

## [v0.18.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.18.1) - 2026-08-14
- **UI:** Resume after background task completion ([`e790852`](https://github.com/LHagfoss/rustcode/commit/e790852))
- **UI:** Pad worked-for separator and space composer footer with active model display ([`0cb2d52`](https://github.com/LHagfoss/rustcode/commit/0cb2d52), [`d44d31f`](https://github.com/LHagfoss/rustcode/commit/d44d31f))
- **UI:** Align transcript and panel surfaces with Codex design ([`248214b`](https://github.com/LHagfoss/rustcode/commit/248214b))
- **UI:** Make high verbosity tool output compact ([`ae8b934`](https://github.com/LHagfoss/rustcode/commit/ae8b934))
- **UI:** Restore chat surface backgrounds and help shortcut ([`e6cd99f`](https://github.com/LHagfoss/rustcode/commit/e6cd99f))
- **UI:** Restore panel backgrounds and approval selection ([`b972cc9`](https://github.com/LHagfoss/rustcode/commit/b972cc9))

## [v0.18.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.18.0) - 2026-08-14

### Features
- **UI:** Full redesign to Claude Code / Copilot CLI style with rounded boxed input and native terminal background ([#526](https://github.com/LHagfoss/rustcode/pull/526) / [`348c282`](https://github.com/LHagfoss/rustcode/commit/348c282))
- **UI:** Unified chat layout, remove welcome screen logic, and remove all remaining solid background fills ([#528](https://github.com/LHagfoss/rustcode/pull/528) / [`b36585f`](https://github.com/LHagfoss/rustcode/commit/b36585f))
- **UI:** Attach modal pickers inline to chat bar and remove solid panel background fills ([#527](https://github.com/LHagfoss/rustcode/pull/527) / [`8c8a188`](https://github.com/LHagfoss/rustcode/commit/8c8a188))
- **UI:** Add startup banner with full-width RustCode ASCII logo ([#530](https://github.com/LHagfoss/rustcode/pull/530) / [`eff7f4f`](https://github.com/LHagfoss/rustcode/commit/eff7f4f))
- **UI:** Format system info/help/status command output inside rounded card boxes with >_ RustCode header ([#534](https://github.com/LHagfoss/rustcode/pull/534) / [`76a6e2b`](https://github.com/LHagfoss/rustcode/commit/76a6e2b))
- **UI:** Modernize user prompt rendering with sleek ❯ prompt glyph ([#535](https://github.com/LHagfoss/rustcode/pull/535) / [`48b3da2`](https://github.com/LHagfoss/rustcode/commit/48b3da2))
- **UI:** Switch to full native inline mode with terminal scrollback, removing alternate screen buffer and mouse capture ([#536](https://github.com/LHagfoss/rustcode/pull/536) / [`8a5939f`](https://github.com/LHagfoss/rustcode/commit/8a5939f))
- **UI:** Clear terminal screen and move cursor to top-left on startup ([#537](https://github.com/LHagfoss/rustcode/pull/537) / [`7401e99`](https://github.com/LHagfoss/rustcode/commit/7401e99))
- **UI:** Set card borders to signature primary orange accent color ([#540](https://github.com/LHagfoss/rustcode/pull/540) / [`ba12b9b`](https://github.com/LHagfoss/rustcode/commit/ba12b9b))
- **UI:** Render animated shimmer Working indicator inside chat stream ([#579](https://github.com/LHagfoss/rustcode/pull/579) / [`03d7d7b`](https://github.com/LHagfoss/rustcode/commit/03d7d7b))
- **UI:** Compact queued prompts and improve chat transcript hierarchy ([#578](https://github.com/LHagfoss/rustcode/pull/578) / [`310eb16`](https://github.com/LHagfoss/rustcode/commit/310eb16))
- **UI:** Display UseSkill and agent tool calls in transcript ([#576](https://github.com/LHagfoss/rustcode/pull/576) / [`e2d8d20`](https://github.com/LHagfoss/rustcode/commit/e2d8d20))
- **UI:** Adopt Codex-style transcript rendering and conversation experience ([#575](https://github.com/LHagfoss/rustcode/pull/575) / [`2da6a2d`](https://github.com/LHagfoss/rustcode/commit/2da6a2d))
- **UI:** Improve streamed markdown and composer suggestions ([#608](https://github.com/LHagfoss/rustcode/pull/608) / [`2b9e322`](https://github.com/LHagfoss/rustcode/commit/2b9e322))
- **UI:** Align streaming transcript with Codex ([#611](https://github.com/LHagfoss/rustcode/pull/611) / [`dcc8eb2`](https://github.com/LHagfoss/rustcode/commit/dcc8eb2))
- **UI:** Match Codex conversation experience ([#612](https://github.com/LHagfoss/rustcode/pull/612) / [`5addb11`](https://github.com/LHagfoss/rustcode/commit/5addb11))
- **Agent:** Improve coding loop context tools and memory ([#605](https://github.com/LHagfoss/rustcode/pull/605) / [`c61255e`](https://github.com/LHagfoss/rustcode/commit/c61255e))
- **Activity:** Add expressive activity status words and terminal title display ([#546](https://github.com/LHagfoss/rustcode/pull/546) / [`d474dec`](https://github.com/LHagfoss/rustcode/commit/d474dec))
- **Activity:** Compact input status bar with activity footer ([#547](https://github.com/LHagfoss/rustcode/pull/547) / [`3e10a13`](https://github.com/LHagfoss/rustcode/commit/3e10a13))
- **Terminal:** Commit chat history to native terminal scrollback ([#573](https://github.com/LHagfoss/rustcode/pull/573) / [`7db5c08`](https://github.com/LHagfoss/rustcode/commit/7db5c08))
- **Footer:** Add depth to activity trail ([#545](https://github.com/LHagfoss/rustcode/pull/545) / [`153d819`](https://github.com/LHagfoss/rustcode/commit/153d819))
- **Config:** Add z.ai and bigmodel.cn function calling hosts ([#488](https://github.com/LHagfoss/rustcode/pull/488) / [`dc815b8`](https://github.com/LHagfoss/rustcode/commit/dc815b8))

### Fixes
- **UI:** Restore chat surface backgrounds and help shortcut ([#614](https://github.com/LHagfoss/rustcode/pull/614) / [`e6cd99f`](https://github.com/LHagfoss/rustcode/commit/e6cd99f))
- **UI:** Restore panel backgrounds and approval selection ([#613](https://github.com/LHagfoss/rustcode/pull/613) / [`b972cc9`](https://github.com/LHagfoss/rustcode/commit/b972cc9))
- **UI:** Make high verbosity tool output compact ([#615](https://github.com/LHagfoss/rustcode/pull/615) / [`ae8b934`](https://github.com/LHagfoss/rustcode/commit/ae8b934))
- **UI:** Replay inline transcript after terminal resize ([#601](https://github.com/LHagfoss/rustcode/pull/601) / [`212b88d`](https://github.com/LHagfoss/rustcode/commit/212b88d))
- **UI:** Align parallel approvals and follow-up spacing ([#599](https://github.com/LHagfoss/rustcode/pull/599) / [`ae01d86`](https://github.com/LHagfoss/rustcode/commit/ae01d86))
- **UI:** Show tool summaries at high verbosity ([#598](https://github.com/LHagfoss/rustcode/pull/598) / [`1560064`](https://github.com/LHagfoss/rustcode/commit/1560064))
- **UI:** Polish markdown headings and table separators ([#597](https://github.com/LHagfoss/rustcode/pull/597) / [`44c69da`](https://github.com/LHagfoss/rustcode/commit/44c69da))
- **UI:** Use full viewport height when pickers or popups are active ([#596](https://github.com/LHagfoss/rustcode/pull/596) / [`486389c`](https://github.com/LHagfoss/rustcode/commit/486389c))
- **UI:** Render stream chunks without trailing blank lines ([#595](https://github.com/LHagfoss/rustcode/pull/595) / [`2df942e`](https://github.com/LHagfoss/rustcode/commit/2df942e))
- **UI:** Collapse blank lines between list items and avoid duplicate empty lines ([#594](https://github.com/LHagfoss/rustcode/pull/594) / [`216ea61`](https://github.com/LHagfoss/rustcode/commit/216ea61))
- **UI:** Sanitize loose bullet lists and normalize bullet characters ([#593](https://github.com/LHagfoss/rustcode/pull/593) / [`f8c3832`](https://github.com/LHagfoss/rustcode/commit/f8c3832))
- **UI:** Sanitize loose markdown tables and position composer compactly ([#592](https://github.com/LHagfoss/rustcode/pull/592) / [`dc530a0`](https://github.com/LHagfoss/rustcode/commit/dc530a0))
- **UI:** Tighten list item gaps, add gap below Working, and pin chat bar to bottom ([#591](https://github.com/LHagfoss/rustcode/pull/591) / [`0de42b4`](https://github.com/LHagfoss/rustcode/commit/0de42b4))
- **UI:** Tighten markdown list spacing, pad user message, and place composer compactly ([#590](https://github.com/LHagfoss/rustcode/pull/590) / [`b3430ee`](https://github.com/LHagfoss/rustcode/commit/b3430ee))
- **UI:** Remove bullet prefix from assistant text and omit streaming blank line ([#589](https://github.com/LHagfoss/rustcode/pull/589) / [`617c3fc`](https://github.com/LHagfoss/rustcode/commit/617c3fc))
- **UI:** Tighten markdown table spacing ([#588](https://github.com/LHagfoss/rustcode/pull/588) / [`411365d`](https://github.com/LHagfoss/rustcode/commit/411365d))
- **UI:** Replace resumed session toast with full-width centered rule ([#584](https://github.com/LHagfoss/rustcode/pull/584) / [`358d03c`](https://github.com/LHagfoss/rustcode/commit/358d03c))
- **UI:** Request redraw after clearing working_status_pending ([#583](https://github.com/LHagfoss/rustcode/pull/583) / [`fc60bca`](https://github.com/LHagfoss/rustcode/commit/fc60bca))
- **UI:** Remove extra blank lines under working status and over composer ([#582](https://github.com/LHagfoss/rustcode/pull/582) / [`220f61d`](https://github.com/LHagfoss/rustcode/commit/220f61d))
- **UI:** Restore single-line bulleted tool call items ([#581](https://github.com/LHagfoss/rustcode/pull/581) / [`3d6a1a5`](https://github.com/LHagfoss/rustcode/commit/3d6a1a5))
- **UI:** Space consecutive thinking blocks, add working padding, and pin chat composer ([#580](https://github.com/LHagfoss/rustcode/pull/580) / [`1089c53`](https://github.com/LHagfoss/rustcode/commit/1089c53))
- **UI:** Keep working through final frame ([#579](https://github.com/LHagfoss/rustcode/pull/579) / [`10e8085`](https://github.com/LHagfoss/rustcode/commit/10e8085))
- **UI:** Cover compact modal boundary ([#578](https://github.com/LHagfoss/rustcode/pull/578) / [`d763994`](https://github.com/LHagfoss/rustcode/commit/d763994))
- **UI:** Preserve compact modal scope ([#578](https://github.com/LHagfoss/rustcode/pull/578) / [`f12b0e6`](https://github.com/LHagfoss/rustcode/commit/f12b0e6))
- **UI:** Keep short modal actions visible ([#578](https://github.com/LHagfoss/rustcode/pull/578) / [`55180aa`](https://github.com/LHagfoss/rustcode/commit/55180aa))
- **UI:** Guard short confirmation modals ([#578](https://github.com/LHagfoss/rustcode/pull/578) / [`b1df637`](https://github.com/LHagfoss/rustcode/commit/b1df637))
- **UI:** Use compact welcome box and clean thought block preambles ([#577](https://github.com/LHagfoss/rustcode/pull/577) / [`764fe6b`](https://github.com/LHagfoss/rustcode/commit/764fe6b))
- **UI:** Keep bare thoughts compact ([#577](https://github.com/LHagfoss/rustcode/pull/577) / [`d4edb7a`](https://github.com/LHagfoss/rustcode/commit/d4edb7a))
- **UI:** Restore progressive streaming ([#576](https://github.com/LHagfoss/rustcode/pull/576) / [`928c351`](https://github.com/LHagfoss/rustcode/commit/928c351))
- **UI:** Tighten transcript thought spacing ([#575](https://github.com/LHagfoss/rustcode/pull/575) / [`41afe41`](https://github.com/LHagfoss/rustcode/commit/41afe41))
- **UI:** Restore welcome and thought flow ([#575](https://github.com/LHagfoss/rustcode/pull/575) / [`bb235d5`](https://github.com/LHagfoss/rustcode/commit/bb235d5))
- **UI:** Align wrapped markdown blocks ([#575](https://github.com/LHagfoss/rustcode/pull/575) / [`bc8992a`](https://github.com/LHagfoss/rustcode/commit/bc8992a))
- **UI:** Adapt narrow markdown tables ([#575](https://github.com/LHagfoss/rustcode/pull/575) / [`f8c934a`](https://github.com/LHagfoss/rustcode/commit/f8c934a))
- **UI:** Clarify tool results ([#574](https://github.com/LHagfoss/rustcode/pull/574) / [`30bc1a2`](https://github.com/LHagfoss/rustcode/commit/30bc1a2))
- **UI:** Improve chat transcript hierarchy ([#574](https://github.com/LHagfoss/rustcode/pull/574) / [`354a92d`](https://github.com/LHagfoss/rustcode/commit/354a92d))
- **App/UI:** Clear session history on /new and handle missing open think tags ([#568](https://github.com/LHagfoss/rustcode/pull/568) / [`b8e819e`](https://github.com/LHagfoss/rustcode/commit/b8e819e))
- **UI/Tools:** Hide oversized response notices and support multi-tool skills ([#567](https://github.com/LHagfoss/rustcode/pull/567) / [`28c3d66`](https://github.com/LHagfoss/rustcode/commit/28c3d66))
- **UI:** Expand inline picker choices ([#566](https://github.com/LHagfoss/rustcode/pull/566) / [`923dc1b`](https://github.com/LHagfoss/rustcode/commit/923dc1b))
- **UI:** Retain incomplete stream in live tail ([#565](https://github.com/LHagfoss/rustcode/pull/565) / [`e2a4f80`](https://github.com/LHagfoss/rustcode/commit/e2a4f80))
- **UI:** Compact short chat layout ([#564](https://github.com/LHagfoss/rustcode/pull/564) / [`cf67a30`](https://github.com/LHagfoss/rustcode/commit/cf67a30))
- **UI:** Tighten tool confirmation layout ([#563](https://github.com/LHagfoss/rustcode/pull/563) / [`ada224b`](https://github.com/LHagfoss/rustcode/commit/ada224b))
- **Images:** Preserve resume analysis ([#562](https://github.com/LHagfoss/rustcode/pull/562) / [`c8492ce`](https://github.com/LHagfoss/rustcode/commit/c8492ce))
- **UI:** Restore inline tui chat ([#561](https://github.com/LHagfoss/rustcode/pull/561) / [`c8492ce`](https://github.com/LHagfoss/rustcode/commit/c8492ce))
- **Terminal:** Remove raw mode calls ([#560](https://github.com/LHagfoss/rustcode/pull/560) / [`ee48ea4`](https://github.com/LHagfoss/rustcode/commit/ee48ea4))
- **Terminal:** Use native interactive scrollback ([#560](https://github.com/LHagfoss/rustcode/pull/560) / [`ca072cd`](https://github.com/LHagfoss/rustcode/commit/ca072cd))
- **Terminal:** Render tui inline with terminal output ([#559](https://github.com/LHagfoss/rustcode/pull/559) / [`d29453d`](https://github.com/LHagfoss/rustcode/commit/d29453d))
- **Terminal:** Keep cargo output outside tui screen ([#559](https://github.com/LHagfoss/rustcode/pull/559) / [`679fbce`](https://github.com/LHagfoss/rustcode/commit/679fbce))
- **UI:** Show full-width new chat boundary ([#558](https://github.com/LHagfoss/rustcode/pull/558) / [`c32399e`](https://github.com/LHagfoss/rustcode/commit/c32399e))
- **UI:** Preserve complete tool output ([#557](https://github.com/LHagfoss/rustcode/pull/557) / [`e19725c`](https://github.com/LHagfoss/rustcode/commit/e19725c))
- **UI:** Align confirmations and mark new chats ([#557](https://github.com/LHagfoss/rustcode/pull/557) / [`ba34e8b`](https://github.com/LHagfoss/rustcode/commit/ba34e8b))
- **Security:** Gate unknown shell commands ([#556](https://github.com/LHagfoss/rustcode/pull/556) / [`c460b4a`](https://github.com/LHagfoss/rustcode/commit/c460b4a))
- **UI:** Preserve transcript history ([#555](https://github.com/LHagfoss/rustcode/pull/555) / [`44201df`](https://github.com/LHagfoss/rustcode/commit/44201df))
- **UI:** Tighten thought timing from answer timing ([#554](https://github.com/LHagfoss/rustcode/pull/554) / [`54d81a9`](https://github.com/LHagfoss/rustcode/commit/54d81a9))
- **Agent:** Harden agent context and tool state review ([#606](https://github.com/LHagfoss/rustcode/pull/606) / [`2c6aefd`](https://github.com/LHagfoss/rustcode/commit/2c6aefd))
- **Network:** Support thought and thinking delta keys for reasoning extraction ([#604](https://github.com/LHagfoss/rustcode/pull/604) / [`1f57285`](https://github.com/LHagfoss/rustcode/commit/1f57285))
- **Hardening:** Harden live tool identity and streamed fences ([#607](https://github.com/LHagfoss/rustcode/pull/607) / [`515faea`](https://github.com/LHagfoss/rustcode/commit/515faea))
- **Network:** Bound silent provider streams ([#517](https://github.com/LHagfoss/rustcode/pull/517) / [`ad91cf1`](https://github.com/LHagfoss/rustcode/commit/ad91cf1))
- **Tools:** Bound malformed call recovery ([#516](https://github.com/LHagfoss/rustcode/pull/516) / [`ea3b64a`](https://github.com/LHagfoss/rustcode/commit/ea3b64a))
- **MCP:** Bound enabled server startup ([#515](https://github.com/LHagfoss/rustcode/pull/515) / [`98b5cde`](https://github.com/LHagfoss/rustcode/commit/98b5cde))
- **ACP:** Run prompt turns outside event loop ([#514](https://github.com/LHagfoss/rustcode/pull/514) / [`e87fa0e`](https://github.com/LHagfoss/rustcode/commit/e87fa0e))
- **Agent:** Escalate repeated failed mutations ([#506](https://github.com/LHagfoss/rustcode/pull/506) / [`64d2824`](https://github.com/LHagfoss/rustcode/commit/64d2824))
- **Security:** Confirm destructive git commands ([#505](https://github.com/LHagfoss/rustcode/pull/505) / [`f366a52`](https://github.com/LHagfoss/rustcode/commit/f366a52))
- **Context:** Surface stale files and compiler snippets ([#504](https://github.com/LHagfoss/rustcode/pull/504) / [`f6a86e1`](https://github.com/LHagfoss/rustcode/commit/f6a86e1))
- **Agent:** Stop repeated compiler diagnostics ([#503](https://github.com/LHagfoss/rustcode/pull/503) / [`09982d0`](https://github.com/LHagfoss/rustcode/commit/09982d0))
- **Tools:** Improve malformed call recovery ([#502](https://github.com/LHagfoss/rustcode/pull/502) / [`ca179ab`](https://github.com/LHagfoss/rustcode/commit/ca179ab))
- **Agent:** Restore tool round backstop ([#501](https://github.com/LHagfoss/rustcode/pull/501) / [`67c8e9e`](https://github.com/LHagfoss/rustcode/commit/67c8e9e))
- **UI:** Attach per-turn thought duration/tokens and eliminate line gaps ([#489](https://github.com/LHagfoss/rustcode/pull/489) / [`410d8b5`](https://github.com/LHagfoss/rustcode/commit/410d8b5))

### Refactor
- **Network:** Extract turn engine into src/network/turn_engine.rs ([#517](https://github.com/LHagfoss/rustcode/pull/517) / [`8c1be43`](https://github.com/LHagfoss/rustcode/commit/8c1be43))
- **Network:** Extract tool execution into src/network/tool_exec.rs ([#516](https://github.com/LHagfoss/rustcode/pull/516) / [`b33e4b5`](https://github.com/LHagfoss/rustcode/commit/b33e4b5))
- **Network:** Extract stream_request into src/network/stream_request.rs ([#515](https://github.com/LHagfoss/rustcode/pull/515) / [`d4764a7`](https://github.com/LHagfoss/rustcode/commit/d4764a7))
- **Network:** Extract title generation and context tail building ([#514](https://github.com/LHagfoss/rustcode/pull/514) / [`5eaf708`](https://github.com/LHagfoss/rustcode/commit/5eaf708))
- **Network:** Extract subagent execution and tool handling to src/network/subagents.rs ([#513](https://github.com/LHagfoss/rustcode/pull/513) / [`2105b80`](https://github.com/LHagfoss/rustcode/commit/2105b80))
- **Network:** Extract compiler check & diagnostic logic into src/network/compiler.rs ([#512](https://github.com/LHagfoss/rustcode/pull/512) / [`214823a`](https://github.com/LHagfoss/rustcode/commit/214823a))
- **Network:** Extract model quota and multimodal payload logic to src/network/payload.rs ([#511](https://github.com/LHagfoss/rustcode/pull/511) / [`d5ea72f`](https://github.com/LHagfoss/rustcode/commit/d5ea72f))
- **Tests:** Extract UI tests into src/ui/tests.rs ([#510](https://github.com/LHagfoss/rustcode/pull/510) / [`71a9b09`](https://github.com/LHagfoss/rustcode/commit/71a9b09))
- **Tests:** Extract filesystem tool tests into src/tools/filesystem/tests.rs ([#509](https://github.com/LHagfoss/rustcode/pull/509) / [`7040da0`](https://github.com/LHagfoss/rustcode/commit/7040da0))
- **Tests:** Extract network tests into src/network/tests.rs ([#508](https://github.com/LHagfoss/rustcode/pull/508) / [`0614e53`](https://github.com/LHagfoss/rustcode/commit/0614e53))
- **Refactor:** Centralize turn lifecycle ([#507](https://github.com/LHagfoss/rustcode/pull/507) / [`40a0e68`](https://github.com/LHagfoss/rustcode/commit/40a0e68))
- **Refactor:** Isolate native tool responses ([#506](https://github.com/LHagfoss/rustcode/pull/506) / [`2b5e472`](https://github.com/LHagfoss/rustcode/commit/2b5e472))
- **UI:** Use native terminal scrollback ([#566](https://github.com/LHagfoss/rustcode/pull/566) / [`d89d8c0`](https://github.com/LHagfoss/rustcode/commit/d89d8c0))

### Chores
- Remove optional docs benchmarks and tests ([#602](https://github.com/LHagfoss/rustcode/pull/602) / [`659aab4`](https://github.com/LHagfoss/rustcode/commit/659aab4))
- Resolve compiler and dead code warnings ([#507](https://github.com/LHagfoss/rustcode/pull/507) / [`0be1a7c`](https://github.com/LHagfoss/rustcode/commit/0be1a7c))

## [v0.17.0](https://github.com/LHagfoss/rustcode/releases/tag/v0.17.0) - 2026-08-11

### Features
- **UI:** Format thinking blocks with duration, token count, and summary preview ([#486](https://github.com/LHagfoss/rustcode/pull/486) / [`7232af5`](https://github.com/LHagfoss/rustcode/commit/7232af5))
- **Vision:** Add image vision fallback and preserve pasted image chips ([#482](https://github.com/LHagfoss/rustcode/pull/482) / [`3c9da74`](https://github.com/LHagfoss/rustcode/commit/3c9da74))

### Fixes
- **UI:** Eliminate duplicate in-flight tool call lines and fix double line gaps ([#485](https://github.com/LHagfoss/rustcode/pull/485) / [`2885fe4`](https://github.com/LHagfoss/rustcode/commit/2885fe4))
- **UI:** Hide harness recovery notices from transcript UI ([#484](https://github.com/LHagfoss/rustcode/pull/484) / [`996828e`](https://github.com/LHagfoss/rustcode/commit/996828e))

## [v0.16.2](https://github.com/LHagfoss/rustcode/releases/tag/v0.16.2) - 2026-08-11

### Fixes
- **UI:** Keep tool pills visible at high verbosity ([#478](https://github.com/LHagfoss/rustcode/pull/478) / [`a8582ac`](https://github.com/LHagfoss/rustcode/commit/a8582ac))
- **UI:** Tighten transcript spacing ([#480](https://github.com/LHagfoss/rustcode/pull/480) / [`523695c`](https://github.com/LHagfoss/rustcode/commit/523695c))

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

## [v0.1.12](https://github.com/LHagfoss/rustcode/releases/tag/v0.1.12) - 2026-07-16
- Initial release
## [v0.50.1](https://github.com/LHagfoss/rustcode/releases/tag/v0.50.1) - 2026-09-02

### Fixes
- **Headless read-only completion:** Mark complete inspection reports as successful in headless mode, preserving genuine incomplete and recovery failures ([#934](https://github.com/LHagfoss/rustcode/issues/934); [#935](https://github.com/LHagfoss/rustcode/pull/935)).
