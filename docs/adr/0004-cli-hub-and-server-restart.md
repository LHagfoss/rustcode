# 0004: The hub link and `rc server restart`

Status: accepted, 2026-11-21. Code: `rustcode/cli/src/hub_member.rs`,
`rustcode/cli/src/run.rs`, `rustcode/cli/src/server_command.rs`.

## Hub link

- A machine that joined a hub keeps its link from the remote gateway, the
  one gateway process a machine runs (#3957).
- Only the gateway that holds this machine's owner lock links, so a second
  `remote serve` that is about to give up never replaces the link the running
  one keeps.
- A revocation, or a hub that stopped being one, ends the link with the
  gateway's error frame; `run` retries after any other loss.

## Server restart

- A restart helper started from inside the server waits `HELPER_DELAY`
  first, so the command that started it can end and its thread can record
  the result before the server stops.
- A caller that descends from the server cannot restart it directly:
  stopping the server would end the caller before it started the new one.
- With `--when-idle` from inside the server, the turn that asked runs until
  the command ends, so the detached helper does the waiting.

## Output

A closed pipe (`rc server logs --follow | head`) ends the command instead of failing
it.
