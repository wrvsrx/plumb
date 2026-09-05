use serde_json::json;

use crate::support::{response, run_server, run_server_after_initial_index, unique_temp_dir};

#[test]
fn completes_link_arguments_with_utf16_ranges_and_preserves_groups() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("current.plumb");
    std::fs::write(root.join("Project 中文.plumb"), "`# Guide\n `@ intro\n").unwrap();
    let inputs = [
        ("😀 `->{|}", "{Project 中文.plumb}", ""),
        ("😀 `->{Pro|}", "{Project 中文.plumb}", "Pro"),
        ("😀 `->{label |}", "Project 中文.plumb", ""),
        (
            "😀 `->{label Project 中|}",
            "Project 中文.plumb",
            "Project 中",
        ),
        ("😀 `->{{Project 中|}}", "Project 中文.plumb", "Project 中"),
        ("😀 `->{{guide page} Pro|}", "Project 中文.plumb", "Pro"),
        ("😀 `->{{Project 中文.plumb#in|}}", "intro", "in"),
    ];
    let source = inputs
        .iter()
        .map(|(input, _, _)| input.replace('|', ""))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&source_path, &source).unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source_path).unwrap();
    let mut messages = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "processId":null,"rootUri":root_uri,"capabilities":{},
            "workspaceFolders":[{"uri":root_uri,"name":"test"}]
        }}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{
            "uri":source_uri,"languageId":"plumb","version":1,"text":source
        }}}),
    ];
    for (line, (input, _, _)) in inputs.iter().enumerate() {
        let character = input[..input.find('|').unwrap()].encode_utf16().count();
        messages.push(
            json!({"jsonrpc":"2.0","id":line+2,"method":"textDocument/completion","params":{
                "textDocument":{"uri":source_uri},"position":{"line":line,"character":character}
            }}),
        );
    }
    messages.push(json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}));
    messages.push(json!({"jsonrpc":"2.0","method":"exit","params":null}));
    let output = run_server_after_initial_index(&messages);
    for (line, (input, expected, query)) in inputs.iter().enumerate() {
        let items = response(&output, (line + 2) as u64)["result"]
            .as_array()
            .unwrap();
        assert_eq!(items.len(), 1, "{input}");
        assert_eq!(items[0]["textEdit"]["newText"], *expected, "{input}");
        let end = input[..input.find('|').unwrap()].encode_utf16().count();
        assert_eq!(
            items[0]["textEdit"]["range"],
            json!({
                "start":{"line":line,"character":end-query.encode_utf16().count()},
                "end":{"line":line,"character":end}
            }),
            "{input}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_citation_constructs_and_csl_json_ids() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(root.join("static")).unwrap();
    let source_path = root.join("note.plumb");
    let bibliography_path = root.join("static/library.json");
    let source = "`= bibliography static/library.json\n\nSee `ci and `cite{smi\n";
    std::fs::write(&source_path, source).unwrap();
    std::fs::write(
        &bibliography_path,
        r#"[{"id":"smith2004","title":"Example Book","author":[{"family":"Smith"}],"issued":{"date-parts":[[2004]]}},{"id":"roe2020","title":"Other"}]"#,
    )
    .unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source_path).unwrap();
    let lines = source.lines().collect::<Vec<_>>();
    let line = lines.len() - 1;
    let construct = lines[line].find("`ci").unwrap() + 3;
    let key = lines[line].len();
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
            "params": { "textDocument": { "uri": source_uri }, "position": { "line": line, "character": construct } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": source_uri }, "position": { "line": line, "character": key } }
        }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let output = run_server_after_initial_index(&messages);
    let construct_items = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(construct_items[0]["label"], "Citation");
    assert_eq!(construct_items[0]["textEdit"]["newText"], "`cite{${1:id}}");
    let key_items = response(&output, 3)["result"].as_array().unwrap();
    assert_eq!(key_items.len(), 1);
    assert_eq!(key_items[0]["label"], "smith2004");
    assert_eq!(key_items[0]["detail"], "Smith · 2004 · Example Book");
    assert_eq!(key_items[0]["textEdit"]["newText"], "smith2004");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_event_titles_by_workspace_frequency() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("current.plumb");
    let history_path = root.join("history.plumb");
    let source = "`- 09:00 re\n\n `+ event\n";
    let cursor = "`- 09:00 re".len();
    std::fs::write(&source_path, source).unwrap();
    std::fs::write(
        &history_path,
        "`- 10:00 relax\n\n `+ event\n\n`- 11:00 research\n\n `+ event\n\n`- 12:00 relax\n\n `+ event\n\n`- 13:00 read\n\n `+ event\n",
    )
    .unwrap();
    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let source_uri = lsp_types::Url::from_file_path(&source_path).unwrap();
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
                "uri": source_uri, "languageId": "plumb", "version": 1, "text": source
            }}
        }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": source_uri },
                "position": { "line": 0, "character": cursor }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server_after_initial_index(&messages);
    let items = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["relax", "read", "research"]
    );
    assert_eq!(items[0]["kind"], 12);
    assert_eq!(items[0]["detail"], "event title, 2 uses");
    assert_eq!(items[0]["textEdit"]["newText"], "relax");
    assert_eq!(
        items[0]["textEdit"]["range"],
        json!({
            "start": { "line": 0, "character": cursor - 2 },
            "end": { "line": 0, "character": cursor }
        })
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_task_dependencies_from_workspace_tasks() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("current.plumb");
    let target_path = root.join("Project Plan.plumb");
    let source = "`- Existing dependency\n\n `+ task\n\n `@ done\n\n`- Local task\n\n `+ task\n\n `@ local\n\n`node Plain anchor\n\n `@ plain\n\n`- Review\n\n `+ task\n\n `@ review\n\n `= depends #done Project Plan.plumb#dr\n\n`- Review two\n\n `+ task\n\n `@ review-two\n\n `= depends #done\n";
    let target = "`- Draft task\n\n `+ task\n\n `@ draft\n\n`- Closed task\n\n `+ task\n\n `@ closed\n\n `= done 2026-08-04T12:00:00+08:00\n\n`node Not a task\n\n `@ note\n";
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

    let output = run_server_after_initial_index(&messages);
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
    let closed_path = "`->{x usXXX}\n";
    let closed_anchor = "`->{x {Usage Guide.plumb#usXXX}}\n";
    let raw = "`\"{raw `->[x]{to=\"us\"}}\"\n";
    let open_path = "`->{x Guide";
    let source_text = format!("`->{{Us\n\n{open_path}\n\n{closed_path}\n{closed_anchor}\n{raw}\n");
    std::fs::write(&source, &source_text).unwrap();
    std::fs::write(&target, "`= title Usage Guide\n\n`# Usage\n\n `@ usage\n").unwrap();
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
                "position": { "line": 2, "character": open_path.len() }
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
    let output = run_server_after_initial_index(&messages);
    let label = &response(&output, 2)["result"][0];
    assert_eq!(label["label"], "Usage Guide.plumb");
    assert_eq!(label["detail"], "Usage Guide");
    assert_eq!(label["textEdit"]["newText"], "{Usage Guide.plumb}");
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
    let source_text = "`->{Target";
    std::fs::write(&source, source_text).unwrap();
    std::fs::write(&target, "`= title Target A\n\n`# Target\n\n `@ target\n").unwrap();
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

    let output = run_server_after_initial_index(&messages);
    let item = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "../a/target.plumb")
        .expect("relative target completion");
    assert_eq!(item["detail"], "Target A");
    assert_eq!(item["textEdit"]["newText"], "../a/target.plumb");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_and_navigates_relative_verbatim_links_files_and_images() {
    let root = unique_temp_dir();
    let static_dir = root.join("static");
    std::fs::create_dir_all(&static_dir).unwrap();
    let current = root.join("current.plumb");
    let target = root.join("target note.plumb");
    let unicode_target = root.join("中文笔记 [草稿].plumb");
    let image = static_dir.join("image one.PNG");
    let attachment = static_dir.join("manual draft.pdf");
    let source = "`->\"tar\"\n`->\"target note.plumb#an\"\n`img{Query `={src static/im}}\n`img{Missing `={src static/missing.png}}\n`->\"target note.plumb\"\n`img{Result `={src {static/image one.PNG}}}\n`->\"中文\"\n`->\"static/manual draft.pdf\"\n`->{manual {static/manual draft.pdf}}\n`->\"static/missing guide.pdf\"\n";
    std::fs::write(&current, source).unwrap();
    std::fs::write(&target, "`= title Target note\n\n`# Anchor\n\n `@ anchor\n").unwrap();
    std::fs::write(&unicode_target, "`# 中文笔记\n").unwrap();
    std::fs::write(&image, b"png").unwrap();
    std::fs::write(&attachment, b"pdf").unwrap();

    let root_uri = lsp_types::Url::from_directory_path(&root).unwrap();
    let current_uri = lsp_types::Url::from_file_path(&current).unwrap();
    let target_uri = lsp_types::Url::from_file_path(&target).unwrap();
    let image_uri = lsp_types::Url::from_file_path(&image).unwrap();
    let attachment_uri = lsp_types::Url::from_file_path(&attachment).unwrap();
    let lines = source.lines().collect::<Vec<_>>();
    let verbatim_link_path_cursor = lines[0].find("tar").unwrap() + "tar".len();
    let verbatim_link_anchor_cursor = lines[1].find("#an").unwrap() + "#an".len();
    let image_query_cursor = lines[2].find("static/im").unwrap() + "static/im".len();
    let verbatim_link_definition = lines[4].find("target note.plumb").unwrap() + 2;
    let image_definition = lines[5].find("static/image").unwrap() + 2;
    let unicode_cursor = lines[6][..lines[6].find("中文").unwrap() + "中文".len()]
        .encode_utf16()
        .count();
    let attachment_verbatim_link = lines[7].find("manual draft").unwrap() + 2;
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
                "position": { "line": 0, "character": verbatim_link_path_cursor }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 1, "character": verbatim_link_anchor_cursor }
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
                "position": { "line": 4, "character": verbatim_link_definition }
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
                "position": { "line": 7, "character": attachment_verbatim_link }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 10, "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": current_uri },
                "position": { "line": 7, "character": attachment_verbatim_link }
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

    let output = run_server_after_initial_index(&messages);
    let verbatim_link_path = response(&output, 2)["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "target note.plumb")
        .expect("verbatim link document path completion");
    assert_eq!(verbatim_link_path["detail"], "Target note");
    assert_eq!(
        verbatim_link_path["textEdit"]["newText"],
        "target note.plumb"
    );

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
        .expect("Unicode verbatim link completion");
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
fn completes_task_and_event_only_from_the_list_marker() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("constructs.plumb");
    let source = "`t\n  `event\n `task\n `-\nText `e";
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
                    "completionItem": {
                        "snippetSupport": true,
                        "insertTextModeSupport": { "valueSet": [1, 2] }
                    }
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
                "position": { "line": 1, "character": 8 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 2, "character": 6 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 3, "character": 3 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 4, "character": 7 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    assert!(response(&output, 2)["result"].is_null());
    assert!(response(&output, 3)["result"].is_null());
    assert!(response(&output, 4)["result"].is_null());

    let list_item_items = response(&output, 5)["result"].as_array().unwrap();
    assert_eq!(list_item_items.len(), 3);
    assert_eq!(list_item_items[0]["label"], "Task");
    assert_eq!(list_item_items[1]["label"], "Event");
    assert_eq!(list_item_items[2]["label"], "Link");
    assert_eq!(list_item_items[0]["insertTextMode"], 1);
    assert_eq!(list_item_items[1]["insertTextMode"], 1);
    let task_text = list_item_items[0]["textEdit"]["newText"].as_str().unwrap();
    assert!(task_text.starts_with("`- ${1:Task}"));
    assert!(task_text.contains(" `+ task"));
    assert!(task_text.contains(" `= created "));
    assert_eq!(
        list_item_items[1]["textEdit"]["newText"],
        "`- ${1:09:00} ${2:Event}\n\n  `+ event"
    );
    assert!(response(&output, 6)["result"].is_null());

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
                "position": { "line": 3, "character": 3 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];
    let fallback_output = run_server(&fallback_messages);
    let fallback_items = response(&fallback_output, 2)["result"].as_array().unwrap();
    assert_eq!(fallback_items.len(), 3);
    assert_eq!(fallback_items[0]["label"], "Task");
    let fallback_task = fallback_items[0]["textEdit"]["newText"].as_str().unwrap();
    assert!(fallback_task.starts_with("`-\n `+ task\n\n `= created "));
    assert_eq!(fallback_items[0]["insertTextFormat"], 1);
    assert!(fallback_items[0].get("insertTextMode").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn projects_nested_task_completion_for_adjusted_indentation() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("adjusted.plumb");
    let source = "`- Parent\n `+ task\n `-";
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
                    "completionItem": {
                        "snippetSupport": true,
                        "insertTextModeSupport": { "valueSet": [2] }
                    },
                    "insertTextMode": 2
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
                "position": { "line": 2, "character": 3 }
            }
        }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        json!({ "jsonrpc": "2.0", "method": "exit", "params": null }),
    ];

    let output = run_server(&messages);
    let items = response(&output, 2)["result"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["label"], "Task");
    assert_eq!(items[0]["insertTextMode"], 2);
    assert_eq!(
        items[0]["textEdit"]["range"],
        json!({ "start": { "line": 2, "character": 1 }, "end": { "line": 2, "character": 3 } })
    );
    let replacement = items[0]["textEdit"]["newText"].as_str().unwrap();
    assert!(replacement.starts_with("`- ${1:Task}"));
    assert!(replacement.contains(" `+ task"));
    assert!(replacement.contains(" `= created "));

    let adjusted = replacement
        .replace("${1:Task}", "Task")
        .split('\n')
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_string()
            } else {
                format!(" {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let completed = format!("`- Parent\n `+ task\n {adjusted}");
    let parsed = plumb_syntax::parse(&completed);
    assert!(parsed.is_valid(), "{:?}\n{completed}", parsed.diagnostics);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_task_construct_immediately_after_direct_declarations() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("current.plumb");
    let source = "`- something\n\n `+ task\n\n `= created 2026-08-09T10:55:24+08:00\n\n`-\n";
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
                "position": { "line": 6, "character": 2 }
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
            "start": { "line": 6, "character": 0 },
            "end": { "line": 6, "character": 2 }
        })
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn narrows_link_constructs_from_the_shared_marker_prefix() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("construct-prefixes.plumb");
    let source = "Text `[\nText `-\nText `->\nText `->{\nText `->\"\n";
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

    let output = run_server_after_initial_index(&messages);
    assert!(response(&output, 2)["result"].is_null());

    for id in [3, 4] {
        let items = response(&output, id)["result"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["label"], "Link");
        assert_eq!(items[0]["textEdit"]["newText"], "`->{${1:target/label}}");
    }

    let link = response(&output, 5)["result"].as_array().unwrap();
    assert!(link.is_empty());

    let verbatim_link = response(&output, 6)["result"].as_array().unwrap();
    assert_eq!(verbatim_link.len(), 1);
    assert_eq!(verbatim_link[0]["label"], "Link");
    assert_eq!(verbatim_link[0]["textEdit"]["newText"], "`->\"${1:path}\"");
    assert_eq!(
        verbatim_link[0]["textEdit"]["range"],
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
        "`- Work\n\n `+ task\n\n `= created now\n `= pr\n\n`img{Alt `={s}} `${\"x\" `={language t}}\n";
    let lines = source.lines().collect::<Vec<_>>();
    let priority_line = 5;
    let resource_line = 7;
    let priority_cursor = lines[priority_line].len();
    let image_cursor = lines[resource_line].find("`={s").unwrap() + "`={s".len();
    let language_cursor = lines[resource_line].find("language t").unwrap() + "language t".len();
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
                "position": { "line": priority_line, "character": priority_cursor } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": document_uri },
                "position": { "line": resource_line, "character": image_cursor } }
        }),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/completion",
            "params": { "textDocument": { "uri": document_uri },
                "position": { "line": resource_line, "character": language_cursor } }
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
    assert_eq!(priority["textEdit"]["newText"], "`= priority ${1:0}");
    assert_eq!(priority["textEdit"]["range"]["start"]["character"], 1);
    assert_eq!(priority["insertTextFormat"], 2);
    let image = &response(&output, 3)["result"][0];
    assert_eq!(image["label"], "src");
    assert_eq!(image["textEdit"]["newText"], "`={src ${1}}");
    let language = &response(&output, 4)["result"][0];
    assert_eq!(language["label"], "tex");
    assert_eq!(language["textEdit"]["newText"], "tex");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completes_recursive_direct_members() {
    let root = unique_temp_dir();
    std::fs::create_dir_all(&root).unwrap();
    let document = root.join("direct-completion.plumb");
    let source = "`- Work\n\n `+ task\n\n `= pr\n\n`->{x target `={t}}\n";
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
                "position": { "line": 4, "character": 5 }
            }
        }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion",
            "params": {
                "textDocument": { "uri": document_uri },
                "position": { "line": 6, "character": 19 }
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
    assert_eq!(priority["textEdit"]["newText"], "`= priority ${1:0}");
    assert!(response(&output, 3)["result"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}
