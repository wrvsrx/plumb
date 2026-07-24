use std::collections::HashMap;
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::{render_note_html, GraphDirection, GraphQuery, WebTargetMode, WebWorkspace};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const NOTE_HTML: &str = include_str!("../assets/note.html");
const APP_JS: &str = include_str!("../assets/app.js");
const STYLES_CSS: &str = include_str!("../assets/styles.css");
const FORCE_GRAPH_JS: &str = include_str!("../assets/vendor/force-graph.min.js");
const FORCE_GRAPH_LICENSE: &str = include_str!("../assets/vendor/FORCE-GRAPH-LICENSE.txt");

#[derive(Debug, Parser)]
#[command(name = "plumb graph", about = "Browse a plumb workspace graph")]
struct GraphConfig {
    /// Directory to scan recursively. Defaults to the current directory.
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Document to select initially, relative to the workspace root.
    #[arg(long, value_name = "PATH")]
    current: Option<PathBuf>,

    /// Address to bind. Defaults to loopback only.
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    host: IpAddr,

    /// TCP port. Zero selects an available random port.
    #[arg(long, default_value_t = 0)]
    port: u16,

    /// Do not open the graph in the default browser.
    #[arg(long)]
    no_open: bool,

    /// Disable workspace file watching.
    #[arg(long)]
    no_watch: bool,
}

#[derive(Clone)]
struct AppState {
    workspace: Arc<RwLock<WebWorkspace>>,
    html_cache: Arc<Mutex<HashMap<(String, i64), String>>>,
    changes: broadcast::Sender<u64>,
    current: Option<String>,
}

pub fn run_graph_cli(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let config = match GraphConfig::try_parse_from(args) {
        Ok(config) => config,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("plumb graph: cannot start runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plumb graph: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: GraphConfig) -> Result<(), String> {
    let root = config
        .root
        .unwrap_or(std::env::current_dir().map_err(|error| error.to_string())?);
    let workspace = WebWorkspace::load(&root)?;
    let current = config.current.as_ref().and_then(|path| {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            workspace.root().join(path)
        };
        workspace.document_id(path).map(str::to_string)
    });
    let (changes, _) = broadcast::channel(32);
    let state = AppState {
        workspace: Arc::new(RwLock::new(workspace)),
        html_cache: Arc::new(Mutex::new(HashMap::new())),
        changes,
        current,
    };
    if !config.no_watch {
        spawn_watcher(state.clone());
    }
    let router = router(state);
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(config.host, config.port))
        .await
        .map_err(|error| format!("cannot bind server: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("cannot read server address: {error}"))?;
    let url = format!("http://{address}/");
    println!("{url}");
    if !config.no_open {
        if let Err(error) = webbrowser::open(&url) {
            eprintln!("plumb graph: cannot open browser: {error}");
        }
    }
    axum::serve(listener, router)
        .await
        .map_err(|error| format!("server failed: {error}"))
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/graph", get(graph))
        .route("/api/note/{id}", get(note_api))
        .route("/note/{id}", get(note_page))
        .route("/resource/{id}/{name}", get(resource))
        .route("/events", get(events))
        .route("/favicon.ico", get(favicon))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/vendor/force-graph.min.js", get(force_graph_js))
        .route("/vendor/FORCE-GRAPH-LICENSE.txt", get(force_graph_license))
        .with_state(state)
}

async fn index(State(state): State<AppState>) -> Response {
    let config = json!({
        "mode": "dynamic",
        "graphUrl": "/api/graph",
        "noteApiBase": "/api/note/",
        "noteApiSuffix": "",
        "notePageBase": "/note/",
        "notePageSuffix": "",
        "eventsUrl": "/events",
        "current": state.current,
    });
    let html = INDEX_HTML
        .replace("__ASSET_PREFIX__", "/")
        .replace("__ROOT_PREFIX__", "/")
        .replace(
            "__PLUMB_CONFIG__",
            &escape_html_attribute(&config.to_string()),
        );
    secure_html(html)
}

async fn graph(State(state): State<AppState>, RawQuery(raw_query): RawQuery) -> Response {
    let query = match parse_graph_query(raw_query.as_deref().unwrap_or_default()) {
        Ok(query) => query,
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };
    Json(state.workspace.read().await.graph(&query)).into_response()
}

fn parse_graph_query(raw: &str) -> Result<GraphQuery, String> {
    let mut query = GraphQuery::default();
    for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "current" => query.current = Some(value.into_owned()),
            "depth" => {
                query.depth = Some(
                    value
                        .parse()
                        .map_err(|_| "depth must be a non-negative integer".to_string())?,
                )
            }
            "limit" => {
                query.limit = Some(
                    value
                        .parse()
                        .map_err(|_| "limit must be a non-negative integer".to_string())?,
                )
            }
            "direction" => {
                query.direction = match value.as_ref() {
                    "incoming" => GraphDirection::Incoming,
                    "outgoing" => GraphDirection::Outgoing,
                    "both" => GraphDirection::Both,
                    _ => return Err("direction must be incoming, outgoing, or both".to_string()),
                }
            }
            "kinds" => query.kinds.extend(
                value
                    .split(',')
                    .filter(|kind| !kind.is_empty())
                    .map(str::to_string),
            ),
            _ => {}
        }
    }
    Ok(query)
}

