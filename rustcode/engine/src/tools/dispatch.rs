use super::schema::{mcp_canonical_name_for_clients, mcp_raw_name_is_unique};
use super::{TOOLS, ToolErrorKind, ToolExecutionOutput, exec, filesystem, search};
use serde_json::Value;
use std::sync::Arc;

/// Present a handler failure as the model-facing `error:` line.
///
/// Handlers are inconsistent about whether their message already opens with
/// `error:`, and prefixing unconditionally produced `error: error: ...`, which
/// reads like the harness lost track of its own output.
pub(super) fn as_error_message(message: &str) -> String {
    let trimmed = message.trim_start();
    if trimmed.to_ascii_lowercase().starts_with("error:") {
        trimmed.to_string()
    } else {
        format!("error: {trimmed}")
    }
}

/// Extract model-facing text from an MCP tool result. MCP servers may expose
/// their payload in `structuredContent` without duplicating it in `content`;
/// preserve the structured value in that case so RustCode does not hand the
/// model an empty tool result.
///
/// Binary items (`image`/`audio` data, `resource` blobs) are decoded into the
/// session's `artifacts/` folder and reported by path, size, type and a short
/// preview instead of handing the model base64 (#2564).
fn mcp_result_content(value: &Value) -> String {
    let artifacts = super::get_active_session_id()
        .and_then(|session| crate::config::get_active_session_artifacts_dir(&session));
    mcp_result_content_in(value, artifacts.as_deref())
}

fn mcp_result_content_in(value: &Value, artifacts: Option<&std::path::Path>) -> String {
    let result = value.get("result");
    let text = result
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .enumerate()
                .filter_map(|(index, item)| mcp_content_item(item, index, artifacts))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    if !text.trim().is_empty() {
        return text;
    }

    if let Some(structured) = result.and_then(|r| r.get("structuredContent")) {
        return serde_json::to_string_pretty(structured).unwrap_or_default();
    }

    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// One MCP content item as model-facing text, or `None` for kinds that
/// carry nothing for the model.
fn mcp_content_item(
    item: &Value,
    index: usize,
    artifacts: Option<&std::path::Path>,
) -> Option<String> {
    let string =
        |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    match item.get("type").and_then(Value::as_str) {
        Some("image" | "audio") => {
            let data = item.get("data").and_then(Value::as_str)?;
            Some(save_mcp_binary(
                data,
                string(item, "mimeType").as_deref(),
                None,
                index,
                artifacts,
            ))
        }
        Some("resource") => {
            let resource = item.get("resource")?;
            if let Some(text) = resource.get("text").and_then(Value::as_str) {
                return Some(text.to_owned());
            }
            let blob = resource.get("blob").and_then(Value::as_str)?;
            Some(save_mcp_binary(
                blob,
                string(resource, "mimeType").as_deref(),
                string(resource, "uri").as_deref(),
                index,
                artifacts,
            ))
        }
        Some("resource_link") => {
            string(item, "uri").map(|uri| format!("[MCP resource link: {uri}]"))
        }
        _ => string(item, "text"),
    }
}

/// Decode a base64 MCP payload into `artifacts` and describe it.
fn save_mcp_binary(
    base64_data: &str,
    mime: Option<&str>,
    uri: Option<&str>,
    index: usize,
    artifacts: Option<&std::path::Path>,
) -> String {
    use base64::Engine as _;
    let mime = mime.unwrap_or("application/octet-stream");
    let compact: String = base64_data
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&compact) else {
        return format!(
            "[binary MCP result ({mime}) could not be decoded; {} base64 characters omitted]",
            compact.len()
        );
    };
    let preview = binary_preview(&bytes);
    let saved = artifacts.and_then(|directory| {
        let name = mcp_artifact_name(mime, uri, index);
        std::fs::create_dir_all(directory).ok()?;
        let path = directory.join(name);
        std::fs::write(&path, &bytes).ok()?;
        Some(path)
    });
    match saved {
        Some(path) => format!(
            "[binary MCP result saved to {} ({} bytes, {mime}). Read it from that file; do not re-encode it.]\n{preview}",
            path.display(),
            bytes.len()
        ),
        None => format!(
            "[binary MCP result ({} bytes, {mime}) not shown: no session artifacts folder to save it in]\n{preview}",
            bytes.len()
        ),
    }
}

