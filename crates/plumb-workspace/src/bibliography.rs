use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use plumb_semantics::{CitationRecord, MetadataOutput};
use plumb_syntax::{Diagnostic, DiagnosticSeverity};
use serde_json::Value;

use crate::normalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyRecord {
    pub id: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<String>,
    pub path: PathBuf,
    pub range: Range<usize>,
}

impl BibliographyRecord {
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        if !self.authors.is_empty() {
            parts.push(self.authors.join(", "));
        }
        if let Some(year) = &self.year {
            parts.push(year.clone());
        }
        if let Some(title) = &self.title {
            parts.push(title.clone());
        }
        if parts.is_empty() {
            "CSL JSON citation".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BibliographyResolution<'a> {
    Resolved(&'a BibliographyRecord),
    Ambiguous,
    Unresolved,
}

#[derive(Debug, Clone, Default)]
pub struct Bibliography {
    pub declared: bool,
    pub sources: Vec<PathBuf>,
    pub records: Vec<BibliographyRecord>,
    pub diagnostics: Vec<Diagnostic>,
    by_id: HashMap<String, Vec<usize>>,
}

impl Bibliography {
    pub fn resolve(&self, id: &str) -> BibliographyResolution<'_> {
        match self.by_id.get(id).map(Vec::as_slice) {
            Some([index]) => BibliographyResolution::Resolved(&self.records[*index]),
            Some(_) => BibliographyResolution::Ambiguous,
            None => BibliographyResolution::Unresolved,
        }
    }

    pub fn citation_diagnostics(&self, citations: &[CitationRecord]) -> Vec<Diagnostic> {
        citations
            .iter()
            .filter_map(|citation| {
                let (code, message) = match self.resolve(&citation.id) {
                    BibliographyResolution::Resolved(_) => return None,
                    BibliographyResolution::Ambiguous => (
                        "citation.ambiguous",
                        format!("citation id '{}' is defined more than once", citation.id),
                    ),
                    BibliographyResolution::Unresolved => (
                        "citation.unresolved",
                        format!(
                            "citation id '{}' is not in the declared bibliography",
                            citation.id
                        ),
                    ),
                };
                Some(Diagnostic {
                    code,
                    severity: DiagnosticSeverity::Warning,
                    message,
                    range: citation.selection_range.clone(),
                    related: Vec::new(),
                })
            })
            .collect()
    }
}

pub fn load_bibliography(
    root: &Path,
    document_path: &Path,
    metadata: &MetadataOutput,
) -> Bibliography {
    let sources = metadata.bibliography_sources();
    let declaration_range = sources
        .first()
        .map(|source| source.range.clone())
        .unwrap_or(0..0);
    let mut output = Bibliography {
        declared: !sources.is_empty(),
        ..Bibliography::default()
    };
    let root = normalize(root);
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    for source in sources {
        let Some(path) = resolve_source(&root, &canonical_root, document_path, &source.value)
        else {
            output.diagnostics.push(diagnostic(
                "citation.invalid-bibliography-path",
                format!(
                    "bibliography path '{}' must be a workspace-relative CSL JSON file",
                    source.value
                ),
                source.range,
            ));
            continue;
        };
        output.sources.push(path.clone());
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                output.diagnostics.push(diagnostic(
                    "citation.unresolved-bibliography",
                    format!("cannot read bibliography '{}': {error}", path.display()),
                    source.range,
                ));
                continue;
            }
        };
        match parse_csl_json(&path, &text) {
            Ok(records) => output.records.extend(records),
            Err(message) => output.diagnostics.push(diagnostic(
                "citation.invalid-bibliography",
                format!(
                    "invalid CSL JSON bibliography '{}': {message}",
                    path.display()
                ),
                source.range,
            )),
        }
    }
    output
        .records
        .sort_by(|left, right| left.id.cmp(&right.id).then(left.path.cmp(&right.path)));
    for (index, record) in output.records.iter().enumerate() {
        output
            .by_id
            .entry(record.id.clone())
            .or_default()
            .push(index);
    }
    let duplicates = output
        .by_id
        .iter()
        .filter(|(_, indexes)| indexes.len() > 1)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    for id in &duplicates {
        output.diagnostics.push(diagnostic(
            "citation.duplicate-id",
            format!("citation id '{id}' is defined more than once"),
            declaration_range.clone(),
        ));
    }
    output
}

fn resolve_source(
    root: &Path,
    canonical_root: &Path,
    document_path: &Path,
    value: &str,
) -> Option<PathBuf> {
    let target = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || target.is_absolute()
        || target.extension().and_then(|value| value.to_str()) != Some("json")
        || target
            .components()
            .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return None;
    }
    let path = normalize(&document_path.parent().unwrap_or(root).join(target));
    if !path.starts_with(root) {
        return None;
    }
    if let Ok(canonical) = path.canonicalize() {
        canonical.starts_with(canonical_root).then_some(path)
    } else {
        Some(path)
    }
}

