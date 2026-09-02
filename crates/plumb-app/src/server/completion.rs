use super::*;
use plumb_edit::render_authored_text_arguments;

pub(super) fn attribute_completion_text(text: &str, snippets: bool) -> String {
    if !snippets {
        return text.to_string();
    }
    if let Some(prefix) = text.strip_suffix(" {}") {
        format!("{prefix} ${{1}}")
    } else if text == "`= priority 0" {
        "`= priority ${1:0}".to_string()
    } else if text.ends_with(' ') {
        format!("{text}${{1}}")
    } else {
        text.to_string()
    }
}

pub(super) fn completion_items(
    source: &str,
    candidates: Vec<CompletionCandidate>,
    kind: CompletionItemKind,
) -> Vec<CompletionItem> {
    candidates
        .into_iter()
        .map(|candidate| CompletionItem {
            label: candidate.label,
            kind: Some(kind),
            detail: Some(candidate.detail),
            text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                byte_range_to_lsp(source, &candidate.replace),
                candidate.new_text,
            ))),
            ..CompletionItem::default()
        })
        .collect()
}

pub(super) struct ConstructTemplate {
    pub(super) label: &'static str,
    pub(super) detail: &'static str,
    pub(super) snippet: String,
    pub(super) plain: String,
    pub(super) uses_block_indentation: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CompletionIndentationProjection {
    AsIs,
    AdjustIndentation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CompletionIndentation {
    pub(super) projection: CompletionIndentationProjection,
    pub(super) item_mode: Option<InsertTextMode>,
}

impl Default for CompletionIndentation {
    fn default() -> Self {
        Self {
            projection: CompletionIndentationProjection::AdjustIndentation,
            item_mode: None,
        }
    }
}

pub(super) fn completion_indentation(
    capabilities: Option<&lsp_types::CompletionClientCapabilities>,
) -> CompletionIndentation {
    let Some(capabilities) = capabilities else {
        return CompletionIndentation::default();
    };
    let supported = capabilities
        .completion_item
        .as_ref()
        .and_then(|item| item.insert_text_mode_support.as_ref())
        .map(|support| support.value_set.as_slice())
        .unwrap_or_default();

    if supported.contains(&InsertTextMode::AS_IS) {
        CompletionIndentation {
            projection: CompletionIndentationProjection::AsIs,
            item_mode: Some(InsertTextMode::AS_IS),
        }
    } else if supported.contains(&InsertTextMode::ADJUST_INDENTATION) {
        CompletionIndentation {
            projection: CompletionIndentationProjection::AdjustIndentation,
            item_mode: Some(InsertTextMode::ADJUST_INDENTATION),
        }
    } else if capabilities.insert_text_mode == Some(InsertTextMode::AS_IS) {
        CompletionIndentation {
            projection: CompletionIndentationProjection::AsIs,
            item_mode: None,
        }
    } else {
        CompletionIndentation::default()
    }
}

pub(super) fn task_construct_template(block_indent: &str, timestamp: &str) -> ConstructTemplate {
    let created = render_authored_text_arguments(&["created", timestamp]);
    ConstructTemplate {
        label: "Task",
        detail: "plumb task list item",
        snippet: format!("`- ${{1:Task}}\n\n{block_indent}`+ task\n\n{block_indent}`= {created}"),
        plain: format!("`-\n{block_indent}`+ task\n\n{block_indent}`= {created}"),
        uses_block_indentation: true,
    }
}

fn event_construct_template(block_indent: &str) -> ConstructTemplate {
    let head = render_authored_text_arguments(&["${1:09:00}", "${2:Event}"]);
    let plain_head = render_authored_text_arguments(&["09:00", "Event"]);
    ConstructTemplate {
        label: "Event",
        detail: "plumb event list item",
        snippet: format!("`- {head}\n\n{block_indent}`+ event"),
        plain: format!("`- {plain_head}\n\n{block_indent}`+ event"),
        uses_block_indentation: true,
    }
}

pub(super) fn construct_completion_items(
    source: &str,
    context: ConstructCompletionContext,
    snippets: bool,
    completion_indentation: CompletionIndentation,
    timestamp: &str,
) -> Vec<CompletionItem> {
    let block_indent = match (&context, completion_indentation.projection) {
        (
            ConstructCompletionContext::TaskEventLinkAndAutolink { replace },
            CompletionIndentationProjection::AsIs,
        ) => {
            let line_start = source[..replace.start]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            format!("{} ", &source[line_start..replace.start])
        }
        _ => " ".to_string(),
    };
    let (replace, templates) = match context {
        ConstructCompletionContext::Citation { replace } => (
            replace,
            vec![ConstructTemplate {
                label: "Citation",
                detail: "plumb citation",
                snippet: "`cite{${1:id}}".to_string(),
                plain: "`cite{}".to_string(),
                uses_block_indentation: false,
            }],
        ),
        ConstructCompletionContext::TaskEventLinkAndAutolink { replace } => (
            replace,
            vec![
                task_construct_template(&block_indent, timestamp),
                event_construct_template(&block_indent),
                link_construct_template(),
                autolink_construct_template(),
            ],
        ),
        ConstructCompletionContext::LinkAndAutolink { replace } => (
            replace,
            vec![link_construct_template(), autolink_construct_template()],
        ),
        ConstructCompletionContext::Autolink { replace } => {
            (replace, vec![autolink_construct_template()])
        }
        ConstructCompletionContext::Link { replace } => (replace, vec![link_construct_template()]),
    };
    templates
        .into_iter()
        .map(|template| CompletionItem {
            label: template.label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(template.detail.to_string()),
            insert_text_format: Some(if snippets {
                InsertTextFormat::SNIPPET
            } else {
                InsertTextFormat::PLAIN_TEXT
            }),
            insert_text_mode: template
                .uses_block_indentation
                .then_some(completion_indentation.item_mode)
                .flatten(),
            text_edit: Some(CompletionTextEdit::Edit(LspTextEdit::new(
                byte_range_to_lsp(source, &replace),
                if snippets {
                    template.snippet
                } else {
                    template.plain
                },
            ))),
            ..CompletionItem::default()
        })
        .collect()
}

fn link_construct_template() -> ConstructTemplate {
    ConstructTemplate {
        label: "Link",
        detail: "plumb link",
        snippet: "`->{{${1:label}} ${2:target}}".to_string(),
        plain: "`->{{} {}}".to_string(),
        uses_block_indentation: false,
    }
}

fn autolink_construct_template() -> ConstructTemplate {
    ConstructTemplate {
        label: "Autolink",
        detail: "plumb autolink",
        snippet: "`->\"${1:path}\"".to_string(),
        plain: "`->\"\"".to_string(),
        uses_block_indentation: false,
    }
}
