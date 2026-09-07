use std::ops::ControlFlow;
use std::task::{Context, Poll};

use async_lsp::{AnyEvent, AnyNotification, AnyRequest, ErrorCode, LspService, ResponseError};
use tower::Service;

pub(crate) const MAX_IN_FLIGHT_REQUESTS: usize = 256;

pub(crate) struct NonblockingAdmission<S>(S);

impl<S> NonblockingAdmission<S> {
    pub(crate) fn new(service: S) -> Self {
        Self(service)
    }
}

impl<S: LspService> Service<AnyRequest> for NonblockingAdmission<S>
where
    S::Error: From<ResponseError>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.0.poll_ready(cx) {
            // async-lsp stops dispatching internal events while awaiting readiness.
            Poll::Pending => Poll::Ready(Err(ResponseError::new(
                ErrorCode::REQUEST_FAILED,
                "server busy: too many in-flight requests; retry after pending requests finish",
            )
            .into())),
            ready => ready,
        }
    }

    fn call(&mut self, request: AnyRequest) -> Self::Future {
        self.0.call(request)
    }
}

impl<S: LspService> LspService for NonblockingAdmission<S>
where
    S::Error: From<ResponseError>,
{
    fn notify(&mut self, notification: AnyNotification) -> ControlFlow<async_lsp::Result<()>> {
        self.0.notify(notification)
    }

    fn emit(&mut self, event: AnyEvent) -> ControlFlow<async_lsp::Result<()>> {
        self.0.emit(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use async_lsp::{concurrency::ConcurrencyLayer, router::Router};
    use futures::FutureExt;
    use serde_json::json;
    use tokio::sync::watch;
    use tower::Layer;

    struct Finished;

    fn service() -> (
        impl LspService<Response = serde_json::Value, Error = ResponseError>,
        Arc<AtomicBool>,
        watch::Sender<bool>,
    ) {
        let (sender, _) = watch::channel(false);
        let observed = Arc::new(AtomicBool::new(false));
        let notified = Arc::clone(&observed);
        let mut router = Router::<_, ResponseError>::new(sender.clone());
        router.request::<lsp_types::request::Shutdown, _>(|sender, ()| {
            let mut receiver = sender.subscribe();
            async move {
                receiver.wait_for(|finished| *finished).await.unwrap();
                Ok(())
            }
        });
        router.event::<Finished>(|sender, _| {
            sender.send_replace(true);
            ControlFlow::Continue(())
        });
        router.notification::<lsp_types::notification::Initialized>(move |_, _| {
            notified.store(true, Ordering::Relaxed);
            ControlFlow::Continue(())
        });
        (
            NonblockingAdmission::new(
                ConcurrencyLayer::new(NonZeroUsize::new(1).unwrap()).layer(router),
            ),
            observed,
            sender,
        )
    }

    fn request(id: i32) -> AnyRequest {
        serde_json::from_value(json!({"id":id,"method":"shutdown","params":null})).unwrap()
    }

    #[tokio::test]
    async fn saturated_admission_keeps_notifications_live() {
        let (mut service, observed, sender) = service();
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        let mut pending = Box::pin(service.call(request(1)));
        assert!(pending.as_mut().now_or_never().is_none());
        let Poll::Ready(Err(error)) = service.poll_ready(&mut cx) else {
            panic!("saturated admission must fail immediately, never block the main loop");
        };
        assert_eq!(error.code, ErrorCode::REQUEST_FAILED);
        assert!(error.message.contains("retry"));
        assert!(matches!(
            service.notify(
                serde_json::from_value(json!({"method":"initialized","params":{}})).unwrap()
            ),
            ControlFlow::Continue(())
        ));
        assert!(observed.load(Ordering::Relaxed));
        sender.send_replace(true);
        assert_eq!(pending.await.unwrap(), json!(null));
        assert!(matches!(service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
        assert_eq!(service.call(request(2)).await.unwrap(), json!(null));
    }

    #[tokio::test]
    async fn cancellation_and_dropped_requests_release_saturated_admission() {
        for cancel in [true, false] {
            let (mut service, _, sender) = service();
            let waker = futures::task::noop_waker();
            let mut cx = Context::from_waker(&waker);
            assert!(matches!(service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
            let mut pending = Box::pin(service.call(request(1)));
            assert!(pending.as_mut().now_or_never().is_none());
            assert!(matches!(service.poll_ready(&mut cx), Poll::Ready(Err(_))));
            if cancel {
                assert!(matches!(
                    service.notify(
                        serde_json::from_value(
                            json!({"method":"$/cancelRequest","params":{"id":1}})
                        )
                        .unwrap()
                    ),
                    ControlFlow::Continue(())
                ));
                assert_eq!(
                    pending.await.unwrap_err().code,
                    ErrorCode::REQUEST_CANCELLED
                );
            } else {
                drop(pending);
            }
            assert!(matches!(service.poll_ready(&mut cx), Poll::Ready(Ok(()))));
            sender.send_replace(true);
            assert_eq!(service.call(request(2)).await.unwrap(), json!(null));
        }
    }

    #[tokio::test]
    async fn saturated_main_loop_installs_completion_before_accepting_the_next_request() {
        let (main_loop, _) = async_lsp::MainLoop::new_server(|client| {
            let (sender, _) = watch::channel(false);
            let mut router = Router::<_, ResponseError>::new(sender);
            router.request::<lsp_types::request::Shutdown, _>(|sender, ()| {
                let mut receiver = sender.subscribe();
                async move {
                    receiver.wait_for(|finished| *finished).await.unwrap();
                    Ok(())
                }
            });
            router.notification::<lsp_types::notification::Initialized>(move |_, _| {
                client.emit(Finished).unwrap();
                ControlFlow::Continue(())
            });
            router.event::<Finished>(|sender, _| {
                sender.send_replace(true);
                ControlFlow::Continue(())
            });
            router.notification::<lsp_types::notification::Exit>(|_, _| ControlFlow::Break(Ok(())));
            NonblockingAdmission::new(
                ConcurrencyLayer::new(NonZeroUsize::new(1).unwrap()).layer(router),
            )
        });
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"shutdown","params":null}),
            json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
            json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
            json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
            json!({"jsonrpc":"2.0","method":"exit","params":null}),
        ];
        let mut input = Vec::new();
        for message in messages {
            use std::io::Write;
            let body = serde_json::to_vec(&message).unwrap();
            write!(input, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
            input.extend(body);
        }
        let mut output = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            main_loop.run_buffered(futures::io::Cursor::new(input), &mut output),
        )
        .await
        .expect("saturation must not block the worker completion event")
        .unwrap();
        let mut bytes = output.as_slice();
        let mut responses = Vec::<serde_json::Value>::new();
        while !bytes.is_empty() {
            let header_end = bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let length: usize = std::str::from_utf8(&bytes[..header_end])
                .unwrap()
                .strip_prefix("Content-Length: ")
                .unwrap()
                .parse()
                .unwrap();
            let (body, rest) = bytes[header_end + 4..].split_at(length);
            responses.push(serde_json::from_slice(body).unwrap());
            bytes = rest;
        }
        assert_eq!(responses.len(), 3);
        let response = |id| {
            responses
                .iter()
                .find(|response| response["id"] == id)
                .unwrap()
        };
        assert_eq!(response(2)["error"]["code"], -32803);
        for id in [1, 3] {
            assert!(response(id).get("error").is_none());
            assert_eq!(response(id).get("result"), Some(&json!(null)));
        }
    }
}
