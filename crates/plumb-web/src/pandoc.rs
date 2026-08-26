use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;

use crate::WebWorkspace;

pub fn render_note_html(workspace: &WebWorkspace, document_id: &str) -> Result<String, String> {
    let source_path = workspace
        .document_path(document_id)
        .ok_or_else(|| format!("unknown document id '{document_id}'"))?;
    let mut document = workspace.pandoc_document(document_id)?;
    let bibliography = workspace.bibliography(document_id)?;
    if bibliography.declared && bibliography.sources.is_empty() {
        let message = bibliography
            .diagnostics
            .first()
            .map(|diagnostic| diagnostic.message.as_str())
            .unwrap_or("no valid bibliography source");
        return Err(format!("cannot render citations: {message}"));
    }
    project_bibliography_paths(&mut document, &bibliography.sources);
    adapt_pandoc_targets(workspace, source_path, &mut document);
    let input = serde_json::to_vec(&document)
        .map_err(|error| format!("cannot encode Pandoc document: {error}"))?;
    let mut child = Command::new("pandoc")
        .args(["--from=json", "--to=html5", "--mathml", "--citeproc"])
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

fn project_bibliography_paths(document: &mut Value, sources: &[PathBuf]) {
    let Some(metadata) = document.get_mut("meta").and_then(Value::as_object_mut) else {
        return;
    };
    if !sources.is_empty() {
        metadata.insert(
            "bibliography".to_string(),
            serde_json::json!({
                "t": "MetaList",
                "c": sources.iter().map(|path| serde_json::json!({
                    "t": "MetaString",
                    "c": path.to_string_lossy(),
                })).collect::<Vec<_>>()
            }),
        );
    }
}

pub fn adapt_pandoc_targets(workspace: &WebWorkspace, source_path: &Path, document: &mut Value) {
    adapt_value(workspace, source_path, document);
}

fn adapt_value(workspace: &WebWorkspace, source_path: &Path, value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                adapt_value(workspace, source_path, value);
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
                    let (adapted, document_id) = if node_kind.as_deref() == Some("Image") {
                        (adapt_resource_target(workspace, source_path, &target), None)
                    } else {
                        adapt_link_target(workspace, source_path, &target)
                    };
                    if node_kind.as_deref() == Some("Link")
                        && is_file_node(object)
                        && is_local_video(workspace, source_path, &target)
                    {
                        let video = video_inline(object, &adapted);
                        *object = video;
                        return;
                    }
                    if let Some(target_value) = object
                        .get_mut("c")
                        .and_then(Value::as_array_mut)
                        .and_then(|contents| contents.get_mut(2))
                        .and_then(Value::as_array_mut)
                        .and_then(|target| target.get_mut(0))
                    {
                        *target_value = Value::String(adapted);
                    }
                    if let Some(document_id) = document_id {
                        add_link_document_attribute(object, &document_id);
                    }
                }
            }
            for child in object.values_mut() {
                adapt_value(workspace, source_path, child);
            }
        }
        _ => {}
    }
}

fn is_file_node(object: &serde_json::Map<String, Value>) -> bool {
    object
        .get("c")
        .and_then(Value::as_array)
        .and_then(|contents| contents.first())
        .and_then(Value::as_array)
        .and_then(|attrs| attrs.get(2))
        .and_then(Value::as_array)
        .is_some_and(|pairs| {
            pairs.iter().any(|pair| {
                pair.as_array().is_some_and(|pair| {
                    pair.first().and_then(Value::as_str) == Some("data-plumb-marker")
                        && pair.get(1).and_then(Value::as_str) == Some("file")
                })
            })
        })
}

fn is_local_video(workspace: &WebWorkspace, source_path: &Path, target: &str) -> bool {
    if is_external(target) || target.contains('#') {
        return false;
    }
    let resolved = resolve_relative(source_path, target);
    let canonical = resolved.canonicalize().unwrap_or(resolved);
    workspace
        .resource_for_path(&canonical)
        .is_some_and(|resource| {
            mime_guess::from_path(&resource.path)
                .first()
                .is_some_and(|mime| mime.type_() == mime_guess::mime::VIDEO)
        })
}