fn parse_csl_json(path: &Path, text: &str) -> Result<Vec<BibliographyRecord>, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let items = value.as_array().ok_or("root must be an array")?;
    let mut search_start = 0;
    items
        .iter()
        .map(|item| {
            let object = item.as_object().ok_or("each item must be an object")?;
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or("each item must have a nonempty string id")?
                .to_string();
            let range = find_id_value_range(text, &id, search_start)
                .ok_or_else(|| format!("cannot locate source for id '{id}'"))?;
            search_start = range.end;
            Ok(BibliographyRecord {
                id,
                title: object
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                authors: object
                    .get("author")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(author_name)
                    .collect(),
                year: object
                    .get("issued")
                    .and_then(|issued| issued.get("date-parts"))
                    .and_then(Value::as_array)
                    .and_then(|parts| parts.first())
                    .and_then(Value::as_array)
                    .and_then(|parts| parts.first())
                    .and_then(|year| {
                        year.as_i64()
                            .map(|year| year.to_string())
                            .or_else(|| year.as_str().map(str::to_string))
                    }),
                path: path.to_path_buf(),
                range,
            })
        })
        .collect()
}

fn find_id_value_range(text: &str, id: &str, start: usize) -> Option<Range<usize>> {
    let key = "\"id\"";
    let value = serde_json::to_string(id).ok()?;
    let mut cursor = start;
    while let Some(relative) = text[cursor..].find(key) {
        let key_end = cursor + relative + key.len();
        let remainder = &text[key_end..];
        let colon = remainder.find(':')?;
        let value_start = key_end + colon + 1;
        let whitespace = text[value_start..]
            .char_indices()
            .find(|(_, character)| !character.is_whitespace())
            .map(|(offset, _)| offset)?;
        let value_start = value_start + whitespace;
        if text[value_start..].starts_with(&value) {
            return Some(value_start..value_start + value.len());
        }
        cursor = key_end;
    }
    None
}

fn author_name(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    object
        .get("literal")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            object
                .get("family")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn diagnostic(code: &'static str, message: String, range: Range<usize>) -> Diagnostic {
    Diagnostic {
        code,
        severity: DiagnosticSeverity::Warning,
        message,
        range,
        related: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use plumb_semantics::analyze_document;
    use plumb_syntax::parse;

    use super::*;

    #[test]
    fn loads_plain_csl_json_and_resolves_citations() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("refs.json"), r#"[{"id":"smith2004","title":"Book","author":[{"family":"Smith"}],"issued":{"date-parts":[[2004]]}}]"#).unwrap();
        let parsed = parse("{\n `= bibliography refs.json\n}\n\n`cite[smith2004]\n");
        let analysis = analyze_document(&parsed.source, &parsed.syntax);
        let bibliography = load_bibliography(
            root.path(),
            &root.path().join("note.plumb"),
            &analysis.metadata,
        );
        assert!(
            bibliography.diagnostics.is_empty(),
            "{:?}",
            bibliography.diagnostics
        );
        let BibliographyResolution::Resolved(record) = bibliography.resolve("smith2004") else {
            panic!()
        };
        assert_eq!(record.detail(), "Smith · 2004 · Book");
    }

    #[test]
    fn diagnoses_missing_sources_duplicate_ids_and_unresolved_citations() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("duplicate.json"),
            r#"[{"id":"same"},{"id":"same"}]"#,
        )
        .unwrap();
        let parsed = parse(
            "{\n `= bibliography\n  `- missing.json\n  `- duplicate.json\n}\n\n`cite[unknown]\n",
        );
        let analysis = analyze_document(&parsed.source, &parsed.syntax);
        let bibliography = load_bibliography(
            root.path(),
            &root.path().join("note.plumb"),
            &analysis.metadata,
        );
        let codes = bibliography
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"citation.unresolved-bibliography"));
        assert!(codes.contains(&"citation.duplicate-id"));
        assert_eq!(
            bibliography.citation_diagnostics(&analysis.citations.citations)[0].code,
            "citation.unresolved"
        );
    }

    #[test]
    fn diagnoses_citations_without_a_declared_bibliography() {
        let root = tempfile::tempdir().unwrap();
        let parsed = parse("See `cite[smith2004]\n");
        let analysis = analyze_document(&parsed.source, &parsed.syntax);
        let bibliography = load_bibliography(
            root.path(),
            &root.path().join("note.plumb"),
            &analysis.metadata,
        );
        let diagnostics = bibliography.citation_diagnostics(&analysis.citations.citations);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "citation.unresolved");
    }

    #[test]
    fn rejects_bibliographies_outside_the_workspace() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(parent.path().join("outside.json"), "[]").unwrap();
        let parsed = parse("{\n `= bibliography ../outside.json\n}\n");
        let analysis = analyze_document(&parsed.source, &parsed.syntax);
        let bibliography = load_bibliography(&root, &root.join("note.plumb"), &analysis.metadata);
        assert_eq!(
            bibliography.diagnostics[0].code,
            "citation.invalid-bibliography-path"
        );
    }

    #[test]
    fn definition_range_targets_the_id_field_not_an_equal_title() {
        let root = tempfile::tempdir().unwrap();
        let text = r#"[{"title":"same","id":"same"}]"#;
        std::fs::write(root.path().join("refs.json"), text).unwrap();
        let parsed = parse("{\n `= bibliography refs.json\n}\n");
        let analysis = analyze_document(&parsed.source, &parsed.syntax);
        let bibliography = load_bibliography(
            root.path(),
            &root.path().join("note.plumb"),
            &analysis.metadata,
        );
        let BibliographyResolution::Resolved(record) = bibliography.resolve("same") else {
            panic!()
        };
        assert_eq!(&text[record.range.clone()], "\"same\"");
        assert!(record.range.start > text.find("\"same\"").unwrap());
    }
}
