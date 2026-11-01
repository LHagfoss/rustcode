//! The ACP side of `localhost_preview` (#2138): each call becomes a
//! `_rustcode/preview` request to the client that advertised
//! `clientCapabilities._meta.rustcode.preview`, carrying the session it came
//! from so the client can check that session's grant.

use agent_client_protocol::{Client, ConnectionTo, UntypedMessage};
use serde_json::Value;

pub(crate) struct AcpPreviewClient {
    pub(crate) connection: ConnectionTo<Client>,
    pub(crate) session_id: String,
}

/// The request params: the engine's `{"operation": …}` plus the session.
pub(crate) fn preview_request_params(session_id: &str, mut params: Value) -> Value {
    if let Some(object) = params.as_object_mut() {
        object.insert("sessionId".to_owned(), Value::String(session_id.to_owned()));
    }
    params
}

impl crate::preview::PreviewClient for AcpPreviewClient {
    fn request(&self, params: Value) -> crate::preview::PreviewFuture {
        let connection = self.connection.clone();
        let params = preview_request_params(&self.session_id, params);
        Box::pin(async move {
            let message = UntypedMessage::new(crate::preview::PREVIEW_METHOD, params)
                .map_err(|error| error.to_string())?;
            connection
                .send_request(message)
                .block_task()
                .await
                .map_err(|error| error.to_string())
        })
    }
}

/// The session's preview client, when the connection's client has one.
pub(crate) fn preview_client_for(
    supported: bool,
    connection: &ConnectionTo<Client>,
    session_id: &str,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    supported.then(|| {
        std::sync::Arc::new(AcpPreviewClient {
            connection: connection.clone(),
            session_id: session_id.to_owned(),
        }) as std::sync::Arc<dyn crate::preview::PreviewClient>
    })
}

pub(crate) struct AcpDesktopClient {
    pub(crate) connection: ConnectionTo<Client>,
    pub(crate) session_id: String,
}
impl crate::preview::PreviewClient for AcpDesktopClient {
    fn request(&self, params: Value) -> crate::preview::PreviewFuture {
        let connection = self.connection.clone();
        let params = preview_request_params(&self.session_id, params);
        Box::pin(async move {
            let message = UntypedMessage::new(crate::desktop_control::METHOD, params)
                .map_err(|e| e.to_string())?;
            connection
                .send_request(message)
                .block_task()
                .await
                .map_err(|e| e.to_string())
        })
    }
}
pub(crate) fn desktop_client_for(
    supported: bool,
    connection: &ConnectionTo<Client>,
    session_id: &str,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    supported.then(|| {
        std::sync::Arc::new(AcpDesktopClient {
            connection: connection.clone(),
            session_id: session_id.to_owned(),
        }) as std::sync::Arc<dyn crate::preview::PreviewClient>
    })
}

/// The other half of `message_session` and its tools (#2637): each call
/// becomes a `_rustcode/orchestration` request naming the calling session.
pub(crate) struct AcpOrchestrationClient {
    pub(crate) connection: ConnectionTo<Client>,
    pub(crate) session_id: String,
}

impl crate::preview::PreviewClient for AcpOrchestrationClient {
    fn request(&self, params: Value) -> crate::preview::PreviewFuture {
        let connection = self.connection.clone();
        let params = preview_request_params(&self.session_id, params);
        Box::pin(async move {
            let message = UntypedMessage::new(crate::orchestration::METHOD, params)
                .map_err(|error| error.to_string())?;
            connection
                .send_request(message)
                .block_task()
                .await
                .map_err(|error| error.to_string())
        })
    }
}

pub(crate) fn orchestration_client_for(
    supported: bool,
    connection: &ConnectionTo<Client>,
    session_id: &str,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    supported.then(|| {
        std::sync::Arc::new(AcpOrchestrationClient {
            connection: connection.clone(),
            session_id: session_id.to_owned(),
        }) as std::sync::Arc<dyn crate::preview::PreviewClient>
    })
}

/// The app's offscreen Smart UI renderer for `smart_ui_preview`: each call
/// becomes a `_rustcode/smartUi/preview` request naming the calling session.
pub(crate) struct AcpSmartUiClient {
    pub(crate) connection: ConnectionTo<Client>,
    pub(crate) session_id: String,
}

impl crate::preview::PreviewClient for AcpSmartUiClient {
    fn request(&self, params: Value) -> crate::preview::PreviewFuture {
        let connection = self.connection.clone();
        let params = preview_request_params(&self.session_id, params);
        Box::pin(async move {
            let message = UntypedMessage::new(crate::smart_ui::PREVIEW_METHOD, params)
                .map_err(|error| error.to_string())?;
            connection
                .send_request(message)
                .block_task()
                .await
                .map_err(|error| error.to_string())
        })
    }
}

pub(crate) fn smart_ui_client_for(
    supported: bool,
    connection: &ConnectionTo<Client>,
    session_id: &str,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    supported.then(|| {
        std::sync::Arc::new(AcpSmartUiClient {
            connection: connection.clone(),
            session_id: session_id.to_owned(),
        }) as std::sync::Arc<dyn crate::preview::PreviewClient>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_its_session() {
        assert_eq!(
            preview_request_params(
                "s-1",
                serde_json::json!({"operation": {"action": "inspect"}})
            ),
            serde_json::json!({"operation": {"action": "inspect"}, "sessionId": "s-1"})
        );
    }
}