/// A unique artifact file name that keeps the resource's own name when it
/// has one, else an extension for its type.
fn mcp_artifact_name(mime: &str, uri: Option<&str>, index: usize) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let own = uri
        .and_then(|uri| uri.rsplit(['/', ':']).next())
        .map(|name| {
            name.chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                .take(80)
                .collect::<String>()
        })
        .filter(|name| !name.trim_matches('.').is_empty());
    let name = own.unwrap_or_else(|| {
        let extension = match mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "application/pdf" => "pdf",
            "application/gzip" | "application/x-gzip" => "gz",
            "application/zip" => "zip",
            "application/json" => "json",
            "audio/wav" | "audio/x-wav" => "wav",
            "audio/mpeg" => "mp3",
            _ => "bin",
        };
        format!("result.{extension}")
    });
    format!("mcp-{stamp}-{index}-{name}")
}

/// A short look inside binary content: decompressed text for gzip, the text
/// itself when it is text, else the first bytes in hex.
fn binary_preview(bytes: &[u8]) -> String {
    const LIMIT: usize = 400;
    let text_preview = |label: &str, data: &[u8]| -> Option<String> {
        let text = std::str::from_utf8(data)
            .or_else(|error| std::str::from_utf8(&data[..error.valid_up_to()]))
            .ok()?;
        let printable = text
            .chars()
            .all(|c| !c.is_control() || c.is_ascii_whitespace());
        (printable && !text.trim().is_empty()).then(|| {
            let shown: String = text.chars().take(LIMIT).collect();
            format!("{label}:\n{shown}")
        })
    };
    if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read as _;
        let mut decompressed = Vec::new();
        let _ = flate2::read::GzDecoder::new(bytes)
            .take(LIMIT as u64 * 4)
            .read_to_end(&mut decompressed);
        if let Some(preview) = text_preview("Preview (gzip, decompressed)", &decompressed) {
            return preview;
        }
    }
    if let Some(preview) = text_preview("Preview", &bytes[..bytes.len().min(LIMIT * 4)]) {
        return preview;
    }
    let hex = bytes
        .iter()
        .take(32)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("First bytes: {hex}")
}

/// Execute a tool by name and return its output with metadata.
///
/// MUST be called off the async runtime (e.g. inside `spawn_blocking`):
/// MCP calls block on the runtime handle and will panic if invoked on a
/// runtime worker thread.
pub(crate) fn execute_with_metadata(name: &str, args: &Value) -> ToolExecutionOutput {
    execute_with_metadata_cancellable(name, args, None)
}