fn video_inline(
    link: &serde_json::Map<String, Value>,
    target: &str,
) -> serde_json::Map<String, Value> {
    let escaped_target = escape_html_attribute(target);
    let contents = link.get("c").and_then(Value::as_array);
    let mut attrs = contents
        .and_then(|contents| contents.first())
        .cloned()
        .unwrap_or_else(|| serde_json::json!(["", [], []]));
    remove_file_marker(&mut attrs);
    let label = contents
        .and_then(|contents| contents.get(1))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let fallback = serde_json::json!({
        "t": "Link",
        "c": [["", [], []], label, [target, ""]],
    });
    serde_json::Map::from_iter([
        ("t".to_string(), Value::String("Span".to_string())),
        (
            "c".to_string(),
            Value::Array(vec![
                attrs,
                Value::Array(vec![
                    serde_json::json!({
                        "t": "RawInline",
                        "c": ["html", format!("<video controls preload=\"metadata\" src=\"{escaped_target}\">")],
                    }),
                    fallback,
                    serde_json::json!({"t": "RawInline", "c": ["html", "</video>"]}),
                ]),
            ]),
        ),
    ])
}

fn remove_file_marker(attrs: &mut Value) {
    let Some(pairs) = attrs
        .as_array_mut()
        .and_then(|attrs| attrs.get_mut(2))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    pairs.retain(|pair| {
        !pair.as_array().is_some_and(|pair| {
            pair.first().and_then(Value::as_str) == Some("data-plumb-marker")
                && pair.get(1).and_then(Value::as_str) == Some("file")
        })
    });
}

fn escape_html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn adapt_link_target(
    workspace: &WebWorkspace,
    source_path: &Path,
    target: &str,
) -> (String, Option<String>) {
    if is_external(target) {
        return (target.to_string(), None);
    }
    if let Some(fragment) = target.strip_prefix('#') {
        return (format!("#{}", encode_fragment(fragment)), None);
    }
    let (path, fragment) = target
        .split_once('#')
        .map_or((target, None), |(path, fragment)| (path, Some(fragment)));
    let resolved = resolve_relative(source_path, path);
    if path.ends_with(".plumb") {
        if let Some(id) = workspace.document_id(&resolved) {
            return (document_url(id, fragment), Some(id.to_string()));
        }
        return (target.to_string(), None);
    }
    (adapt_resource_target(workspace, source_path, target), None)
}

fn add_link_document_attribute(object: &mut serde_json::Map<String, Value>, document_id: &str) {
    let Some(attributes) = object
        .get_mut("c")
        .and_then(Value::as_array_mut)
        .and_then(|contents| contents.first_mut())
        .and_then(Value::as_array_mut)
        .and_then(|attributes| attributes.get_mut(2))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    attributes.push(Value::Array(vec![
        Value::String("data-plumb-document".to_string()),
        Value::String(document_id.to_string()),
    ]));
}

fn adapt_resource_target(workspace: &WebWorkspace, source_path: &Path, target: &str) -> String {
    if is_external(target) {
        return target.to_string();
    }
    let resolved = resolve_relative(source_path, target);
    let canonical = resolved.canonicalize().unwrap_or(resolved);
    let Some(resource) = workspace.resource_for_path(&canonical) else {
        return target.to_string();
    };
    let name = utf8_percent_encode(&resource.name, NON_ALPHANUMERIC).to_string();
    format!("/resource/{}/{}", resource.id, name)
}

