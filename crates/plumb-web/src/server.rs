use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query as AxumQuery, RawQuery, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Args;
use notify::{RecursiveMode, Watcher};
use plumb_semantics::TaskStatus;
use plumb_workspace::resolve_workspace_root;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::presentation::{
    render_backlinks, render_index, render_note_page, AGENDA_STATE_JS, APP_JS, FORCE_GRAPH_JS,
    FORCE_GRAPH_LICENSE, QUERY_STATE_JS, REVISION_STATE_JS, STYLES_CSS, TASK_UI_JS,
};
use crate::{
    render_note_html, GraphDirection, GraphQuery, WebEventInput, WebEventLocator, WebQuery,
    WebTaskInput, WebTaskLocator, WebTaskPlacement, WebView, WebWorkspace, GRAPH_PRESETS,
    TASK_PRESETS,
};

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
        .route("/agenda", get(index))
        .route("/api/query", post(query))
        .route("/api/query-presets", get(query_presets))
        .route("/api/graph", get(graph))
        .route("/api/tasks", get(tasks))
        .route("/api/task-candidates", get(task_candidates))
        .route("/api/task/{document_id}/{action}", post(update_task))
        .route("/api/events", get(event_snapshot))
        .route("/api/event/{document_id}/{action}", post(update_event))
        .route("/api/note/{id}", get(note_api))
        .route("/note/{id}", get(note_page))
        .route("/resource/{id}/{name}", get(resource))
        .route("/events", get(events))
        .route("/favicon.ico", get(favicon))
        .route("/app.js", get(app_js))
        .route("/agenda-state.js", get(agenda_state_js))
        .route("/query-state.js", get(query_state_js))
        .route("/task-ui.js", get(task_ui_js))
        .route("/revision-state.js", get(revision_state_js))
        .route("/styles.css", get(styles_css))
        .route("/vendor/force-graph.min.js", get(force_graph_js))
        .route("/vendor/FORCE-GRAPH-LICENSE.txt", get(force_graph_license))
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
        "queryUrl": "/api/query",
        "presetsUrl": "/api/query-presets",
        "graphRoute": "/graph",
        "tasksRoute": "/tasks",
        "agendaRoute": "/agenda",
        "noteApiBase": "/api/note/",
        "noteApiSuffix": "",
        "notePageBase": "/note/",
        "notePageSuffix": "",
        "eventsUrl": "/events",
        "eventSnapshotUrl": "/api/events",
        "eventActionBase": "/api/event/",
        "taskActionBase": "/api/task/",
        "taskCandidatesUrl": "/api/task-candidates",
        "taskMutations": state.allow_mutations,
        "eventMutations": state.allow_mutations,
        "current": state.current,
    });
    secure_html(render_index(&config, "/", "/"))
}

