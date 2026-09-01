# Provider stream traces

Set `debug_verbose_network_logging = true` in the RustCode configuration to
enable an additional `provider.stream_trace` event in `debug.log`. The event
is associated with the active session and assistant turn and is intended to be
attached to provider interoperability bug reports.

The trace records only response structure: event sequence, SSE line byte
counts, finish reasons, tool-call index, whether an ID/name was present (plus a
non-reversible fingerprint and length), argument byte counts, and numeric
usage. It does not record authorization headers, API keys, prompts, file
contents, or raw tool arguments. A request contributes at most 256 events and
64 KiB of event data; dropped events are reported in the trace summary.

The setting also enables the existing full request-payload debug line, which
is intentionally opt-in because it can contain prompt and file content. Keep
it disabled unless that additional request detail is necessary.
