use rmcp::model::{ClientRequest, ErrorCode, ErrorData, JsonRpcMessage};
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;

/// rmcp aborts startup on any pre-`initialize` request except `ping`, which kills the process
/// before the client can retry. Answer such probes with `-32601` and keep waiting for the legacy
/// handshake, so clients speaking the stateless spec (`server/discover`, SEP-1442) can fall back.
/// @see https://github.com/dmtrKovalenko/fff/issues/797
pub(crate) struct ProbeTolerantTransport<T> {
    inner: T,
    initialized: bool,
}

impl<T> ProbeTolerantTransport<T> {
    pub(crate) fn new(inner: T) -> Self {
        Self {
            inner,
            initialized: false,
        }
    }
}

impl<T> Transport<RoleServer> for ProbeTolerantTransport<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            let msg = self.inner.receive().await?;
            if self.initialized {
                return Some(msg);
            }

            let JsonRpcMessage::Request(request) = &msg else {
                return Some(msg);
            };

            match &request.request {
                ClientRequest::InitializeRequest(_) => {
                    self.initialized = true;
                    return Some(msg);
                }
                // rmcp answers pre-init pings itself
                ClientRequest::PingRequest(_) => return Some(msg),
                unsupported => {
                    let error = unsupported_probe_error(unsupported.method());
                    tracing::warn!(
                        method = unsupported.method(),
                        "rejecting pre-initialize request, awaiting initialize"
                    );
                    let id = request.id.clone();
                    self.inner
                        .send(JsonRpcMessage::error(error, Some(id)))
                        .await
                        .ok()?;
                }
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

fn unsupported_probe_error(method: &str) -> ErrorData {
    ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        format!(
            "{method} is not supported before initialize; this server uses the initialize handshake"
        ),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        ClientCapabilities, ClientJsonRpcMessage, CustomRequest, Implementation, InitializeRequest,
        InitializeRequestParams, NumberOrString, ServerJsonRpcMessage,
    };
    use std::collections::VecDeque;

    #[derive(Default)]
    struct MockTransport {
        incoming: VecDeque<ClientJsonRpcMessage>,
        sent: Vec<ServerJsonRpcMessage>,
    }

    impl Transport<RoleServer> for MockTransport {
        type Error = std::io::Error;

        fn send(
            &mut self,
            item: ServerJsonRpcMessage,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
            self.sent.push(item);
            async { Ok(()) }
        }

        fn receive(&mut self) -> impl Future<Output = Option<ClientJsonRpcMessage>> + Send {
            let next = self.incoming.pop_front();
            async move { next }
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn custom_request(id: i64, method: &str) -> ClientJsonRpcMessage {
        ClientJsonRpcMessage::request(
            ClientRequest::CustomRequest(CustomRequest::new(method, Some(serde_json::json!({})))),
            NumberOrString::Number(id),
        )
    }

    fn initialize_request(id: i64) -> ClientJsonRpcMessage {
        ClientJsonRpcMessage::request(
            ClientRequest::InitializeRequest(InitializeRequest::new(InitializeRequestParams::new(
                ClientCapabilities::default(),
                Implementation::new("probe", "1"),
            ))),
            NumberOrString::Number(id),
        )
    }

    #[tokio::test]
    async fn pre_init_probe_is_rejected_and_initialize_still_arrives() {
        let inner = MockTransport {
            incoming: VecDeque::from(vec![
                custom_request(1, "server/discover"),
                initialize_request(2),
            ]),
            sent: Vec::new(),
        };
        let mut transport = ProbeTolerantTransport::new(inner);

        let received = transport.receive().await.expect("initialize forwarded");
        assert!(matches!(
            received,
            JsonRpcMessage::Request(req)
                if matches!(req.request, ClientRequest::InitializeRequest(_))
        ));

        let JsonRpcMessage::Error(err) = &transport.inner.sent[0] else {
            panic!("expected an error response for the probe");
        };
        assert_eq!(err.id, Some(NumberOrString::Number(1)));
        assert_eq!(err.error.code, ErrorCode::METHOD_NOT_FOUND);
        assert!(err.error.message.contains("server/discover"));
    }

    #[tokio::test]
    async fn post_init_requests_pass_through_untouched() {
        let inner = MockTransport {
            incoming: VecDeque::from(vec![
                initialize_request(1),
                custom_request(2, "server/discover"),
            ]),
            sent: Vec::new(),
        };
        let mut transport = ProbeTolerantTransport::new(inner);

        transport.receive().await.expect("initialize forwarded");
        let received = transport.receive().await.expect("custom request forwarded");
        assert!(matches!(
            received,
            JsonRpcMessage::Request(req)
                if matches!(req.request, ClientRequest::CustomRequest(_))
        ));
        assert!(transport.inner.sent.is_empty());
    }
}
