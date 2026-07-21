# Agent-harness debugging recap — for porting into another harness

Context handoff. We debugged why a local terminal coding agent (`rustcode`, Rust)
kept failing a simple refactor task when driven by a **stock Devstral Small 2 24B**
(Unsloth `Devstral-Small-2-24B-Instruct-2512-GGUF:UD-Q6_K_XL`) served by Ollama over
an OpenAI-compatible `/v1/chat/completions` endpoint. Below is every root cause we
found and the fix, so you can check whether YOUR harness has the same gaps.

## The model / serving config (check these FIRST — cheapest wins)

Ollama's `/v1/chat/completions` endpoint quirks that bit us:

1. **`repeat_penalty` from the Modelfile is the ONLY repetition guard that lands.**
   `frequency_penalty` / `presence_penalty` sent in the request are effectively
   ignored by Ollama. So repetition control lives in the Modelfile.
   - **Catastrophic bug we hit:** `repeat_penalty 0.3`. Values `<1.0` REWARD
     repetition. The model collapsed into repeating one phrase 100× ("token-level
     repetition collapse"). Correct value for code: **`repeat_penalty 1.1`**
     (1.0 = neutral/no guard, >1.15 = starts hurting code, 0.3 = actively loops).
2. **A `temperature` in the REQUEST overrides the Modelfile PARAMETER.** We were
   sending `temperature: 0.7` hardcoded in the request, silently overriding the
   user's carefully-tuned `0.15`. For structured tool-calling / code, use
   **~0.15–0.2**. 0.7 makes a 24B incoherent and prone to repetition collapse.
3. Modelfile sanity for this model: `temperature 0.2`, `top_p 0.95`,
   `repeat_penalty 1.1`, `num_predict 8192`. `num_ctx 128000` works but is heavy on
   VRAM/adherence; 32k–64k is safer if the task fits.

## Harness bugs we found + fixed (each one made the model "look stupid")

1. **Compiler feedback couldn't spawn.** `Command::new("cargo")` failed with ENOENT
   because a GUI/TUI launch doesn't inherit the shell PATH. Result: agent got zero
   compile errors back → edited blind → thrashed. **Fix:** resolve the binary to an
   absolute path (check `~/.cargo/bin`, `/opt/homebrew/bin`, `/usr/local/bin`,
   `/usr/bin`, then fall back to bare name). Also bump the `cargo check` timeout to
   120s (a real crate check exceeds a few seconds; a short timeout = blind agent).

2. **Amnesia on the file being edited.** History compaction hard-capped *tool
   outputs* at 30k tokens regardless of the model's real context window (128k). The
   file under edit was ~32k tokens, so reading it evicted the start mid-read → the
   model could never hold the whole file → re-read forever. **Fix:** scale the
   tool-output cap to the real budget (we raised 30k→90k; better: make it a % of the
   model context window, not a fixed constant).

3. **Loop detector dodged by cosmetic range shifts.** Dedup keyed `view_file` on
   `path|start-end`, so shifting the line range by 1 produced a "new" call and never
   tripped. **Fix:** bucket `view_file` by `path + coarse region` (e.g. `start/200`)
   so re-reading the SAME area collapses to one category (trips the detector) while
   reading DIFFERENT parts of a big file stays distinct (legit paging, doesn't
   over-fire). Read-only tools without ranges (grep/glob/list) key on target only.

4. **No finish gate.** The model declared "Task completed" on a RED build (it had
   left duplicate defs + an unimported macro). **Fix:** when the model tries to
   finish with prose AND it edited code this task, run a compile check; if it fails,
   reject the finish, hand the compiler errors back, force another round. Bound it
   (max 2 retries) so it can't spin. Gate on ERRORS only, not warnings.

5. **Loop-abort produced a junk "answer".** On abort we ran one tools-disabled
   "wrap-up" turn, but under the JSON tool protocol the model emits tool calls as
   *text*, so "tools are disabled" is just an instruction it ignores — it emitted a
   `view_file` call as text, which we then saved verbatim as the final answer.
   **Fix:** on the forced wrap-up turn, strip tool-call syntax (```tool fences and
   Mistral `[TOOL_CALLS]...[ARGS]{...}`) from the content; save the remaining prose,
   or a synthesized fallback message if nothing readable remains.

6. **Temperature 0.7** (see serving config above) — hardcoded in the request,
   dropped to 0.2.

## Model-behavior weaknesses observed (harness-independent — worth guarding)

- Re-reads the same region repeatedly even after `grep` already returned exact line
  numbers. Weak planning / working memory. Guard: aggressive-but-fair loop detection
  (fix 3) + explicit "read once, then act" in the task prompt.
- Dumps multiple speculative tool calls in ONE turn instead of one-at-a-time waiting
  for results. Consider a system-prompt rule: "emit exactly one tool call per turn,
  then wait for its result."
- Latches onto an instruction phrase and repeats it (amplified massively by the
  `repeat_penalty 0.3` bug).

## The evaluation task we used (good minimal probe)

Moving 11 scattered functions across a 3200-line file overwhelmed the 24B. Shrink to
a **1-function move with a re-export so no call sites change** — isolates "can it do
a clean move + compile green" from planning load:

> Create `src/network/history.rs` starting with `use super::*;`. Move ONE function
> (`count_tokens`) verbatim out of `src/network.rs` into it, make it
> `pub(crate) fn`. In `src/network.rs` add `pub(crate) mod history;` and
> `pub(crate) use history::count_tokens;` (the re-export means no call sites change).
> Copy the body verbatim, never stub. Read the function ONCE then act. Run
> `cargo build`; it must compile clean before finishing; do not report done on a red
> build.

## Debug order that worked (apply to any harness)

1. Check serving/sampler config first: `repeat_penalty` (must be ≥1.0, ~1.1),
   request `temperature` (~0.2 for tools), verify the request isn't overriding a
   good Modelfile.
2. Verify the compiler/feedback tool actually spawns and returns errors (PATH!).
3. Verify the model can hold the whole target file in context (compaction cap vs
   file size vs context window).
4. Verify loop detection catches cosmetic-variation spins without killing legit work.
5. Verify the agent can't finish on a broken build, and that a forced stop yields
   real prose, not a raw tool call.
