use crate::{SourceLocation, WebWorkspace};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const NOTE_HTML: &str = include_str!("../assets/note.html");
pub(crate) const APP_JS: &str = include_str!("../assets/app.js");
pub(crate) const AGENDA_STATE_JS: &str = include_str!("../assets/agenda-state.js");
pub(crate) const QUERY_STATE_JS: &str = include_str!("../assets/query-state.js");
pub(crate) const TASK_UI_JS: &str = include_str!("../assets/task-ui.js");
pub(crate) const REVISION_STATE_JS: &str = include_str!("../assets/revision-state.js");
pub(crate) const STYLES_CSS: &str = include_str!("../assets/styles.css");
pub(crate) const FORCE_GRAPH_JS: &str = include_str!("../assets/vendor/force-graph.min.js");
pub(crate) const FORCE_GRAPH_LICENSE: &str =
    include_str!("../assets/vendor/FORCE-GRAPH-LICENSE.txt");

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
        .replace("__DOCUMENT_ID__", &escape_html(id))
        .replace("__CONTENT__", content)
        .replace("__BACKLINKS__", backlinks)
        .replace("__ASSET_PREFIX__", asset_prefix)
        .replace("__ROOT_PREFIX__", root_prefix)
}

pub(crate) fn render_backlinks(
    workspace: &WebWorkspace,
    locations: &[SourceLocation],
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
            escape_html(&href),
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
        .replace("__PLUMB_CONFIG__", &escape_html(&config.to_string()))
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
