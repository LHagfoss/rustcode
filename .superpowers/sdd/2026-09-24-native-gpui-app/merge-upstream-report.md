# Upstream integration report

Date: 2026-09-24
Worktree: `/tmp/rustcode-gpui-integration-research`
Branch: `feature/gpui-integration-research`

## Commits

- Feature tip before integration: `86c5ffd7ec70eb60b98965044e00a1c17fa0f5a8`
- Upstream `origin/main`: `9ad11607ee4e25b50ef3f35e0a49feac77e2e7fe`
- Merge base: `a8bcf900e67ddef6cdd61c431c9c03f556c569c4`
- Merge commit: `08ab28d` (`Merge remote-tracking branch 'origin/main' into feature/gpui-integration-research`)
- Integration fix commit: `2e2eaf721155b9a32d2940360a274c5eff56d89f`

The merge was performed only in this isolated worktree. `git merge --no-edit origin/main` completed without textual conflicts. No upstream or feature changes were discarded.

## Overlap review and resolutions

Git auto-merged the shared paths. I reviewed the combined changes in `README.md`, `src/app/actions/enter.rs`, `src/app/actions/tests.rs`, `src/app/runtime/orchestration.rs`, `src/config/session.rs`, `src/network.rs`, and `src/network/tests.rs`.

- `README.md`: retained the feature's native-app usage/build documentation together with upstream's shell approval and OS sandbox documentation.
- `src/app/actions/enter.rs`: retained upstream's `/sandbox` command alongside the feature's native session entry behavior.
- `src/app/actions/tests.rs`: retained both the native action tests and the upstream sandbox-mode test.
- `src/app/runtime/orchestration.rs`: retained upstream's two new `CommandRequest` fields in the existing background-completion fixture.
- `src/config/session.rs`: retained the feature's session workspace support and upstream's additional session listing/loading/metadata APIs.
- `src/network.rs` and `src/network/tests.rs`: retained upstream's full-history-until-soft-budget projection and tests alongside feature changes for loaded ACP transcript continuation and background completion routing.
- `Cargo.lock`: no upstream-side change existed after the merge base; the feature's GPUI dependency lockfile changes remained intact.
- `src/controller/tests.rs`: after the merge, `cargo check --tests` exposed one feature-side `CommandRequest` fixture that had not gained upstream's new `status_command` and `sandboxed_shell` fields. Added the same safe defaults already used by the orchestration fixture (`None` and `false`) in commit `2e2eaf7`.

No architectural ambiguity arose. `git diff --check` passed after the resolution.

## Verification

- Initial `cargo check --tests`: failed because the controller fixture omitted upstream's new request fields. After the fixture update: passed (`Finished dev profile`).
- `cargo test -- --test-threads=1`: passed, 1,734 passed, 0 failed, 0 ignored. Serial execution was used to avoid the known parallel daemon-test race.
- `cargo build --bin rustcode`: passed.
- `cargo build -p rustcode-app`: passed. Cargo emitted a future-incompatibility warning for dependency `block v0.1.6`; build succeeded.