/// Cancellable variant of [`execute_with_metadata`].
///
/// Same off-runtime contract: call only from blocking threads, never from
/// async tasks running on the Tokio runtime.
pub(crate) fn execute_with_metadata_cancellable(
    name: &str,
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> ToolExecutionOutput {
    execute_with_metadata_cancellable_for_call(name, args, cancel_token, None)
}

/// Call-scoped variant of [`execute_with_metadata_cancellable`].
///
/// Same off-runtime contract: the MCP path uses `Handle::block_on`, so this
/// must run on a blocking thread (see `network/tool_exec.rs` `spawn_blocking`
/// call sites), never directly on a runtime worker.
pub(crate) fn execute_with_metadata_cancellable_for_call(
    name: &str,
    args: &Value,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    call_id: Option<&str>,
) -> ToolExecutionOutput {
    // A built-in name never resolves to an MCP tool: canonical MCP names carry
    // the `mcp__` prefix and `mcp_raw_name_is_unique` rejects built-in names.
    // Skip the catalogue scan for them; it clones every server's tool list.
    let is_builtin = TOOLS.iter().any(|tool| tool.name == name);
    // Every built-in schema offers the display-only `summary`; drop it here so
    // handlers that reject unknown keys never see it.
    let without_summary;
    let args = match args.as_object() {
        Some(object) if is_builtin && object.contains_key(super::schema::CALL_SUMMARY_KEY) => {
            let mut object = object.clone();
            object.remove(super::schema::CALL_SUMMARY_KEY);
            without_summary = Value::Object(object);
            &without_summary
        }
        _ => args,
    };
    // Snapshot the clients and release the registry before the call: the UI
    // locks the same registry to label tool rows, so holding it across a slow
    // `tools/call` freezes rendering and serializes calls to other servers.
    let mcp_clients = if is_builtin {
        None
    } else {
        crate::mcp::get_mcp_registry()
            .lock()
            .map(|reg| reg.values().cloned().collect::<Vec<_>>())
            .ok()
    };
    if let Some(mut clients) = mcp_clients {
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        for client in &clients {
            if let Ok(tools) = client.get_tools()
                && tools
                    .iter()
                    .find_map(|t| {
                        let raw = t.get("name").and_then(|n| n.as_str())?;
                        let canonical = mcp_canonical_name_for_clients(&client.name, raw, &clients);
                        (name == canonical
                            || (name == raw && mcp_raw_name_is_unique(name, &clients)))
                        .then_some(raw)
                    })
                    .is_some()
            {
                let handle = tokio::runtime::Handle::current();
                let client_clone = Arc::clone(&client);
                let name_owned = name.to_string();
                let args_clone = args.clone();
                let raw_name = tools
                    .iter()
                    .find_map(|tool| {
                        let raw = tool.get("name").and_then(|n| n.as_str())?;
                        let canonical = mcp_canonical_name_for_clients(&client.name, raw, &clients);
                        (name == canonical
                            || (name == raw && mcp_raw_name_is_unique(name, &clients)))
                        .then_some(raw.to_string())
                    })
                    .unwrap_or(name_owned.clone());

                let res = handle.block_on(async move {
                    client_clone
                        .call(
                            "tools/call",
                            serde_json::json!({
                                "name": raw_name,
                                "arguments": args_clone
                            }),
                        )
                        .await
                });

                return match res {
                    Ok(val) => {
                        let success = !val
                            .get("result")
                            .and_then(|result| result.get("isError"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        ToolExecutionOutput {
                            content: mcp_result_content(&val),
                            success,
                            pending: false,
                            command: None,
                            exit_code: None,
                            truncated: false,
                            completeness: rustcode_core::ToolResultCompleteness::Complete,
                            replayed: false,
                            error_kind: (!success).then_some(ToolErrorKind::McpFailed),
                            retryable: false,
                            command_status: None,
                        }
                    }
                    Err(e) => ToolExecutionOutput::failure_with_kind(
                        format!("error: MCP tool call failed: {e}"),
                        ToolErrorKind::McpFailed,
                        true,
                    ),
                };
            }
        }
    }

    if name == "run_command" {
        return match exec::run_command_output_with_call_id(args, cancel_token, call_id) {
            Ok(output) => output,
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::CommandFailed,
                true,
            ),
        };
    }
    if name == "view_file" {
        return match filesystem::view_file_output(args) {
            Ok(output) => ToolExecutionOutput {
                content: output.content,
                success: true,
                pending: false,
                command: None,
                exit_code: None,
                truncated: output.truncated,
                completeness: output.completeness,
                replayed: false,
                error_kind: None,
                retryable: false,
                command_status: None,
            },
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            ),
        };
    }
    if name == "zoom_context" {
        return match super::context_archive::zoom(args) {
            Ok(content) => {
                let page: Value = serde_json::from_str(&content).expect("zoom_context emits JSON");
                let mut output = ToolExecutionOutput::success(content);
                if page["complete"] != true {
                    output.truncated = page["next_offset"].as_u64().is_some();
                    output.completeness = if output.truncated {
                        rustcode_core::ToolResultCompleteness::ByteTruncated
                    } else {
                        rustcode_core::ToolResultCompleteness::UserLimited
                    };
                }
                output
            }
            Err(error) => ToolExecutionOutput::failure(as_error_message(&error)),
        };
    }
    if name == "get_project_map" {
        return search::get_project_map_execution_output(args).unwrap_or_else(|error| {
            ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            )
        });
    }
    if matches!(name, "grep" | "glob" | "list_directory") {
        let result = match name {
            "grep" => search::grep_execution_output(args),
            "glob" => search::glob_execution_output(args),
            "list_directory" => search::list_directory_output(args),
            _ => unreachable!(),
        };
        return result.unwrap_or_else(|error| {
            ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            )
        });
    }

    match TOOLS.iter().find(|t| t.name == name) {
        Some(tool) => match (tool.handler)(args) {
            Ok(out) => ToolExecutionOutput::success(out),
            // A refused note is the caller's to reword, not a harness fault.
            Err(e) if name == "remember" && crate::memory::is_refusal(&e) => {
                ToolExecutionOutput::failure_with_kind(
                    as_error_message(&e),
                    ToolErrorKind::Validation,
                    false,
                )
            }
            Err(e) => ToolExecutionOutput::failure(as_error_message(&e)),
        },
        None => {
            let suggestion = super::fuzzy_match_tool_name(name)
                .map(|close| format!(" Did you mean '{close}'?"))
                .unwrap_or_default();
            ToolExecutionOutput::failure_with_kind(
                format!(
                    "error: unknown tool '{name}'.{suggestion} Available: {}",
                    TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
                ),
                ToolErrorKind::UnavailableDependency,
                false,
            )
        }
    }
}

