# Permissions

A thread runs at one access level, and a few other layers can still ask or
refuse. This page lists every layer, what Full access covers, and where each
one is changed. A refusal names the layer that refused and the one place that
allows it (`rustcode/engine/src/permission_places.rs` keeps those names).

## What Full access covers

Full access covers everything RustCode itself would otherwise ask about for
that thread: every tool call of a harness that asks (`session/request_permission`),
orchestration on the user's own threads and projects (start, message, fan out,
merge, clean up, hand off), and the management changes its work needs. It is
set per thread in the **Permissions** menu under the composer, and for new
threads in Settings › Permissions › Default access.

## What still asks at Full access

Each of these has one reason, and Settings › Permissions › "Still asks at Full
access" lists the same three:

| What | Why | Where it is approved |
| --- | --- | --- |
| Questions and plan reviews | Only the user can answer them. | The card in the thread, or "Needs you" in the sidebar. |
| A delegate on a named account | Starting a thread on a signed-in account spends that account's usage, and a thread's access does not cover spending another login. A delegate on the caller's own named login (the same harness with no `account`, or the same one named) is not asked. | The card, once per thread (kept in Settings › Permissions › Remembered approvals), or for all delegates with Settings › Delegation › "Let delegates use my named accounts without asking". That switch covers named accounts; the default sign-in of a harness with named accounts is approved per thread. |
| Outside agents | An app connected under Settings › Outside agents is not one of the user's threads. | Its card in the app, or `rc outside update <name> --ask false`. |

When the engine answers a call itself because no RustCode window hosts the
thread's tools (`docs/orchestrator.md` › *No app attached*), the account
question is still asked with the thread's own approval card when the thread is
open in the app. A thread with no window to ask in is refused, and the refusal
says to retry with the thread open, or names the trust switch.

## What access does not control

These refuse at every access level, Full included. No card asks; Settings ›
Permissions › "Not controlled by access" lists them.

- **The project's command rules.** `.rustcode/command-policy.toml` refuses or
  rewrites commands for the whole project (`docs/command-guard.md`). The
  refusal names the rule and the file.
- **The harness's own permission check.** Each harness has a permission layer
  RustCode does not run. Claude Code's auto mode has a classifier that refuses
  a call without asking RustCode, so there is no request to answer; the call
  fails with `Permission for this action was denied by the Claude Code auto
  mode classifier. Reason: [...]`. The engine recognizes that text
  (`rustcode/engine/src/harness/denial.rs`), marks the call
  (`_meta.rustcode.harnessDenial`), records a `harness.permission_denied`
  operational event, and the app shows under the call who refused, why, and
  that Claude Code's own mode (`permissions.defaultMode` in its settings.json)
  is where it is changed.

RustCode does not set Claude Code's permission mode from the thread's access
level, the way it sets Codex's (`Adapter::apply_access`). Claude Code's mode
is the user's own choice in Claude Code's settings, and mapping Full access to
a mode that skips its classifier would override a safety check the user turned
on there. A refusal is shown instead, with where to change it.

## Every layer

| Layer | Code | Covered by Full access |
| --- | --- | --- |
| Thread access (built-in harness tools) | `rustcode/engine/src/acp/permissions.rs` | Yes |
| A harness's approval requests | `acp/permissions.rs` (`external_tool_approver`), `harness/session.rs`, `apps/desktop/src/state/permissions.ts` | Yes |
| Orchestration hosted by the app | `apps/desktop/src/state/host.ts` (`permitted`) | Yes, except named accounts |
| Orchestration with no app hosting | `rustcode/engine/src/orchestration/local_threads.rs` | Only at Full; named accounts ask with the thread's card or are refused |
| Management tools | `rustcode/engine/src/manage.rs` (`gate`) | Asked with the thread's card; with no window, refused with the command to run |
| Outside agents | `rustcode/engine/src/orchestration/outside.rs` | No |
| Command guard | `rustcode/engine/src/command_guard.rs` | No (a project rule) |
| The harness's own check | `rustcode/engine/src/harness/denial.rs` | No (not RustCode's) |
