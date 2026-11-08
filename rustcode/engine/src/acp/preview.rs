//! The ACP side of `localhost_preview` (#2138), desktop control,
//! orchestration and Smart UI previews: each call becomes a request to a
//! client that has the session and advertised the capability
//! (`clientCapabilities._meta.rustcode.*`), carrying the session it came from
//! so the client can check that session's grant. The client is looked up on
//! every call ([`SessionClients::request`]), so it follows the session from
//! one connection to another.

use agent_client_protocol::UntypedMessage;
use serde_json::Value;

use super::clients::{Capability, SessionClients};

/// The request params: the engine's `{"operation": …}` plus the session.
pub(crate) fn preview_request_params(session_id: &str, mut params: Value) -> Value {
    if let Some(object) = params.as_object_mut() {
        object.insert("sessionId".to_owned(), Value::String(session_id.to_owned()));
    }
    params
}

/// One kind of client request (`method`) on behalf of one session.
pub(crate) struct AcpClientRequests {
    clients: SessionClients,
    method: &'static str,
    need: Capability,
}

impl crate::preview::PreviewClient for AcpClientRequests {
    fn request(&self, params: Value) -> crate::preview::PreviewFuture {
        let clients = self.clients.clone();
        let (method, need) = (self.method, self.need);
        let params = preview_request_params(clients.session_id(), params);
        Box::pin(async move {
            let message = UntypedMessage::new(method, params).map_err(|error| error.to_string())?;
            clients
                .request(Some(need), message)
                .await
                .map_err(|error| error.to_string())
        })
    }

    /// The call was dropped before its answer arrived: the app ends it there and
    /// then, instead of holding the links it waited on until they time out.
    fn call_cancelled(&self, call_id: &str) {
        if self.need != Capability::Orchestration {
            return;
        }
        let params = crate::orchestration::cancelled_params(self.clients.session_id(), call_id);
        let Ok(message) = UntypedMessage::new(crate::orchestration::CANCELLED_METHOD, params)
        else {
            return;
        };
        // Best effort: a client that cannot hear this answers a call nobody reads.
        if let Some(connection) = self.clients.requester(Some(Capability::Orchestration)) {
            let _ = connection.send_notification(message);
        }
    }
}

fn client_for(
    supported: bool,
    clients: &SessionClients,
    method: &'static str,
    need: Capability,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    supported.then(|| {
        std::sync::Arc::new(AcpClientRequests {
            clients: clients.clone(),
            method,
            need,
        }) as std::sync::Arc<dyn crate::preview::PreviewClient>
    })
}

/// The session's preview client, when its client has one.
pub(crate) fn preview_client_for(
    supported: bool,
    clients: &SessionClients,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    client_for(
        supported,
        clients,
        crate::preview::PREVIEW_METHOD,
        Capability::Preview,
    )
}

pub(crate) fn desktop_client_for(
    supported: bool,
    clients: &SessionClients,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    client_for(
        supported,
        clients,
        crate::desktop_control::METHOD,
        Capability::Desktop,
    )
}

/// The host and version for a session whose client advertised orchestration
/// `version` (0 for none). The other half of `message_session` and its tools
/// (#2637): each call becomes a `_rustcode/orchestration` request naming the
/// calling session.
pub(crate) fn orchestration_for(
    version: u8,
    clients: &SessionClients,
) -> (
    Option<std::sync::Arc<dyn crate::preview::PreviewClient>>,
    u8,
) {
    (orchestration_client_for(version >= 1, clients), version)
}

pub(crate) fn orchestration_client_for(
    supported: bool,
    clients: &SessionClients,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    client_for(
        supported,
        clients,
        crate::orchestration::METHOD,
        Capability::Orchestration,
    )
}

/// The app's offscreen Smart UI renderer for `smart_ui_preview`: each call
/// becomes a `_rustcode/smartUi/preview` request naming the calling session.
pub(crate) fn smart_ui_client_for(
    supported: bool,
    clients: &SessionClients,
) -> Option<std::sync::Arc<dyn crate::preview::PreviewClient>> {
    client_for(
        supported,
        clients,
        crate::smart_ui::PREVIEW_METHOD,
        Capability::SmartUiPreview,
    )
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
