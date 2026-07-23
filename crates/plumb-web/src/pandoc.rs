use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;

use crate::WebWorkspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebTargetMode {
    Dynamic,
    StaticNote,
}

pub fn render_note_html(
    workspace: &WebWorkspace,
    document_id: &str,
    mode: WebTargetMode,
) -> Result<String, String> {
    let source_path = workspace
        .document_path(document_id)
        .ok_or_else(|| format!("unknown document id '{document_id}'"))?;
    let mut document = workspace.pandoc_document(document_id)?;
    adapt_pandoc_targets(workspace, source_path, mode, &mut document);
    let input = serde_json::to_vec(&document)
        .map_err(|error| format!("cannot encode Pandoc document: {error}"))?;
    let mut child = Command::new("pandoc")
        .args(["--from=json", "--to=html5", "--mathml"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start Pandoc HTML writer: {error}"))?;
    child
        .stdin
        .as_mut()
        .expect("Pandoc stdin is piped")
        .write_all(&input)
        .map_err(|error| format!("cannot send document to Pandoc: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot wait for Pandoc: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Pandoc HTML writer failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("Pandoc returned invalid UTF-8: {error}"))
}

pub fn adapt_pandoc_targets(
    workspace: &WebWorkspace,
    source_path: &Path,
    mode: WebTargetMode,
    document: &mut Value,
) {
    adapt_value(workspace, source_path, mode, document);
}

fn adapt_value(
    workspace: &WebWorkspace,
    source_path: &Path,
    mode: WebTargetMode,
    value: &mut Value,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                adapt_value(workspace, source_path, mode, value);
            }
        }
        Value::Object(object) => {
            let node_kind = object.get("t").and_then(Value::as_str).map(str::to_string);
            if matches!(node_kind.as_deref(), Some("Link" | "Image")) {
                if let Some(target) = object
                    .get("c")
                    .and_then(Value::as_array)
                    .and_then(|contents| contents.get(2))
                    .and_then(Value::as_array)
                    .and_then(|target| target.first())
                    .and_then(Value::as_str)
                    .map(str::to_string)
                {
                    let adapted = if node_kind.as_deref() == Some("Image") {
                        adapt_resource_target(workspace, source_path, mode, &target)
                    } else {
                        adapt_link_target(workspace, source_path, mode, &target)
                    };
                    if let Some(target_value) = object
                        .get_mut("c")
                        .and_then(Value::as_array_mut)
                        .and_then(|contents| contents.get_mut(2))
                        .and_then(Value::as_array_mut)
                        .and_then(|target| target.get_mut(0))
                    {
                        *target_value = Value::String(adapted);
                    }
                }
            }
            for child in object.values_mut() {
                adapt_value(workspace, source_path, mode, child);
            }
        }
        _ => {}
    }
}

fn adapt_link_target(
    workspace: &WebWorkspace,
    source_path: &Path,
    mode: WebTargetMode,
    target: &str,
) -> String {
    if is_external(target) {
        return target.to_string();
    }
    if let Some(fragment) = target.strip_prefix('#') {
        return format!("#{}", encode_fragment(fragment));
    }
    let (path, fragment) = target
        .split_once('#')
        .map_or((target, None), |(path, fragment)| (path, Some(fragment)));
    let resolved = resolve_relative(source_path, path);
    if path.ends_with(".plumb") {
        if let Some(id) = workspace.document_id(&resolved) {
            return document_url(mode, id, fragment);
        }
        return target.to_string();
    }
    adapt_resource_target(workspace, source_path, mode, target)
}

fn adapt_resource_target(
    workspace: &WebWorkspace,
    source_path: &Path,
    mode: WebTargetMode,
    target: &str,
) -> String {
    if is_external(target) {
        return target.to_string();
    }
    let resolved = resolve_relative(source_path, target);
    let canonical = resolved.canonicalize().unwrap_or(resolved);
    let Some(resource) = workspace.resource_for_path(&canonical) else {
        return target.to_string();
    };
    let name = utf8_percent_encode(&resource.name, NON_ALPHANUMERIC).to_string();
    match mode {
        WebTargetMode::Dynamic => format!("/resource/{}/{}", resource.id, name),
        WebTargetMode::StaticNote => format!("../../resources/{}/{name}", resource.id),
    }
}

fn document_url(mode: WebTargetMode, id: &str, fragment: Option<&str>) -> String {
    let base = match mode {
        WebTargetMode::Dynamic => format!("/note/{id}"),
        WebTargetMode::StaticNote => format!("../../notes/{id}/"),
    };
    fragment.map_or(base.clone(), |fragment| {
        format!("{base}#{}", encode_fragment(fragment))
    })
}

fn encode_fragment(fragment: &str) -> String {
    utf8_percent_encode(fragment, NON_ALPHANUMERIC).to_string()
}

fn is_external(target: &str) -> bool {
    target.starts_with("//")
        || url::Url::parse(target)
            .ok()
            .is_some_and(|url| !url.scheme().is_empty())
}

fn resolve_relative(from: &Path, target: &str) -> PathBuf {
    let parent = from.parent().unwrap_or_else(|| Path::new(""));
    plumb_workspace::normalize(&parent.join(target))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn adapts_documents_anchors_resources_and_external_targets_before_html() {
        let root = temp_dir();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/a b.png"), b"png").unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`->[B]{to=\"b.plumb#section\"}\n\n`img[x]{src=\"assets/a b.png\"}\n",
        )
        .unwrap();
        std::fs::write(root.join("b.plumb"), "`#{#section} B\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let mut document = workspace
            .pandoc_document(workspace.document_id(root.join("a.plumb")).unwrap())
            .unwrap();
        adapt_pandoc_targets(
            &workspace,
            &root.join("a.plumb"),
            WebTargetMode::Dynamic,
            &mut document,
        );
        let link = &document["blocks"][0]["c"][0];
        assert!(link["c"][2][0].as_str().unwrap().starts_with("/note/d"));
        assert!(link["c"][2][0].as_str().unwrap().ends_with("#section"));
        let image = &document["blocks"][1]["c"][0];
        let image_target = image["c"][2][0].as_str().unwrap();
        assert!(image_target.starts_with("/resource/r"), "{image_target}");
        assert!(image_target.ends_with("/a%20b%2Epng"), "{image_target}");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "plumb-web-pandoc-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
