use lsp_types::{request::Request, Location};
use serde::{Deserialize, Serialize};

use crate::search::SearchProvenance;

/// Protocol projection of the shared `next` shortlist. The two sections are
/// produced by `plumb_workspace::query_next`; this adapter only maps them to LSP
/// types, so editors never re-implement the filtering or ordering.
#[derive(Debug)]
pub(crate) enum PlumbNextRequest {}

impl Request for PlumbNextRequest {
    type Params = NextParams;
    type Result = NextResult;
    const METHOD: &'static str = "plumb/next";
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NextParams {
    /// Ready-to-start candidates to return (clamped to 1..=10 by the shared
    /// layer; the default is 3).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque continuation cursor for the in-flight section.
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NextResult {
    pub schema_version: u32,
    pub focused: Vec<NextItem>,
    pub focused_total: usize,
    pub focused_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_next_cursor: Option<String>,
    pub candidates: Vec<NextItem>,
    pub candidate_limit: usize,
    pub candidates_complete: bool,
    pub skipped_invalid: Vec<NextSkippedItem>,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NextItem {
    pub title: String,
    pub path: String,
    pub location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub state: String,
    pub wait_reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_since: Option<String>,
    pub effective_priority: i32,
    pub provenance: SearchProvenance,
}

/// A task whose focus history is invalid. It is never treated as unfocused; it
/// is reported so the editor can jump to the offending value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NextSkippedItem {
    pub title: String,
    pub path: String,
    pub location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub codes: Vec<String>,
}
