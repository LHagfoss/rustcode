# AGENTS.md (ios/)

The native SwiftUI iOS client for RustCode. The repository root `AGENTS.md`
governs workflow, naming and releases; these notes add what is specific to the
app. Cargo commands do not cover it: run the checks below for changes here.
The app ships in every RustCode release under the RustCode version, set by
`scripts/release.sh`; do not bump its version or tag it separately. Use the
`ios` scope for app changes.

## Search and architecture

- Start with `rg` or `rg --files` for exact strings and files. Use available
  semantic or structural search when the location or syntax is unclear, and
  verify the source before acting.
- Search for existing behavior before adding helpers or dependencies. Prefer
  project code, then platform APIs, then a small local implementation.
- App views use `RemoteStore`; the store uses `RemoteClient`. Keep transport and
  protocol DTOs behind the client/codec boundary. Put presentation parsing in
  `RustCodeRemote/Presentation` and native rendering in `RustCode/Views`.
- Preserve raw transcript content. Keep streaming presentation safe for partial
  Markdown and thinking delimiters without changing literal code examples.
- Check the actual host contract before adding controls or metadata. Do not
  invent model choices, timing, completion timestamps or operations the host
  does not expose. Keep unknown values and absent optional fields compatible.
- The contract tests read `docs/remote-protocol/v1` in place. A host protocol
  change and the matching app change belong in the same PR.

## Verification

- For package behavior changes, run `swift test --package-path RustCodeRemote`.
  Add meaningful regressions for bugs in streaming, mapping or state handling.
- For app changes, build the shared scheme:
  `xcodebuild -project RustCode.xcodeproj -scheme RustCode -destination 'generic/platform=iOS Simulator' build CODE_SIGNING_ALLOWED=NO`.
- For touch, focus, keyboard or transcript interaction changes, run the relevant
  `RustCodeUITests` on an installed iPhone Simulator. Discover destinations with
  `xcodebuild -showdestinations -project RustCode.xcodeproj -scheme RustCode`.
  Example:
  `xcodebuild test -project RustCode.xcodeproj -scheme RustCode -destination 'platform=iOS Simulator,name=iPhone 17 Pro'`.
- Visually check layout changes, including expanded thinking and the keyboard.
  A build passing does not establish correct touch handling or real-device UX.
- Real-gateway tests are opt-in using `RUSTCODE_GATEWAY_BIN=/path/to/rustcode`.
  Report when they were skipped; distinguish demo, Simulator, real host and
  physical phone evidence. Do not rerun unrelated suites for documentation edits.
- Keep signing settings in ignored `Config/Local.xcconfig`. Preserve it and
  never commit personal team IDs, credentials or pairing tokens.
- When the user asks to run on their phone, use the available paired device and
  existing signing configuration. Verify build, installation and launch
  separately; report any blocked step precisely. Preserve paired app data.
- Before publishing, run `git diff --check`, inspect the final diff and report
  actual checks and any remaining limits. Do not claim checks that did not run.
