# 0003: How `rc machines` lists, pairs and forgets machines

Status: accepted, 2026-11-21. Code: `rustcode/cli/src/machines.rs`.

## `list` is a glance

- A route that does not answer within seconds (3 s per route, 5 s in all)
  is skipped instead of waited on.
- A machine with no token is not dialled: there is nothing to show without
  getting in.
- The hint about overlay networks is checked against this computer's own
  interfaces, so it never asks about an overlay this computer is not on
  (#3421).
- A machine paired while its gateway reported no platform learns it from the
  handshake, so the row stops saying `unknown` (#3418).
- A LAN win stops the race before the overlay attempts start, so the overlay
  routes are probed afterwards to say why they are not used (#3202). An
  overlay win may instead leave a LAN failure in the race.
- A refusal that pairing again fixes reads as an expired pairing; anything
  else (a timeout, a different machine at that address) is offline.
- A machine file that still says `main: true` (written before every machine
  was a peer) is listed like any other.

## Network labels

- A `wss://` public route reaches the machine's public address through its
  tunnel.
- An `ssh -W` route connects from the machine itself: its gateway sees its
  own loopback, and SSH encrypts the way there.

## Pairing

- A connect code typed where an address was expected pairs with the code
  instead of asking, as `rc machines connect` does.
- The credential store is opened before the other machine is asked, in
  `add`, `connect` and `add-ssh`. A store that cannot answer stops the
  pairing at this end, not after the other machine accepted and enrolled
  this computer or the code was spent (#3325).
- A mistyped `--code` is refused before the machine is asked: the code has
  eight digits, and the frame that reports its length arrives only after the
  owner accepted.
- The token is saved before the machine. A saved machine without a token
  could only ever say "pair again"; a token whose machine was not saved is
  unreachable and harmless.
- Pairing counts as a successful connection, so `list` shows when it
  happened. A machine paired by connect code answered `describe` before the
  code went out, so it is saved as one that describes itself.

## Removing

The record goes first, because that is what the user asked for. A token left
behind is unusable without its record, so failing to delete it is a warning,
not an error.
