# GitHub Copilot

Connect RustCode to GitHub Copilot with `/login github-copilot` and select a
catalog model with `/model`. Profiles are normal `[[models]]` entries bound to
the saved Copilot account; the existing picker consumes `config.models`
unchanged.

## Prerequisites

Two sign-in paths exist. RustCode ships no OAuth client ID and never borrows
another application's ID.

- **GitHub CLI (default):** install `gh` and sign in first with
  `gh auth login`. `/login github-copilot` reads only `gh auth token`
  output and copies that credential into RustCode's scoped native storage.
  If no CLI credential exists, RustCode starts `gh auth login --web`
  and shows only the validated device code and fixed
  `https://github.com/login/device` URL. Never paste the token itself.
- **Own OAuth app (optional):** register your own GitHub OAuth app, then set
  `RUSTCODE_COPILOT_CLIENT_ID` to its public client ID before sign-in.
  RustCode runs the standard device authorization flow against
  `https://github.com/login/device/code` and polls with backoff for
  `authorization_pending` / `slow_down`. No client secret is used or stored.

Without `gh` and without `RUSTCODE_COPILOT_CLIENT_ID`, sign-in fails with
installation instructions instead of hanging.

## Commands

- `/login github-copilot` — reuse the saved account catalog, or sign in when
  none is saved.
- `/login github-copilot new` — sign in a second account.
- `/login github-copilot [new] --headless` — show the device page and code
  without opening a browser; `--browser` opens it and fails if it cannot.
  With neither, the browser is opened unless the session is remote (SSH) or
  has no display.
- `/login github-copilot <account-id>` — refresh that saved account's model
  catalog (account IDs come from `/auth status`).
- `/refresh github-copilot [account-id]` (or `/account refresh ...`) —
  re-fetch the authenticated `/models` catalog without browser sign-in.
- `/auth status`, `/accounts` — list Copilot accounts, endpoints, and expiry.
- `/logout github-copilot [account-id]` — remove the saved credential and
  cancel any pending device flow.
- `/model` — pick a `copilot/<id>` profile after sign-in.
- Press `Esc`, `/cancel`, or `/new` during device authorization to cancel.

## What sign-in creates

A successful sign-in verifies the credential against
`https://api.github.com/user`, fetches `GET {endpoint}/models`, and installs
one profile per usable model:

```toml
[[models]]
name = "copilot/gpt-5-mini"
url = "https://api.githubcopilot.com/chat/completions"
model = "gpt-5-mini"
api_protocol = "chat_completions"

[models.credential]
provider = "github-copilot"
account = "github-42"
method = "github_copilot"
```

The credential itself stays in the OS credential store under the
`copilot-token` kind, bound to its endpoint. Rewritten endpoint metadata or a
profile pointed at an unsupported path fails before any secret is returned.
Existing unrelated `[[models]]` entries are preserved; refreshing one Copilot
account only replaces profiles bound to that account.

## Model selection rules

The catalog drives names, context limits, vision/tool/reasoning support, and
the wire protocol (`/v1/messages`, `/responses`, or `/chat/completions`).
A model is listed only when it is policy-enabled, advertises tool calls and
streaming, exposes `max_prompt_tokens` / `max_output_tokens`, and maps to a
supported endpoint. `model_picker_enabled` is ignored: the authenticated
catalog sets it false on records that are otherwise usable, so policy,
capabilities, and protocol are authoritative. Records without
`supported_endpoints` fall back to `/chat/completions` only when they are
explicitly `type: chat` with valid streaming, tool-call, and limit
advertisements; embeddings and completion-only records stay excluded.

Vision support follows the catalog's vision flags and image media types.
Reasoning-effort controls are advertised only for Chat/Responses models that
list effort levels. Explicit extended/adaptive thinking controls are not
advertised on Messages models in this release: the canonical history does not
retain signed thinking blocks yet, and enabling unsupported thinking could
invalidate subsequent tool history. Ordinary tool, text, image, usage, and
cancellation behavior works on Messages models.

## Requests and errors

Requests carry the Copilot API version, conversation intent, agent/user
initiator, vision marker for image payloads, and `anthropic-version:
2023-06-01` for Messages models. Access-denied, expired, missing-entitlement,
and unknown-model responses fail with the action to take (`gh auth login`,
reconnect, or `/refresh github-copilot`) and never print the credential.
Expiring OAuth credentials refresh automatically; non-refreshable CLI
credentials report expiry and ask for `gh auth login` plus
`/login github-copilot new`. `/usage` notes that Copilot subscription usage
and quota are managed by GitHub; local token totals do not measure remaining
Copilot quota.
