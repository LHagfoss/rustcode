# 0006: CLI integration tests never touch the developer's setup

Status: accepted, 2026-11-21. Code: `rustcode/cli/tests/`,
`rustcode/cli/src/test_isolation.rs`.

- Every run gets its own config dir, home dir and workspace. The sandbox
  root is also the workspace, so project config discovery and
  workspace-scoped listings stay off the real repository.
- `RUSTCODE_MACHINE_TOKEN_STORE=file`: tokens go to a private file, never the
  developer's keychain.
- `RUSTCODE_SHELL_PROBE=1` marks the process as a shell-probe child, so
  startup does not spawn the developer's login shell or parse their
  dotfiles.
- An unreachable machine is `127.0.0.1:1`: nothing listens on port 1, so the
  connection is refused at once instead of hanging.
