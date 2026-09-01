# Rustcode benchmarks

Benchmark reports are stored as `benchmarks/YYYY-MM-DD/<benchmark>.md`.

Each report should record the Rustcode version and commit, model profile, model server, exact prompt, cache policy, workspace, session ID, verification results, Rustcode metrics, provider metrics, outcome, and known confounders.

Cache policy terminology:

- **Warm production-style:** keep the model server and its prefix/KV cache running between complete benchmark runs. Never clear cache between turns; turn-to-turn reuse is part of normal agent performance.
- **Cold run:** clear or restart the model cache once before the complete benchmark. Do not clear it between turns.

Do not compare a cold run to a warm run without labeling the difference. Preserve failed runs instead of rerunning until success.
