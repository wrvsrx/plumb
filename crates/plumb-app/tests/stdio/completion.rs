use serde_json::json;

use crate::support::{response, run_server, unique_temp_dir};

#[test]
fn completes_task_dependencies_from_workspace_tasks() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("current.plumb");
    let target_path = root.join("Project Plan.plumb");
    let source = "`task Existing dependency {\n  `@ done\n}\n`task Local task {\n  `@ local\n}\n\n`node Plain anchor {\n  `@ plain\n}\n\n`task Review {\n  `@ review\n  `: depends #done Project Plan.plumb#dr\n}\n`task Review two {\n  `@ review-two\n  `: depends #done \n}\n";
    let target = "`task Draft task {\n  `@ draft\n}\n`task Closed task {\n  `@ closed\n  `: done 2026-08-04T12:00:00+08:00\n}\n\n`node Not a task {\n  `@ note\n}\n";
    std::fs::write(&source_path, source).unwrap();
    std::fs::write(&target_path, target).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source_path).unwrap();
    let lines = source.lines().collect::<Vec<_>>();
    let path_line = lines.iter().position(|line| line.contains("#dr")).unwrap();
    let empty_line = lines
        .iter()
        .position(|line| line.contains("#done "))
        .unwrap();
    let path_cursor = lines[path_line].find("#dr").unwrap() + "#dr".len();
    let empty_cursor = lines[empty_line].find("#done ").unwrap() + "#done ".len();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "completion": {
                    "completionItem": { "snippetSupport": true }
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": path_line, "character": path_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": empty_line, "character": empty_cursor }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let path_items = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(path_items.len(), 1);
    assert_eq!(path_items[0]["label"], "draft");
    assert_eq!(path_items[0]["kind"], 18);
    assert!(path_items[0]["detail"]
        .as_str()
        .unwrap()
        .contains("READY  Draft task"));
    assert_eq!(path_items[0]["textEdit"]["newText"], "draft");
    assert_eq!(
        path_items[0]["textEdit"]["range"],
        json!({
            "start": {
                "line": path_line,
                "character": lines[path_line].find("#dr").unwrap() + 1
            },
            "end": {
                "line": path_line,
                "character": lines[path_line].find("#dr").unwrap() + "#dr".len()
            }
        })
    );

    let all_items = response(&output, 3)["result"].as_array().unwrap();
    let labels = all_items
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(labels, ["#", "Project Plan.plumb#"]);
    assert!(all_items.iter().all(|item| item["kind"] == 17));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_links_by_document_metadata_title() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("current.plumb");
    let target = root.join("Usage Guide.plumb");
    let closed_path = "`->[x]{`:[to usXXX]}\n";
    let closed_anchor = "`->[x]{`:[to Usage Guide.plumb#usXXX]}\n";
    let raw = "`\"[raw `->[x]{to=\"us\"}]\"";
    let source_text =
        format!("`->[Us\n\n`->[x]{{`:[to Guide\n\n{closed_path}\n{closed_anchor}\n{raw}\n");
    std::fs::write(&source, &source_text).unwrap();
    std::fs::write(
        &target,
        "{\n  `: title Usage Guide\n}\n\n`# Usage {\n  `@ usage\n}\n",
    )
    .unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": 8 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 2, "character": 18 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": {
                    "line": 4,
                    "character": closed_path.find("usXXX").unwrap() + 2
                }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": {
                    "line": 6,
                    "character": closed_anchor.find("usXXX").unwrap() + 2
                }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": {
                    "line": 8,
                    "character": raw.find("us").unwrap() + 2
                }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let label = &response(&output, 2)["result"][0];
    assert_eq!(label["label"], "Usage Guide");
    assert_eq!(label["detail"], "Usage Guide.plumb");
    assert_eq!(
        label["textEdit"]["newText"],
        "`->[Usage Guide]{`:[to Usage Guide.plumb]}"
    );
    let path = &response(&output, 3)["result"][0];
    assert_eq!(path["label"], "Usage Guide.plumb");
    assert_eq!(path["detail"], "Usage Guide");
    assert_eq!(path["textEdit"]["newText"], "Usage Guide.plumb");
    let closed_path_item = &response(&output, 4)["result"][0];
    assert_eq!(closed_path_item["textEdit"]["newText"], "Usage Guide.plumb");
    assert_eq!(
        closed_path_item["textEdit"]["range"],
        json!({
            "start": { "line": 4, "character": closed_path.find("usXXX").unwrap() },
            "end": { "line": 4, "character": closed_path.find("usXXX").unwrap() + 5 }
        })
    );
    let closed_anchor_item = &response(&output, 5)["result"][0];
    assert_eq!(closed_anchor_item["textEdit"]["newText"], "usage");
    assert_eq!(
        closed_anchor_item["textEdit"]["range"],
        json!({
            "start": { "line": 6, "character": closed_anchor.find("usXXX").unwrap() },
            "end": { "line": 6, "character": closed_anchor.find("usXXX").unwrap() + 5 }
        })
    );
    assert!(response(&output, 6)["result"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_from_a_subdirectory_inserts_a_relative_path() {
    let root = unique_temp_dir();
    let source_dir = root.join("b");
    let target_dir = root.join("a");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    let source = source_dir.join("current.plumb");
    let target = target_dir.join("target.plumb");
    let source_text = "`->[Target";
    std::fs::write(&source, source_text).unwrap();
    std::fs::write(
        &target,
        "{\n  `: title Target A\n}\n\n`# Target {\n  `@ target\n}\n",
    )
    .unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source_text
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": source_text.len() }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let item = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "Target A")
        .expect("Target A completion");
    assert_eq!(item["detail"], "../a/target.plumb");
    assert_eq!(
        item["textEdit"]["newText"],
        "`->[Target A]{`:[to ../a/target.plumb]}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_and_navigates_relative_autolinks_files_and_images() {
    let root = unique_temp_dir();
    let static_dir = root.join("static");
    std::fs::create_dir_all(&static_dir).unwrap();
    let current = root.join("current.plumb");
    let target = root.join("target note.plumb");
    let unicode_target = root.join("中文笔记 [草稿].plumb");
    let image = static_dir.join("image one.PNG");
    let attachment = static_dir.join("manual draft.pdf");
    let source = "`->\"tar\"\n`->\"target note.plumb#an\"\n`img[Query]{`:[src static/im]}\n`img[Missing]{`:[src static/missing.png]}\n`->\"target note.plumb\"\n`img[Result]{`:[src static/image one.PNG]}\n`->\"中文\"\n`->\"static/manual draft.pdf\"\n`->[manual]{`:[to static/manual draft.pdf]}\n`->\"static/missing guide.pdf\"\n";
    std::fs::write(&current, source).unwrap();
    std::fs::write(
        &target,
        "{\n  `: title Target note\n}\n\n`# Anchor {\n  `@ anchor\n}\n",
    )
    .unwrap();
    std::fs::write(&unicode_target, "`# 中文笔记\n").unwrap();
    std::fs::write(&image, b"png").unwrap();
    std::fs::write(&attachment, b"pdf").unwrap();

    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let current_uri = lsp_types::Url::from_file_path(&current).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let image_uri = lsp_types::Url::from_file_path(&image).unwrap();
    let attachment_uri = lsp_types::Url::from_file_path(&attachment).unwrap();
    let lines = source.lines().collect::<Vec<_>>();
    let autolink_path_cursor = lines[0].find("tar").unwrap() + "tar".len();
    let autolink_anchor_cursor = lines[1].find("#an").unwrap() + "#an".len();
    let image_query_cursor = lines[2].find("static/im").unwrap() + "static/im".len();
    let autolink_definition = lines[4].find("target note.plumb").unwrap() + 2;
    let image_definition = lines[5].find("static/image").unwrap() + 2;
    let unicode_cursor = lines[6][..lines[6].find("中文").unwrap() + "中文".len()]
        .encode_utf16()
        .count();
    let attachment_autolink = lines[7].find("manual draft").unwrap() + 2;
    let attachment_link = lines[8].find("manual draft").unwrap() + 2;
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": current_uri, "languageId": "plumb", "version": 4, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 0, "character": autolink_path_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 1, "character": autolink_anchor_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 2, "character": image_query_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 5, "character": image_definition }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 5, "character": image_definition }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 4, "character": autolink_definition }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 8, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 6, "character": unicode_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 9, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 7, "character": attachment_autolink }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 10, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 7, "character": attachment_autolink }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 11, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 8, "character": attachment_link }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 12, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let autolink_path = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "target note.plumb")
        .expect("autolink document path completion");
    assert_eq!(autolink_path["detail"], "Target note");
    assert_eq!(autolink_path["textEdit"]["newText"], "target note.plumb");

    let anchor = response(&output, 3)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "#anchor")
        .expect("raw anchor completion");
    assert_eq!(anchor["textEdit"]["newText"], "anchor");

    let image_completion = response(&output, 4)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "static/image one.PNG")
        .expect("image path completion");
    assert_eq!(image_completion["kind"], 17);
    assert_eq!(
        image_completion["textEdit"]["newText"],
        "static/image one.PNG"
    );

    assert!(response(&output, 5)["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("Image file"));
    assert_eq!(response(&output, 6)["result"]["uri"], image_uri.as_str());
    assert_eq!(response(&output, 7)["result"]["uri"], target_uri.as_str());

    let unicode_completion = response(&output, 8)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "中文笔记 [草稿].plumb")
        .expect("Unicode autolink completion");
    assert_eq!(
        unicode_completion["textEdit"]["newText"],
        "中文笔记 [草稿].plumb"
    );
    assert_eq!(
        unicode_completion["textEdit"]["range"],
        json!({
            "start": { "line": 6, "character": 4 },
            "end": { "line": 6, "character": 6 }
        })
    );
    assert!(response(&output, 9)["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("File"));
    assert_eq!(
        response(&output, 10)["result"]["uri"],
        attachment_uri.as_str()
    );
    assert_eq!(
        response(&output, 11)["result"]["uri"],
        attachment_uri.as_str()
    );

    let diagnostics = output
        .iter()
        .rfind(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == current_uri.as_str()
        })
        .expect("current diagnostics");
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "image.unresolved-file"));
    assert!(diagnostics["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["code"] == "link.unresolved-file"));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_block_constructs_from_their_marker_prefixes() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("constructs.plumb");
    let source = "`t\n  `ev\n`-\nText `";
    std::fs::write(&document, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "completion": {
                    "completionItem": { "snippetSupport": true }
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 0, "character": 2 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 1, "character": 5 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 2, "character": 2 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 3, "character": 6 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 6, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let trigger_characters = response(&output, 1)["result"]["capabilities"]["completionProvider"]
        ["triggerCharacters"]
        .as_array()
        .unwrap();
    assert!(trigger_characters.iter().any(|character| character == "t"));
    assert!(trigger_characters.iter().any(|character| character == "e"));

    let task_items = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(task_items.len(), 1);
    assert_eq!(task_items[0]["label"], "Task");
    assert_eq!(
        task_items[0]["textEdit"]["range"],
        json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 2 } })
    );
    let task = task_items[0]["textEdit"]["newText"].as_str().unwrap();
    assert!(task.starts_with("`task ${1:Task} {\n  `: created "));
    assert!(task.ends_with("\n}"));
    assert_eq!(task_items[0]["insertTextFormat"], 2);

    let event_items = response(&output, 3)["result"].as_array().unwrap();
    assert_eq!(event_items.len(), 1);
    assert_eq!(event_items[0]["label"], "Event");
    let event = &event_items[0];
    assert_eq!(event["textEdit"]["newText"], "`event ${1:09:00} ${2:Event}");
    assert_eq!(event["insertTextFormat"], 2);

    let link_items = response(&output, 4)["result"].as_array().unwrap();
    assert_eq!(link_items.len(), 2);
    assert_eq!(link_items[0]["label"], "Link");
    assert_eq!(link_items[1]["label"], "Autolink");
    assert!(response(&output, 5)["result"].is_null());

    let fallback_messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {}
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 0, "character": 2 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let fallback_output = run_server(&fallback_messages);
    let fallback_items = response(&fallback_output, 2)["result"].as_array().unwrap();
    assert_eq!(fallback_items.len(), 1);
    assert_eq!(fallback_items[0]["label"], "Task");
    let fallback_task = fallback_items[0]["textEdit"]["newText"].as_str().unwrap();
    assert!(fallback_task.starts_with("`task  {\n  `: created "));
    assert!(fallback_task.ends_with("\n}"));
    assert_eq!(fallback_items[0]["insertTextFormat"], 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_task_construct_immediately_after_attached_group() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("current.plumb");
    let source = "`task something {\n `: created 2026-08-09T10:55:24+08:00\n}\n`t";
    std::fs::write(&document, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "completion": {
                    "completionItem": { "snippetSupport": true }
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 3, "character": 2 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let item = &response(&output, 2)["result"][0];
    assert_eq!(item["label"], "Task");
    assert_eq!(
        item["textEdit"]["range"],
        json!({
            "start": { "line": 3, "character": 0 },
            "end": { "line": 3, "character": 2 }
        })
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn narrows_link_constructs_from_the_shared_marker_prefix() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("construct-prefixes.plumb");
    let source = "Text `[\nText `-\nText `->\nText `->[\nText `->\"\n";
    std::fs::write(&document, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "completion": {
                    "completionItem": { "snippetSupport": true }
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 0, "character": 7 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 1, "character": 7 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 2, "character": 8 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 3, "character": 9 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 4, "character": 9 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert!(response(&output, 2)["result"].is_null());

    for id in [3, 4] {
        let items = response(&output, id)["result"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["label"], "Link");
        assert_eq!(items[1]["label"], "Autolink");
        assert_eq!(
            items[0]["textEdit"]["newText"],
            "`->[${1:label}]{`:[to ${2:target}]}"
        );
        assert_eq!(items[1]["textEdit"]["newText"], "`->\"${1:path}\"");
    }

    let link = response(&output, 5)["result"].as_array().unwrap();
    assert_eq!(link.len(), 1);
    assert_eq!(link[0]["label"], "Link");
    assert_eq!(
        link[0]["textEdit"]["range"],
        json!({ "start": { "line": 3, "character": 5 }, "end": { "line": 3, "character": 9 } })
    );

    let autolink = response(&output, 6)["result"].as_array().unwrap();
    assert_eq!(autolink.len(), 1);
    assert_eq!(autolink[0]["label"], "Autolink");
    assert_eq!(autolink[0]["textEdit"]["newText"], "`->\"${1:path}\"");
    assert_eq!(
        autolink[0]["textEdit"]["range"],
        json!({ "start": { "line": 4, "character": 5 }, "end": { "line": 4, "character": 9 } })
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_attributes_with_protocol_ranges_and_snippets() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("attributes.plumb");
    let source =
        "`task Work {\n  `: created now\n  `: pr\n}\n`img[Alt]{`: s}\n`$\"x\"{`:[language t]}\n";
    std::fs::write(&document, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "completion": {
                    "completionItem": { "snippetSupport": true }
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": document_uri },
                "position": { "line": 2, "character": 7 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": document_uri },
                "position": { "line": 4, "character": 14 } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": document_uri },
                "position": { "line": 5, "character": 19 } }
        }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server(&messages);
    let priority = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "priority")
        .unwrap();
    assert_eq!(priority["textEdit"]["newText"], "`: priority ${1:0}");
    assert_eq!(priority["textEdit"]["range"]["start"]["character"], 2);
    assert_eq!(priority["insertTextFormat"], 2);
    let image = &response(&output, 3)["result"][0];
    assert_eq!(image["label"], "src");
    assert_eq!(image["textEdit"]["newText"], "`:[src ${1}]");
    let language = &response(&output, 4)["result"][0];
    assert_eq!(language["label"], "tex");
    assert_eq!(language["textEdit"]["newText"], "tex");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_recursive_attached_elements() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("attached-completion.plumb");
    let source = "`task Work {\n  `: pr\n}\n`->[x]{`: t}\n";
    std::fs::write(&document, source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let document_uri = lsp_types::Url::from_file_path(&document).unwrap();
    let messages = [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": null, "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": { "textDocument": { "completion": {
                    "completionItem": { "snippetSupport": true }
                } } }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": document_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 1, "character": 7 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 3, "character": 11 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let properties = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(properties.len(), 2);
    let priority = properties
        .iter()
        .find(|item| item["label"] == "priority")
        .unwrap();
    assert_eq!(priority["textEdit"]["newText"], "`: priority ${1:0}");
    let inline = response(&output, 3)["result"].as_array().unwrap();
    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0]["label"], "to");
    assert_eq!(inline[0]["textEdit"]["newText"], "`:[to ${1}]");
    std::fs::remove_dir_all(root).unwrap();
}