async fn tasks(State(state): State<AppState>) -> Response {
    Json(state.workspace.read().await.tasks()).into_response()
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TaskCandidateQuery {
    #[serde(default)]
    query: String,
    document_id: Option<String>,
    limit: Option<usize>,
}

async fn task_candidates(
    State(state): State<AppState>,
    AxumQuery(query): AxumQuery<TaskCandidateQuery>,
) -> Response {
    let workspace = state.workspace.read().await;
    let limit = query.limit.unwrap_or(50).min(500);
    Json(json!({
        "revision": workspace.revision(),
        "tasks": workspace.task_candidates(&query.query, query.document_id.as_deref(), limit),
    }))
    .into_response()
}

async fn event_snapshot(State(state): State<AppState>) -> Response {
    Json(state.workspace.read().await.events()).into_response()
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
    locator: Option<WebTaskLocator>,
    task: Option<WebTaskInput>,
    placement: Option<WebTaskPlacement>,
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
    eprintln!(
        "plumb site serve: received task {action} for {document_id} ({:?})",
        request.locator
    );
    let (path, revision) = {
        let workspace = state.workspace.read().await;
        let result = match action.as_str() {
            "complete" | "cancel" => request
                .locator
                .as_ref()
                .ok_or_else(|| "task status action requires a locator".to_string())
                .and_then(|locator| {
                    workspace.set_task_status(
                        &document_id,
                        locator,
                        &request.revision,
                        if action == "complete" {
                            TaskStatus::Done
                        } else {
                            TaskStatus::Canceled
                        },
                    )
                }),
            "create" => request
                .task
                .as_ref()
                .ok_or_else(|| "task create requires fields".to_string())
                .and_then(|task| {
                    let placement = request.placement.clone().unwrap_or_default();
                    workspace.create_task(&document_id, &request.revision, task, &placement)
                }),
            "update" => request
                .locator
                .as_ref()
                .zip(request.task.as_ref())
                .ok_or_else(|| "task update requires locator and fields".to_string())
                .and_then(|(locator, task)| {
                    workspace.update_task_fields(
                        &document_id,
                        locator,
                        &request.revision,
                        task,
                        request.placement.as_ref(),
                    )
                }),
            _ => return (StatusCode::NOT_FOUND, "unknown task action").into_response(),
        };
        if let Err(error) = result {
            eprintln!(
                "plumb site serve: task {action} rejected for {document_id} ({:?}): {error}",
                request.locator
            );
            return (StatusCode::CONFLICT, error).into_response();
        }
        (
            workspace
                .document_path(&document_id)
                .expect("validated task document id")
                .to_path_buf(),
            workspace.revision() + 1,
        )
    };
    {
        let mut workspace = state.workspace.write().await;
        if let Err(error) = workspace.refresh_document(path, revision) {
            return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response();
        }
    }
    state.html_cache.lock().await.clear();
    let _ = state.changes.send(revision);
    eprintln!(
        "plumb site serve: task {action} completed for {document_id} ({:?})",
        request.locator
    );
    (
        StatusCode::NO_CONTENT,
        [("x-plumb-revision", revision.to_string())],
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventActionRequest {
    revision: String,
    locator: Option<WebEventLocator>,
    event: Option<WebEventInput>,
}

async fn update_event(
    State(state): State<AppState>,
    AxumPath((document_id, action)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<EventActionRequest>,
) -> Response {
    if !state.allow_mutations {
        return (
            StatusCode::FORBIDDEN,
            "event mutations are disabled for non-loopback servers",
        )
            .into_response();
    }
    if !same_origin(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin event mutations are forbidden",
        )
            .into_response();
    }
    let result = {
        let workspace = state.workspace.read().await;
        match action.as_str() {
            "create" => request
                .event
                .as_ref()
                .ok_or_else(|| "create requires event fields".to_string())
                .and_then(|event| workspace.create_event(&document_id, &request.revision, event)),
            "update" => request
                .locator
                .as_ref()
                .zip(request.event.as_ref())
                .ok_or_else(|| "update requires locator and event fields".to_string())
                .and_then(|(locator, event)| {
                    workspace.update_event(&document_id, locator, &request.revision, event)
                }),
            "delete" => request
                .locator
                .as_ref()
                .ok_or_else(|| "delete requires an event locator".to_string())
                .and_then(|locator| {
                    workspace.delete_event(&document_id, locator, &request.revision)
                }),
            _ => return (StatusCode::NOT_FOUND, "unknown event action").into_response(),
        }
    };
    if let Err(error) = result {
        return (StatusCode::CONFLICT, error).into_response();
    }
    let (root, revision) = {
        let workspace = state.workspace.read().await;
        (workspace.root().to_path_buf(), workspace.revision() + 1)
    };
    let refreshed = match WebWorkspace::load_with_revision(root, revision) {
        Ok(workspace) => workspace,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };
    let events = refreshed.events();
    *state.workspace.write().await = refreshed;
    state.html_cache.lock().await.clear();
    let _ = state.changes.send(revision);
    Json(events).into_response()
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
    let html = tokio::task::spawn_blocking(move || render_note_html(&workspace, &id))
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

async fn agenda_state_js() -> Response {
    asset("application/javascript; charset=utf-8", AGENDA_STATE_JS)
}

async fn query_state_js() -> Response {
    asset("application/javascript; charset=utf-8", QUERY_STATE_JS)
}

async fn task_ui_js() -> Response {
    asset("application/javascript; charset=utf-8", TASK_UI_JS)
}

async fn revision_state_js() -> Response {
    asset("application/javascript; charset=utf-8", REVISION_STATE_JS)
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
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn spawn_watcher(state: AppState) {
    tokio::spawn(async move {
        let root = state.workspace.read().await.root().to_path_buf();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut watcher = match notify::recommended_watcher(move |event| match event {
            Ok(event) if watch_event_affects_workspace(&event) => {
                let _ = sender.send(event.paths);
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
        while let Some(mut changed_paths) = receiver.recv().await {
            tokio::time::sleep(Duration::from_millis(180)).await;
            while let Ok(paths) = receiver.try_recv() {
                changed_paths.extend(paths);
            }
            changed_paths.sort();
            changed_paths.dedup();
            let already_current = if changed_paths.is_empty()
                || changed_paths.iter().any(|path| {
                    path.extension()
                        .is_none_or(|extension| extension != "plumb")
                }) {
                false
            } else {
                let workspace = state.workspace.read().await;
                changed_paths
                    .iter()
                    .all(|path| workspace.document_source_matches_disk(path))
            };
            if already_current {
                continue;
            }
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
        std::fs::write(
            root.join("tasks.plumb"),
            "`task Ready task {\n  `@ ready\n}\n",
        )
        .unwrap();
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
        for path in [
            "/graph?preset=connected",
            "/tasks?preset=ready",
            "/agenda",
            "/agenda-state.js",
            "/task-ui.js",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }

        let response = app
            .clone()
            .oneshot(Request::get("/api/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["events"].as_array().unwrap().len(), 0);
        assert_eq!(value["documents"].as_array().unwrap().len(), 1);

        let response = app.clone().oneshot(
            Request::post("/api/query")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"view":"tasks","presets":["ready"],"query":"Ready","sort":["source"],"traversal":{}}"#))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["view"], "tasks");
        assert_eq!(value["tasks"]["tasks"].as_array().unwrap().len(), 1);

        let response = app
            .clone()
            .oneshot(
                Request::get("/api/task-candidates")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["tasks"].as_array().unwrap().len(), 1);

        let response = app
            .clone()
            .oneshot(
                Request::get("/api/task-candidates?query=missing&limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value["tasks"].as_array().unwrap().is_empty());

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
            .clone()
            .oneshot(Request::get("/api/tasks").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let tasks: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let document = &tasks["documents"][0];
        let request = json!({
            "revision": document["revision"],
            "task": {
                "title": "Created from API", "created": null, "due": null,
                "wait": null, "recur": null, "prev": null, "depends": [], "priority": 3
            },
            "placement": { "parent": null, "after": null }
        });
        let response = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/api/task/{}/create",
                    document["id"].as_str().unwrap()
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::HOST, "127.0.0.1:4242")
                .header(header::ORIGIN, "http://127.0.0.1:4242")
                .body(Body::from(request.to_string()))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["x-plumb-revision"], "2");
        assert!(to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
        let source = std::fs::read_to_string(root.join("tasks.plumb")).unwrap();
        assert!(source.contains("Created from API"));
        assert!(source.contains("`: priority 3"), "{source}");

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
