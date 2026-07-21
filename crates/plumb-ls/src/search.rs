use lsp_types::{request::Request, Location};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub(crate) enum PlumbSearchRequest {}

impl Request for PlumbSearchRequest {
    type Params = SearchParams;
    type Result = SearchResult;
    const METHOD: &'static str = "plumb/search";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SearchKind {
    Note,
    Task,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchParams {
    #[serde(default)]
    pub kind: Option<SearchKind>,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResult {
    pub schema_version: u32,
    pub items: Vec<SearchItem>,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchItem {
    pub kind: SearchKind,
    pub title: String,
    pub path: String,
    pub location: Location,
    pub provenance: SearchProvenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actionable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchProvenance {
    pub source: String,
    pub revision: i64,
}
