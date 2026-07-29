use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, RawQuery, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Args;
use notify::{RecursiveMode, Watcher};
use plumb_extensions::TaskStatus;
use plumb_workspace::resolve_workspace_root;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::{
    render_note_html, GraphDirection, GraphQuery, WebQuery, WebTargetMode, WebTaskLocator, WebView,
    WebWorkspace, GRAPH_PRESETS, TASK_PRESETS,
};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const NOTE_HTML: &str = include_str!("../assets/note.html");
const APP_JS: &str = include_str!("../assets/app.js");
const QUERY_STATE_JS: &str = include_str!("../assets/query-state.js");
const STYLES_CSS: &str = include_str!("../assets/styles.css");
const FORCE_GRAPH_JS: &str = include_str!("../assets/vendor/force-graph.min.js");
const FORCE_GRAPH_LICENSE: &str = include_str!("../assets/vendor/FORCE-GRAPH-LICENSE.txt");
const CEL_JS: &str = include_str!("../assets/vendor/cel-js.min.js");
const CEL_JS_LICENSE: &str = include_str!("../assets/vendor/CEL-JS-LICENSE.txt");

#[derive(Debug, Args)]
pub(crate) struct ServeConfig {
    /// Workspace root. Defaults to the nearest ancestor containing .plumb/.
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

    /// Disable workspace file watching.
    #[arg(long)]
    no_watch: bool,

    /// Enable task mutations when binding to a non-loopback address.
    #[arg(long)]
    allow_mutations: bool,

    /// Hide notes whose CEL predicate evaluates to true.
    #[arg(long, value_name = "EXPR")]
    exclude: Option<String>,
}

#[derive(Clone)]
struct AppState {
    workspace: Arc<RwLock<WebWorkspace>>,
    html_cache: Arc<Mutex<HashMap<(String, i64), String>>>,
    changes: broadcast::Sender<u64>,
    current: Option<String>,
    exclude: Option<Arc<str>>,
    allow_mutations: bool,
}

pub(crate) fn serve(config: ServeConfig) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("plumb site serve: cannot start runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("plumb site serve: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: ServeConfig) -> Result<(), String> {
    let root = resolve_workspace_root(config.root.as_deref())?;
    let workspace = WebWorkspace::load(&root)?;
    workspace.graph_excluding(&GraphQuery::default(), config.exclude.as_deref())?;
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
        exclude: config.exclude.map(Arc::from),
        allow_mutations: mutations_enabled(config.host, config.allow_mutations),
    };
    if !config.no_watch {
        spawn_watcher(state.clone());
    }
    let mutations_enabled = state.allow_mutations;
    let router = router(state);
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(config.host, config.port))
        .await
        .map_err(|error| format!("cannot bind server: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("cannot read server address: {error}"))?;
    let url = format!("http://{address}/");
    println!("{url}");
    eprintln!(
        "plumb site serve: listening on {url} (task mutations {})",
        if mutations_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    axum::serve(listener, router)
        .await
        .map_err(|error| format!("server failed: {error}"))
}

fn mutations_enabled(host: IpAddr, explicitly_allowed: bool) -> bool {
    host.is_loopback() || explicitly_allowed
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::permanent("/graph") }))
        .route("/graph", get(index))
        .route("/tasks", get(index))
        .route("/api/query", post(query))
        .route("/api/query-presets", get(query_presets))
        .route("/api/graph", get(graph))
        .route("/api/tasks", get(tasks))
        .route("/api/task/{document_id}/{action}", post(update_task))
        .route("/api/note/{id}", get(note_api))
        .route("/note/{id}", get(note_page))
        .route("/resource/{id}/{name}", get(resource))
        .route("/events", get(events))
        .route("/favicon.ico", get(favicon))
        .route("/app.js", get(app_js))
        .route("/query-state.js", get(query_state_js))
        .route("/styles.css", get(styles_css))
        .route("/vendor/force-graph.min.js", get(force_graph_js))
        .route("/vendor/FORCE-GRAPH-LICENSE.txt", get(force_graph_license))
        .route("/vendor/cel-js.min.js", get(cel_js))
        .route("/vendor/CEL-JS-LICENSE.txt", get(cel_js_license))
        .layer(middleware::from_fn(log_requests))
        .with_state(state)
}

async fn log_requests(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if method != Method::GET
        || response.status().is_client_error()
        || response.status().is_server_error()
    {
        eprintln!(
            "plumb site serve: {method} {path} -> {}",
            response.status().as_u16()
        );
    }
    response
}

