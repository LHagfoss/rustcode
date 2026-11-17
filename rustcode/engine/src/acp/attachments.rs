//! `_rustcode/attachments/read` and `_rustcode/attachments/upload`: the images
//! a thread's messages carry, read back and sent ahead in chunks
//! (`crate::controller` attachments, `docs/remote.md` › Images in remote
//! threads). A paired computer's connection takes frames of at most 1 MiB, so
//! an image never travels whole: the app reads it, or uploads it before the
//! prompt that names it, 512 KiB at a time.

/// Read one chunk of an image a thread's message names.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/attachments/read", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentReadRequest {
    /// The path in the message's `![image](file://…)` marker, or its URI.
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    /// The thread whose reply names the picture, on this computer. With it, an
    /// image file in that thread's folders or the temporary folders is served
    /// too, not only the stored attachments.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Send one chunk of an image ahead of the prompt that carries it.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/attachments/upload", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentUploadRequest {
    /// Absent on the first chunk, which starts the upload.
    #[serde(default)]
    pub upload_id: Option<String>,
    pub mime_type: String,
    #[serde(default)]
    pub offset: u64,
    /// Base64 of at most 512 KiB.
    #[serde(default)]
    pub data: String,
    /// The last chunk: the image is checked and kept.
    #[serde(default)]
    pub done: bool,
}

pub(crate) fn read(request: AttachmentReadRequest) -> Result<serde_json::Value, String> {
    let chunk = match request.session_id.as_deref() {
        Some(session_id) => {
            crate::controller::read_thread_image_chunk(session_id, &request.path, request.offset)?
        }
        None => crate::controller::read_image_chunk(&request.path, request.offset)?,
    };
    serde_json::to_value(chunk).map_err(|error| error.to_string())
}

pub(crate) fn upload(request: AttachmentUploadRequest) -> Result<serde_json::Value, String> {
    let answer = crate::controller::upload_image_chunk(
        request.upload_id.as_deref(),
        &request.mime_type,
        request.offset,
        &request.data,
        request.done,
    )?;
    serde_json::to_value(answer).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;

    #[test]
    fn the_requests_read_what_the_app_sends() {
        let read: AttachmentReadRequest =
            serde_json::from_str(r#"{"path":"/a/attachments/x.png"}"#).unwrap();
        assert_eq!(read.offset, 0);
        assert_eq!(read.session_id, None);
        let read: AttachmentReadRequest =
            serde_json::from_str(r#"{"path":"/tmp/a.png","sessionId":"abc"}"#).unwrap();
        assert_eq!(read.session_id.as_deref(), Some("abc"));
        let upload: AttachmentUploadRequest = serde_json::from_str(
            r#"{"uploadId":"1_2","mimeType":"image/png","offset":5,"data":"QQ==","done":true}"#,
        )
        .unwrap();
        assert_eq!(upload.upload_id.as_deref(), Some("1_2"));
        assert!(upload.done);
        assert!(AttachmentReadRequest::matches_method(
            "_rustcode/attachments/read"
        ));
        assert!(AttachmentUploadRequest::matches_method(
            "_rustcode/attachments/upload"
        ));
    }
}
