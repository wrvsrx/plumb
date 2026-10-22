use super::{display_path, line_column, TagConfig};
use plumb_workspace::{DiskWorkspace, TagMatch};
use std::path::Path;

pub(super) fn run(root: &Path, loaded: &DiskWorkspace, options: &TagConfig) -> Result<(), String> {
    if let Some(document) = &options.document {
        let path = plumb_workspace::normalize(&root.join(document));
        if !path.starts_with(root) || loaded.source(&path).is_none() {
            return Err("tag document must be an indexed document under the workspace root".into());
        }
        let scope = options
            .scope
            .as_deref()
            .map(|scope| {
                let (start, end) = scope.split_once(':').ok_or("scope must be START:END")?;
                let start = start.parse::<usize>().map_err(|_| "invalid scope start")?;
                let end = end.parse::<usize>().map_err(|_| "invalid scope end")?;
                let source = loaded.source(&path).unwrap();
                if start > end || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
                    return Err("scope must be an ordered UTF-8 byte range within the document");
                }
                Ok(start..end)
            })
            .transpose()?;
        let result = loaded
            .workspace
            .document_tag_owners(&path, scope)
            .map_err(|e| e.to_string())?;
        print_matches(
            root,
            loaded,
            &result.value,
            result.is_complete(),
            false,
            options.json,
        )?;
    } else if let Some(tag) = &options.label {
        let result = loaded
            .workspace
            .query_tag(tag, options.limit as usize, None)
            .map_err(|e| e.to_string())?;
        print_matches(
            root,
            loaded,
            &result.value.items,
            result.is_complete(),
            result.value.next_cursor.is_some(),
            options.json,
        )?;
    } else {
        let result = loaded
            .workspace
            .query_tag_values(
                options.prefix.as_deref().unwrap_or(""),
                options.limit as usize,
            )
            .map_err(|e| e.to_string())?;
        if options.json {
            println!(
                "{}",
                serde_json::json!({"values": result.value.values, "complete": result.is_complete(), "has_more": result.value.has_more})
            );
        } else {
            for tag in &result.value.values {
                println!("{tag}");
            }
            if result.value.has_more {
                eprintln!(
                    "More tags are available; narrow --prefix or raise --limit (maximum 1000)."
                );
            }
        }
    }
    Ok(())
}
fn print_matches(
    root: &Path,
    loaded: &DiskWorkspace,
    items: &[TagMatch],
    complete: bool,
    has_more: bool,
    json: bool,
) -> Result<(), String> {
    if json {
        let items = items
            .iter()
            .map(|item| {
                let mut value = serde_json::to_value(item).expect("serializable tag match");
                value["path"] = display_path(root, &item.path).into();
                value
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::json!({"items": items, "complete": complete, "has_more": has_more})
        );
    } else {
        for item in items {
            let source = loaded
                .source(&item.path)
                .ok_or("tag source is unavailable")?;
            let (line, column) = line_column(source, item.record.selection_range.start);
            println!(
                "{}:{line}:{column}\t{}\t{}",
                display_path(root, &item.path),
                item.record.title,
                item.record.tags.values.join(", ")
            );
        }
        if has_more {
            eprintln!("More owners match; raise --limit (maximum 1000) or inspect --document.");
        }
    }
    Ok(())
}
