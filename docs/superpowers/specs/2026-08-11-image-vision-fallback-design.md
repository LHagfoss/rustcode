# Image Vision Fallback Design

## Goal

Make pasted images usable with text-only active models by preprocessing them through a dedicated configured vision model, while preserving the existing native multimodal request path for vision-capable models.

## Architecture

`ModelProfile` gains an optional explicit image-input capability and `AppConfig` gains an optional dedicated vision profile reference using the existing model-profile abstraction. Before `prepare_turn_request` builds provider messages, the turn orchestrator detects image markers in the current history, resolves the active capability, and—only for unsupported models—replaces image markers with ordered `[Attached image analysis]` wrappers containing concise structured vision output.

The fallback is isolated in a provider-agnostic image-analysis module. It reuses the existing file markers and image bytes, sends a normal OpenAI-compatible multimodal request to the configured vision profile, and caches successful descriptions in runtime state by SHA-256 image hash. A focus parameter is part of the internal analysis interface so targeted reinspection can be added later without another attachment system.

## Behavior

- Explicitly vision-capable profiles continue through `parse_multimodal_content` unchanged.
- Text-only profiles never receive image parts; each image is analyzed in source order.
- The original surrounding user text remains in the message.
- Cached descriptions avoid repeated vision calls for the same image bytes.
- Missing images, missing vision configuration, and vision request failures produce a useful harness-level error and abort the main request.
- Analysis text is bounded to structured downstream facts, not a generic caption, and contains no path/base64/provider metadata.

## Testing

Unit tests cover capability resolution, native-path preservation, single-image fallback, multiple-image ordering, cache reuse, vision failure, and unchanged text-only requests. Request-level tests use a local mock HTTP server or injectable request function so the active model and dedicated vision model calls can be asserted independently.
