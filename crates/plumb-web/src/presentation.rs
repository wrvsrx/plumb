use std::path::Path;

use crate::{SourceLocation, WebWorkspace};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const NOTE_HTML: &str = include_str!("../assets/note.html");
pub(crate) const APP_JS: &str = include_str!("../assets/app.js");
pub(crate) const QUERY_STATE_JS: &str = include_str!("../assets/query-state.js");
pub(crate) const STYLES_CSS: &str = include_str!("../assets/styles.css");
pub(crate) const FORCE_GRAPH_JS: &str = include_str!("../assets/vendor/force-graph.min.js");
pub(crate) const FORCE_GRAPH_LICENSE: &str =
    include_str!("../assets/vendor/FORCE-GRAPH-LICENSE.txt");
pub(crate) const CEL_JS: &str = include_str!("../assets/vendor/cel-js.min.js");
pub(crate) const CEL_JS_LICENSE: &str = include_str!("../assets/vendor/CEL-JS-LICENSE.txt");

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

pub(crate) fn write_assets(output: &Path) -> Result<(), String> {
    std::fs::create_dir_all(output.join("vendor"))
        .map_err(|error| format!("cannot create assets directory: {error}"))?;
    for (path, contents) in [
        ("app.js", APP_JS),
        ("query-state.js", QUERY_STATE_JS),
        ("styles.css", STYLES_CSS),
        ("vendor/force-graph.min.js", FORCE_GRAPH_JS),
        ("vendor/FORCE-GRAPH-LICENSE.txt", FORCE_GRAPH_LICENSE),
        ("vendor/cel-js.min.js", CEL_JS),
        ("vendor/CEL-JS-LICENSE.txt", CEL_JS_LICENSE),
    ] {
        std::fs::write(output.join(path), contents)
            .map_err(|error| format!("cannot write {path}: {error}"))?;
    }
    Ok(())
}