fn document_url(id: &str, fragment: Option<&str>) -> String {
    let base = format!("/note/{id}");
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
            "`->[B|b.plumb#section]\n\n`img[x|=[src|assets/a b.png]]\n",
        )
        .unwrap();
        std::fs::write(root.join("b.plumb"), "`# B\n  `@ section\n").unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let mut document = workspace
            .pandoc_document(workspace.document_id(root.join("a.plumb")).unwrap())
            .unwrap();
        adapt_pandoc_targets(&workspace, &root.join("a.plumb"), &mut document);
        let link = &document["blocks"][0]["c"][0];
        assert!(link["c"][2][0].as_str().unwrap().starts_with("/note/d"));
        assert!(link["c"][2][0].as_str().unwrap().ends_with("#section"));
        let attributes = link["c"][0][2].as_array().unwrap();
        let document_attribute = attributes
            .iter()
            .find(|attribute| attribute[0] == "data-plumb-document")
            .unwrap();
        assert_eq!(
            document_attribute[1].as_str(),
            workspace.document_id(root.join("b.plumb"))
        );
        let image = &document["blocks"][1]["c"][0];
        let image_target = image["c"][2][0].as_str().unwrap();
        assert!(image_target.starts_with("/resource/r"), "{image_target}");
        assert!(image_target.ends_with("/a%20b%2Epng"), "{image_target}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renders_local_video_files_as_media() {
        let root = temp_dir();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/demo video.mp4"), b"video").unwrap();
        std::fs::write(root.join("assets/manual.pdf"), b"pdf").unwrap();
        std::fs::write(
            root.join("a.plumb"),
            "`file[Demo video|=[src|assets/demo video.mp4]]\n\n`file[Manual|=[src|assets/manual.pdf]]\n\n`->[Video link|assets/demo video.mp4]\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let document_id = workspace.document_id(root.join("a.plumb")).unwrap();

        let mut document = workspace.pandoc_document(document_id).unwrap();
        adapt_pandoc_targets(&workspace, &root.join("a.plumb"), &mut document);
        let video = &document["blocks"][0]["c"][0];
        assert_eq!(video["t"], "Span");
        let html = video["c"][1][0]["c"][1].as_str().unwrap();
        assert!(html.starts_with("<video controls"), "{html}");
        assert!(html.contains("src=\"/resource/r"), "{html}");
        assert!(html.contains("demo%20video%2Emp4"), "{html}");
        assert_eq!(video["c"][1][1]["c"][1][0]["c"], "Demo");
        let fallback_file = &document["blocks"][1]["c"][0];
        assert_eq!(fallback_file["t"], "Link");
        assert!(fallback_file["c"][2][0]
            .as_str()
            .unwrap()
            .starts_with("/resource/r"));
        assert_eq!(document["blocks"][2]["c"][0]["t"], "Link");

        let rendered = render_note_html(&workspace, document_id).unwrap();
        assert!(rendered.contains("<video controls"), "{rendered}");
        assert!(rendered.contains("src=\"/resource/r"), "{rendered}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renders_citations_from_plain_csl_json_metadata() {
        if Command::new("pandoc").arg("--version").output().is_err() {
            return;
        }
        let root = temp_dir();
        std::fs::create_dir_all(root.join("static")).unwrap();
        std::fs::write(
            root.join("static/library file.json"),
            r#"[{"id":"smith2004","type":"book","title":"Example Book","author":[{"family":"Smith","given":"Alice"}],"issued":{"date-parts":[[2004]]}}]"#,
        )
        .unwrap();
        std::fs::write(
            root.join("note.plumb"),
            "`= bibliography\n `- static/library file.json\n\nSee `cite[smith2004].\n",
        )
        .unwrap();
        let workspace = WebWorkspace::load(&root).unwrap();
        let document_id = workspace.document_id(root.join("note.plumb")).unwrap();
        let rendered = render_note_html(&workspace, document_id).unwrap();
        assert!(rendered.contains("class=\"citation\""), "{rendered}");
        assert!(rendered.contains("Smith"), "{rendered}");
        assert!(rendered.contains("2004"), "{rendered}");
        assert!(rendered.contains("Example Book"), "{rendered}");
        assert!(rendered.contains("id=\"refs\""), "{rendered}");
        assert!(!rendered.contains("[smith2004]"), "{rendered}");
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