#[allow(
    dead_code,
    reason = "preserved display-only interface for direct callers"
)]
pub fn execute(name: &str, args: &Value) -> String {
    execute_with_metadata(name, args).content
}

pub fn needs_confirmation(name: &str) -> bool {
    TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.requires_confirmation)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{mcp_result_content, mcp_result_content_in};

    #[test]
    fn refused_memory_note_is_a_validation_error() {
        // Refused before anything is read or written, so no store is touched.
        let output = super::execute_with_metadata(
            "remember",
            &serde_json::json!({"key": "api_key", "value": "0a1b2c3d4e5f"}),
        );
        assert!(!output.success);
        assert_eq!(output.error_kind, Some(super::ToolErrorKind::Validation));
        assert!(output.content.contains("(rule: credential assignment)"));
        assert!(!output.content.contains("0a1b2c3d4e5f"));
    }

    #[test]
    fn structured_only_mcp_results_reach_the_model() {
        let result = serde_json::json!({
            "result": {
                "structuredContent": {"mailboxes": [{"name": "INBOX"}]},
                "content": []
            }
        });
        let content = mcp_result_content(&result);
        assert!(content.contains("mailboxes"));
        assert!(content.contains("INBOX"));
    }

    #[test]
    fn ordinary_mcp_text_content_is_not_duplicated() {
        let result = serde_json::json!({
            "result": {
                "structuredContent": {"value": 1},
                "content": [{"type": "text", "text": "already formatted"}]
            }
        });
        assert_eq!(mcp_result_content(&result), "already formatted");
    }

    #[test]
    fn binary_mcp_results_are_saved_as_artifacts_instead_of_base64() {
        use base64::Engine as _;
        use std::io::Write as _;
        let artifacts = tempfile::tempdir().unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(b"2026-10-28 12:00:01 ERROR upload failed\n")
            .unwrap();
        let gzip = gzip.finish().unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&gzip);
        let result = serde_json::json!({"result": {"content": [
            {"type": "text", "text": "fetched log"},
            {"type": "resource", "resource": {
                "uri": "file:///logs/app.log.gz", "mimeType": "application/gzip", "blob": encoded
            }},
            {"type": "image", "data": base64::engine::general_purpose::STANDARD.encode([0x89, b'P', b'N', b'G', 0, 1]), "mimeType": "image/png"}
        ]}});

        let content = mcp_result_content_in(&result, Some(artifacts.path()));
        assert!(content.starts_with("fetched log\n"), "{content}");
        assert!(!content.contains(&encoded), "base64 leaked: {content}");
        assert!(content.contains("ERROR upload failed"), "{content}");
        assert!(content.contains(&format!("{} bytes, application/gzip", gzip.len())));
        assert!(content.contains("image/png"));
        let mut saved = std::fs::read_dir(artifacts.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        saved.sort();
        assert_eq!(saved.len(), 2);
        let log = saved
            .iter()
            .find(|path| path.to_string_lossy().ends_with("app.log.gz"))
            .unwrap();
        assert_eq!(std::fs::read(log).unwrap(), gzip);
        assert!(content.contains(&log.display().to_string()));
        assert!(
            saved
                .iter()
                .any(|path| path.to_string_lossy().ends_with(".png"))
        );
    }

    #[test]
    fn text_resources_and_plain_text_are_unchanged() {
        let result = serde_json::json!({"result": {"content": [
            {"type": "text", "text": "one"},
            {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "two"}}
        ]}});
        assert_eq!(mcp_result_content_in(&result, None), "one\ntwo");
    }
}
