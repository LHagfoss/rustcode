# Architecture decision records

Why the code is the way it is, where a comment used to say it. Code comments
are banned by the comment ratchet ([0001](0001-comment-ratchet.md)); a reason
that matters goes here, in the doc for its area, or into the code as a name,
a type or a test.

Each record names the files it covers. When the code a record describes
changes, update the record in the same pull request.

| ADR | Decision |
| --- | --- |
| [0001](0001-comment-ratchet.md) | Ban code comments with a ratchet |
| [0002](0002-cli-startup.md) | What `rc` does before it runs a command |
| [0003](0003-cli-machines.md) | How `rc machines` lists, pairs and forgets machines |
| [0004](0004-cli-hub-and-server-restart.md) | The hub link and `rc server restart` |
| [0005](0005-cli-threads-and-usage.md) | `rc threads` status rules and `rc usage` totals |
| [0006](0006-cli-test-isolation.md) | CLI integration tests never touch the developer's setup |
| [0007](0007-desktop-pinned-section-headers.md) | Opened chat sections pin their header by script, not CSS `sticky` |
| [0008](0008-builtin-agent-crate.md) | The built-in agent gets its own crate, behind a `BuiltinAgent` trait |
| [0009](0009-objectives-and-coordination.md) | Objectives run as workflow runs, and the engine owns them |
| [0010](0010-thread-state-ownership.md) | Who owns each piece of thread state, and how it comes back after a crash |
| [0011](0011-os-computer-use.md) | OS computer use runs in a separate worker, behind the RustCode MCP server |
