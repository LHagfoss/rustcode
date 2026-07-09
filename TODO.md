# rustcode

## stuff

- [x] pressing shift + cambiamenti in the future tool calls".
- [x] accept tool call should insta close modal, currently wait command to finish
- [x] press ESC or Deny on "tool modal", should stop or purge agent, cancel it.
- [x] pressing ESC normally in chat should insta stop everything or like it stops but takes a while (bad ux), it needs visual feedback it stopped.
- [x] cmd + v doesnt paste (currently ctrl + v)
- [x] Thought: in ms, can be in seconds if more than like 1000ms

## Tool Optimizations (Claude Code style)

- [x] Improve `read_file`: Support multi-range reads to reduce turn count.
- [x] Improve `edit`: Move from exact string replacement to a more robust line-based or block-replacement system (less fragile than `old_string`).
- [ ] Implement Symbol Search: Add indexing for functions/classes to avoid brute-force grepping paths.
- [x] Shell Output: Review and optimize output capping to ensure critical logs aren't lost while preventing context overflow.
