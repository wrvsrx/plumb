mod position;
mod server;

use async_lsp::client_monitor::ClientProcessMonitorLayer;
use async_lsp::concurrency::ConcurrencyLayer;
use async_lsp::panic::CatchUnwindLayer;
use async_lsp::router::Router;
use async_lsp::server::LifecycleLayer;
use async_lsp::tracing::TracingLayer;
use server::ServerState;
use tower::ServiceBuilder;
use tracing::Level;

#[tokio::main(flavor = "current_thread")]
pub async fn run_lsp() {
    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        ServiceBuilder::new()
            .layer(TracingLayer::default())
            .layer(LifecycleLayer::default())
            .layer(CatchUnwindLayer::default())
            .layer(ConcurrencyLayer::default())
            .layer(ClientProcessMonitorLayer::new(client.clone()))
            .service(Router::from_language_server(ServerState::new(client)))
    });

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();

    let stdin = async_lsp::stdio::PipeStdin::lock_tokio().expect("lock stdin");
    let stdout = async_lsp::stdio::PipeStdout::lock_tokio().expect("lock stdout");
    server
        .run_buffered(stdin, stdout)
        .await
        .expect("run language server");
}