async fn index(State(state): State<AppState>) -> Response {
    let config = json!({
        "mode": "dynamic",
        "graphUrl": "/api/graph",
        "queryUrl": "/api/query",
        "presetsUrl": "/api/query-presets",
        "graphRoute": "/graph",
        "tasksRoute": "/tasks",
        "noteApiBase": "/api/note/",
        "noteApiSuffix": "",
        "notePageBase": "/note/",
        "notePageSuffix": "",
        "eventsUrl": "/events",
        "tasksUrl": "/api/tasks",
        "taskActionBase": "/api/task/",
        "taskMutations": state.allow_mutations,
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

async fn tasks(State(state): State<AppState>) -> Response {
    Json(state.workspace.read().await.tasks()).into_response()
}

async fn query(State(state): State<AppState>, Json(query): Json<WebQuery>) -> Response {
    let workspace = state.workspace.read().await;
    let result = match query.view {
        WebView::Graph => workspace
            .query_graph(&query, state.exclude.as_deref())
            .map(|snapshot| json!({ "view": "graph", "graph": snapshot })),
        WebView::Tasks => workspace
            .query_tasks(&query)
            .map(|snapshot| json!({ "view": "tasks", "tasks": snapshot })),
    };
    match result {
        Ok(result) => Json(result).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(error)).into_response(),
    }
}

async fn query_presets() -> Response {
    Json(json!({ "graph": GRAPH_PRESETS, "tasks": TASK_PRESETS })).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskActionRequest {
    revision: String,
    locator: WebTaskLocator,
}

async fn update_task(
    State(state): State<AppState>,
    AxumPath((document_id, action)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<TaskActionRequest>,
) -> Response {
    if !state.allow_mutations {
        return (
            StatusCode::FORBIDDEN,
            "task mutations are disabled for non-loopback servers",
        )
            .into_response();
    }
    if !same_origin(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin task mutations are forbidden",
        )
            .into_response();
    }
    let status = match action.as_str() {
        "complete" => TaskStatus::Done,
        "cancel" => TaskStatus::Canceled,
        _ => return (StatusCode::NOT_FOUND, "unknown task action").into_response(),
    };
    eprintln!(
        "plumb site serve: received task {action} for {document_id} ({:?})",
        request.locator
    );
    let (root, revision) = {
        let workspace = state.workspace.read().await;
        if let Err(error) =
            workspace.set_task_status(&document_id, &request.locator, &request.revision, status)
        {
            eprintln!(
                "plumb site serve: task {action} rejected for {document_id} ({:?}): {error}",
                request.locator
            );
            return (StatusCode::CONFLICT, error).into_response();
        }
        (workspace.root().to_path_buf(), workspace.revision() + 1)
    };
    let refreshed = match WebWorkspace::load_with_revision(root, revision) {
        Ok(workspace) => workspace,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };
    let tasks = refreshed.tasks();
    *state.workspace.write().await = refreshed;
    state.html_cache.lock().await.clear();
    let _ = state.changes.send(revision);
    eprintln!(
        "plumb site serve: task {action} completed for {document_id} ({:?})",
        request.locator
    );
    Json(tasks).into_response()
}

fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let (Ok(origin), Some(host)) = (
        origin
            .to_str()
            .ok()
            .and_then(|origin| url::Url::parse(origin).ok())
            .ok_or(()),
        headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok()),
    ) else {
        return false;
    };
    let authority = &origin[url::Position::BeforeHost..url::Position::AfterPort];
    origin.scheme() == "http" && authority.eq_ignore_ascii_case(host)
}

async fn graph(State(state): State<AppState>, RawQuery(raw_query): RawQuery) -> Response {
    let query = match parse_graph_query(raw_query.as_deref().unwrap_or_default()) {
        Ok(query) => query,
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };
    match state
        .workspace
        .read()
        .await
        .graph_excluding(&query, state.exclude.as_deref())
    {
        Ok(graph) => Json(graph).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    }
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
        "/graph",
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

async fn query_state_js() -> Response {
    asset("application/javascript; charset=utf-8", QUERY_STATE_JS)
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

async fn cel_js() -> Response {
    asset("application/javascript; charset=utf-8", CEL_JS)
}

async fn cel_js_license() -> Response {
    asset("text/plain; charset=utf-8", CEL_JS_LICENSE)
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
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn spawn_watcher(state: AppState) {
    tokio::spawn(async move {
        let root = state.workspace.read().await.root().to_path_buf();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut watcher = match notify::recommended_watcher(move |event| match event {
            Ok(event) if watch_event_affects_workspace(&event) => {
                let _ = sender.send(());
            }
            Ok(_) => {}
            Err(error) => eprintln!("plumb site serve: workspace watcher failed: {error}"),
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("plumb site serve: cannot create workspace watcher: {error}");
                return;
            }
        };
        if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
            eprintln!("plumb site serve: cannot watch workspace: {error}");
            return;
        }
        while receiver.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(180)).await;
            while receiver.try_recv().is_ok() {}
            let revision = state.workspace.read().await.revision() + 1;
            match WebWorkspace::load_with_revision(&root, revision) {
                Ok(workspace) => {
                    if let Err(error) =
                        workspace.graph_excluding(&GraphQuery::default(), state.exclude.as_deref())
                    {
                        eprintln!("plumb site serve: cannot apply graph exclusion: {error}");
                        continue;
                    }
                    if state.workspace.read().await.has_same_documents(&workspace) {
                        continue;
                    }
                    *state.workspace.write().await = workspace;
                    state.html_cache.lock().await.clear();
                    let _ = state.changes.send(revision);
                }
                Err(error) => eprintln!("plumb site serve: cannot refresh workspace: {error}"),
            }
        }
    });
}

fn watch_event_affects_workspace(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
    ) && event.paths.iter().any(|path| {
        path.extension()
            .is_some_and(|extension| extension == "plumb")
            || path.file_name().is_some_and(|name| name == ".ignore")
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};
    use notify::{Event, EventKind};
    use tokio::sync::{broadcast, Mutex, RwLock};
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn workspace_watcher_ignores_reads_and_unrelated_files() {
        let read = Event::new(EventKind::Access(AccessKind::Read)).add_path("note.plumb".into());
        let rust = Event::new(EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path("main.rs".into());

        assert!(!watch_event_affects_workspace(&read));
        assert!(!watch_event_affects_workspace(&rust));
    }

    #[test]
    fn workspace_watcher_accepts_plumb_content_changes() {
        let events = [
            Event::new(EventKind::Create(CreateKind::File)).add_path("new.plumb".into()),
            Event::new(EventKind::Modify(ModifyKind::Any)).add_path("note.plumb".into()),
            Event::new(EventKind::Remove(RemoveKind::File)).add_path("old.plumb".into()),
        ];

        assert!(events.iter().all(watch_event_affects_workspace));
        let ignore = Event::new(EventKind::Modify(ModifyKind::Any)).add_path(".ignore".into());
        assert!(watch_event_affects_workspace(&ignore));
    }

    #[test]
    fn task_mutations_reject_cross_origin_browser_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:4242"));
        assert!(same_origin(&headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:4242"),
        );
        assert!(same_origin(&headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.test"),
        );
        assert!(!same_origin(&headers));
        assert!(mutations_enabled(IpAddr::V4(Ipv4Addr::LOCALHOST), false));
        assert!(!mutations_enabled(IpAddr::V4(Ipv4Addr::UNSPECIFIED), false));
        assert!(mutations_enabled(IpAddr::V4(Ipv4Addr::UNSPECIFIED), true));
    }

    #[tokio::test]
    async fn web_routes_restore_views_and_execute_structured_queries() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("tasks.plumb"), "`-{.task #ready} Ready task\n").unwrap();
        let (changes, _) = broadcast::channel(2);
        let state = AppState {
            workspace: Arc::new(RwLock::new(WebWorkspace::load(&root).unwrap())),
            html_cache: Arc::new(Mutex::new(HashMap::new())),
            changes,
            current: None,
            exclude: None,
            allow_mutations: true,
        };
        let app = router(state);

        let root_response = app
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root_response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(root_response.headers()[header::LOCATION], "/graph");
        for path in ["/graph?preset=connected", "/tasks?preset=ready"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }

        let response = app.clone().oneshot(
            Request::post("/api/query")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"view":"tasks","presets":["ready"],"query":"Ready","sort":"source","traversal":{}}"#))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["view"], "tasks");
        assert_eq!(value["tasks"]["tasks"].as_array().unwrap().len(), 1);

        let response = app.clone().oneshot(
            Request::post("/api/query")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"view":"tasks","filters":["title.contains('Ready')","state == 'ready'"],"traversal":{}}"#))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["tasks"]["tasks"].as_array().unwrap().len(), 1);

        let response = app
            .oneshot(
                Request::post("/api/query")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"view":"tasks","filter":"title","traversal":{}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["source"], "custom");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-web-server-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
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
    std::fs::write(output.join("query-state.js"), QUERY_STATE_JS)
        .map_err(|error| format!("cannot write query-state.js: {error}"))?;
    std::fs::write(output.join("styles.css"), STYLES_CSS)
        .map_err(|error| format!("cannot write styles.css: {error}"))?;
    std::fs::write(output.join("vendor/force-graph.min.js"), FORCE_GRAPH_JS)
        .map_err(|error| format!("cannot write Force Graph: {error}"))?;
    std::fs::write(
        output.join("vendor/FORCE-GRAPH-LICENSE.txt"),
        FORCE_GRAPH_LICENSE,
    )
    .map_err(|error| format!("cannot write Force Graph license: {error}"))?;
    std::fs::write(output.join("vendor/cel-js.min.js"), CEL_JS)
        .map_err(|error| format!("cannot write CEL JS: {error}"))?;
    std::fs::write(output.join("vendor/CEL-JS-LICENSE.txt"), CEL_JS_LICENSE)
        .map_err(|error| format!("cannot write CEL JS license: {error}"))?;
    Ok(())
}
