---
name: delete-comment
description: Remove the code comments in one area of the repository by kind (tautology, stale, excuse, decision record), move decisions into docs/adr, file an issue per area for excused hacks, and tighten the comment ratchet. Use when asked to delete, clean up or ratchet down comments in a file, folder or crate.
---

# /delete-comment

Run one pass over an area (a crate, folder or file), not the whole repository:
each comment needs reading in context. The rule and its exemptions are in
`docs/adr/0001-comment-ratchet.md`.

## 1. List

```bash
python3 scripts/check-comments.py --list <path>...
```

This prints every counted comment line. Doc comments (`///`, `//!`, `/** */`),
`// SAFETY:` runs and tool directives are not listed and stay. Clap `///` text
is the CLI's `--help` output: keep it. Delete a non-clap doc comment only
when it is a pure tautology.

Read each run of lines with the code under it before you choose.

## 2. Categorise and act

| Kind | Looks like | Action |
| --- | --- | --- |
| Tautology | Repeats the next line, a test step narrating its assert, a section label | Delete |
| Stale | Names code, a file, a version or behaviour that is gone; "used to", "after X.swift" | Delete |
| Excuse | Justifies a workaround: an invariant held by a comment (`copies[0]` "always one"), a limit raised for a hack, the same code copied with "as for …" | Delete, and add it to the area's issue |
| Decision record | A non-obvious reason: ordering, a race, a security boundary, a UX rule, an issue number | Move it into an ADR |

Prefer turning a small reason into code over moving it: a name
(`template_safe_about`), a computed expected value in a test
(`1.0 * 1.25 + 0.1 * 10.0`), an assertion message, a type that makes the
invariant hold.

## 3. Write the ADRs

- One ADR per topic in `docs/adr/NNNN-<slug>.md`, numbered after the highest
  one. Head it with `Status: accepted, <YYYY-MM-DD>. Code: <files>`, then
  short sections of bullets in plain words. Add it to the table in
  `docs/adr/README.md`.
- Keep the issue numbers. Check every name you cite (functions, flags,
  timeouts) against the code.
- If an area doc in `docs/` already explains the topic, add the decision
  there instead and say so in the PR.

## 4. File the excuses

One issue per area, not one per comment:

```bash
gh issue create --title "refactor(<scope>): remove the workarounds comments excused in <area>" --body-file <file>
```

The body has one checklist item for each excuse: `path:line` (before the
deletion), the deleted comment quoted, the hack, the fix, and the acceptance
criteria. Follow `AGENTS.md` › Issues.

## 5. Ratchet and check

```bash
python3 scripts/check-comments.py --update   # the baseline only shrinks
python3 scripts/check-comments.py
```

Then run the repository's checks for the files you touched (`AGENTS.md` ›
Checks). After deleting a block, look for blank-line leftovers.

## 6. Report

In the PR body, give the counts by kind, the ADRs written, the issue URL,
and the lines removed (`git diff --shortstat`). Name the area for the next
pass by the size of each area's baseline:

```bash
awk '!/^#/{split($2,p,"/"); a[p[1]"/"p[2]]+=$1} END{for(k in a) print a[k], k}' scripts/comment-baseline.txt | sort -nr | head
```