async fn note_api(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    let workspace = state.workspace.read().await.clone();
    let Some(note) = workspace.note(&id) else {
        return (StatusCode::NOT_FOUND, "unknown note").into_response();
    };
    let html = match cached_html(&state, &workspace, &id, note.revision).await {
        Ok(html) => html,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };
    Json(json!({
        "id": note.id,
        "title": note.title,
        "path": note.path,
        "revision": note.revision,
        "location": note.location,
        "backlinks": note.backlinks,
        "html": html,
    }))
    .into_response()
}

async fn note_page(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    let workspace = state.workspace.read().await.clone();
    let Some(note) = workspace.note(&id) else {
        return (StatusCode::NOT_FOUND, "unknown note").into_response();
    };
    let html = match cached_html(&state, &workspace, &id, note.revision).await {
        Ok(html) => html,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };
    let backlinks = render_backlinks(&workspace, &note.backlinks, "/note/", "");
    secure_html(render_note_page(
        &note.title,
        &note.path,
        &id,
        &html,
        &backlinks,
        "/",
        "/",
    ))
}

async fn cached_html(
    state: &AppState,
    workspace: &WebWorkspace,
    id: &str,
    revision: i64,
) -> Result<String, String> {
    let key = (id.to_string(), revision);
    if let Some(html) = state.html_cache.lock().await.get(&key).cloned() {
        return Ok(html);
    }
    let workspace = workspace.clone();
    let id = id.to_string();
    let html = tokio::task::spawn_blocking(move || {
        render_note_html(&workspace, &id, WebTargetMode::Dynamic)
    })
    .await
    .map_err(|error| format!("HTML render task failed: {error}"))??;
    state.html_cache.lock().await.insert(key, html.clone());
    Ok(html)
}

