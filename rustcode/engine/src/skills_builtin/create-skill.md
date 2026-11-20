---
name: create-skill
description: Create a new RustCode skill (a SKILL.md an agent loads on demand). Use when the user asks to make, write or add a skill, or to turn a workflow they repeat into one.
triggers: create a skill, new skill, make a skill, write a skill, add a skill
keywords: skill, SKILL.md, workflow, instructions, capability
---

# Create a skill

A skill is one folder holding `SKILL.md`, plus any scripts or references it
points to. RustCode lists it in Settings › Capabilities and offers it to
every thread on the next turn.

## Where

- User (every project): `~/.config/rustcode/skills/<name>/SKILL.md`
  (`%APPDATA%\rustcode\skills` on Windows).
- Project (this repository only): `.rustcode/skills/<name>/SKILL.md`.

Ask which one when the user did not say; default to user.

## Format

```markdown
---
name: <name>            # the folder name: lowercase letters, digits, dashes
description: <what it does>. Use when <the requests it answers>.
triggers: <comma-separated phrases a user types>
keywords: <comma-separated topic words>
---

# <Title>

Steps, commands and rules the agent follows, shortest first.
```

The description decides when the skill is picked: say what it does and
when to use it in one or two sentences. Keep the body under about 500
lines; move long references into files next to `SKILL.md` and name them.

## Steps

1. Ask for anything that blocks you: the job, an example request, the
   scope. Do not invent the user's workflow.
2. Pick a short name that is not taken (`manage_capabilities` with
   `kind: "skill", action: "list"`, or `rc skill list`).
3. Write it. With the RustCode MCP server, call `manage_capabilities` with
   `kind: "skill", action: "create", name, content` (the whole `SKILL.md`):
   the user approves it and it lands in the user folder. Otherwise write the
   file yourself; a project skill is always a file you write.
4. Confirm it is listed (`rc skill list`) and tell the user its path.