async fn resource(
    State(state): State<AppState>,
    AxumPath((id, name)): AxumPath<(String, String)>,
) -> Response {
    let record = state.workspace.read().await.resource(&id).cloned();
    let Some(record) = record else {
        return (StatusCode::NOT_FOUND, "unknown resource").into_response();
    };
    if name != record.name {
        return (StatusCode::NOT_FOUND, "unknown resource").into_response();
    }
    let bytes = match std::fs::read(&record.path) {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::NOT_FOUND, "resource is unavailable").into_response(),
    };
    let mime = mime_guess::from_path(&record.path).first_or_octet_stream();
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime.as_ref()).unwrap(),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        bytes,
    )
        .into_response()
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(state.changes.subscribe()).filter_map(|result| {
        result.ok().map(|revision| {
            Ok(Event::default()
                .event("workspace")
                .data(revision.to_string()))
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn app_js() -> Response {
    asset("application/javascript; charset=utf-8", APP_JS)
}

async fn styles_css() -> Response {
    asset("text/css; charset=utf-8", STYLES_CSS)
}

async fn force_graph_js() -> Response {
    asset("application/javascript; charset=utf-8", FORCE_GRAPH_JS)
}

async fn force_graph_license() -> Response {
    asset("text/plain; charset=utf-8", FORCE_GRAPH_LICENSE)
}

async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
        .into_response()
}

fn secure_html(html: String) -> Response {
    let mut response = Html(html).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn spawn_watcher(state: AppState) {
    tokio::spawn(async move {
        let root = state.workspace.read().await.root().to_path_buf();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut watcher = match notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("plumb graph: cannot create workspace watcher: {error}");
                return;
            }
        };
        if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
            eprintln!("plumb graph: cannot watch workspace: {error}");
            return;
        }
        while receiver.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(180)).await;
            while receiver.try_recv().is_ok() {}
            let revision = state.workspace.read().await.revision() + 1;
            match WebWorkspace::load_with_revision(&root, revision) {
                Ok(workspace) => {
                    *state.workspace.write().await = workspace;
                    state.html_cache.lock().await.clear();
                    let _ = state.changes.send(revision);
                }
                Err(error) => eprintln!("plumb graph: cannot refresh workspace: {error}"),
            }
        }
    });
}

pub(crate) fn render_note_page(
    title: &str,
    path: &str,
    id: &str,
    content: &str,
    backlinks: &str,
    asset_prefix: &str,
    root_prefix: &str,
) -> String {
    NOTE_HTML
        .replace("__TITLE__", &escape_html(title))
        .replace("__PATH__", &escape_html(path))
        .replace("__DOCUMENT_ID__", &escape_html_attribute(id))
        .replace("__CONTENT__", content)
        .replace("__BACKLINKS__", backlinks)
        .replace("__ASSET_PREFIX__", asset_prefix)
        .replace("__ROOT_PREFIX__", root_prefix)
}

pub(crate) fn render_backlinks(
    workspace: &WebWorkspace,
    locations: &[crate::SourceLocation],
    prefix: &str,
    suffix: &str,
) -> String {
    if locations.is_empty() {
        return "<p>No backlinks</p>".to_string();
    }
    let mut output = String::from("<ul class=\"backlink-list\">");
    for location in locations {
        let path = workspace.root().join(&location.path);
        let href = workspace
            .document_id(path)
            .map(|id| format!("{prefix}{id}{suffix}"))
            .unwrap_or_else(|| "#".to_string());
        output.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            escape_html_attribute(&href),
            escape_html(&location.path)
        ));
    }
    output.push_str("</ul>");
    output
}

pub(crate) fn render_index(
    config: &serde_json::Value,
    asset_prefix: &str,
    root_prefix: &str,
) -> String {
    INDEX_HTML
        .replace("__ASSET_PREFIX__", asset_prefix)
        .replace("__ROOT_PREFIX__", root_prefix)
        .replace(
            "__PLUMB_CONFIG__",
            &escape_html_attribute(&config.to_string()),
        )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_html_attribute(value: &str) -> String {
    escape_html(value)
}

pub(crate) fn write_assets(output: &Path) -> Result<(), String> {
    std::fs::create_dir_all(output.join("vendor"))
        .map_err(|error| format!("cannot create assets directory: {error}"))?;
    std::fs::write(output.join("app.js"), APP_JS)
        .map_err(|error| format!("cannot write app.js: {error}"))?;
    std::fs::write(output.join("styles.css"), STYLES_CSS)
        .map_err(|error| format!("cannot write styles.css: {error}"))?;
    std::fs::write(output.join("vendor/force-graph.min.js"), FORCE_GRAPH_JS)
        .map_err(|error| format!("cannot write Force Graph: {error}"))?;
    std::fs::write(
        output.join("vendor/FORCE-GRAPH-LICENSE.txt"),
        FORCE_GRAPH_LICENSE,
    )
    .map_err(|error| format!("cannot write Force Graph license: {error}"))?;
    Ok(())
}
