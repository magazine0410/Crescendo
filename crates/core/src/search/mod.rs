//! Quod Libet-style search queries, compiled to SQL WHERE fragments.
//!
//! Supported subset (see docs/searching in Quod Libet):
//! - free text: `nausicaa wind` (terms AND-ed, matched against a
//!   pre-lowercased blob of title/artist/album/albumartist/genre)
//! - tag match: `artist=parker`, `genre!=rock`, `artist="Charlie Parker"`,
//!   `artist="BoA"c` (quoted = substring match preserving spaces, `c` =
//!   case-sensitive)
//! - regex: `album=/shadow/`, `/Boa/c` (case-insensitive unless `c`)
//! - boolean: `&(a, b)`, `|(a, b)`, `!term`, value lists `genre=|(jazz, soul)`
//! - numeric: `#(year >= 1990)`, `#(10 <= track < 100)`, `#(length > 2:00)`,
//!   or without the `#( )` wrapper: `plays>2`, `year>=1990`
//! - plays: `plays>2` / `plays=3` / `plays<3` over the per-track play counter.
//!   Zero means "never played", so a comparison hides those rows unless the
//!   query names 0 (`plays=0`).

mod parser;
mod sql;

pub(crate) use sql::browse_column;

use rusqlite::types::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    /// Bare or quoted text with no tag: match the search blob.
    FreeText(String),
    Tag {
        tag: String,
        matcher: Matcher,
        negated: bool,
    },
    /// Chained comparison: operands.len() == ops.len() + 1.
    Numeric {
        operands: Vec<NumOperand>,
        ops: Vec<CmpOp>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Matcher {
    /// Unquoted value: case-folded substring match.
    Contains(String),
    /// Quoted value: substring match that preserves spaces and special chars.
    /// Case-folded unless `case_sensitive` (the `c` flag) is set.
    Phrase {
        value: String,
        case_sensitive: bool,
    },
    Regex {
        pattern: String,
        case_sensitive: bool,
    },
    AnyOf(Vec<Matcher>),
    AllOf(Vec<Matcher>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum NumOperand {
    Number(f64),
    Tag(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Eq,
    Ne,
    Ge,
    Gt,
}

#[derive(Debug, thiserror::Error)]
#[error("{message} (at offset {offset})")]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

/// A compiled query: SQL fragment over the `tracks` table using only
/// anonymous `?` placeholders, plus its bind parameters in order.
#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub where_sql: String,
    pub params: Vec<Value>,
}

pub fn parse(query: &str) -> Result<Node, ParseError> {
    parser::parse(query)
}

pub fn compile(query: &str) -> anyhow::Result<SearchFilter> {
    let node = parse(query).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(sql::compile(&node))
}

/// None if the query parses, otherwise the error message (for the red
/// search-bar tint).
pub fn validate(query: &str) -> Option<String> {
    if query.trim().is_empty() {
        return None;
    }
    parse(query).err().map(|e| e.to_string())
}

/// How the search bar should tint a query. Drives the input text color:
/// plain free text stays white, a valid structured query turns green, an
/// unparseable one turns red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryKind {
    /// Only bare words — no operators. Render normally.
    Plain,
    /// Parses and uses search syntax (tags, numbers, booleans, …).
    Syntax,
    /// Fails to parse.
    Invalid,
}

/// One top-level term of a query and how the search bar should tint it. Byte
/// offsets into the query as typed; the text between two spans is whitespace
/// and stays uncoloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct QuerySpan {
    pub start: usize,
    pub end: usize,
    pub kind: QueryKind,
}

/// Classify every top-level term on its own, so a query mixing plain words with
/// tag syntax tints only the parts that use it — and a term that fails to parse
/// reddens only itself.
pub fn highlight(query: &str) -> Vec<QuerySpan> {
    parser::segments(query)
        .into_iter()
        .map(|s| QuerySpan {
            start: s.start,
            end: s.end,
            kind: match &s.node {
                None => QueryKind::Invalid,
                Some(node) if uses_syntax(node) => QueryKind::Syntax,
                Some(_) => QueryKind::Plain,
            },
        })
        .filter(|s| s.end > s.start)
        .collect()
}

/// Classify a query for the search-bar tint.
pub fn classify(query: &str) -> QueryKind {
    if query.trim().is_empty() {
        return QueryKind::Plain;
    }
    match parse(query) {
        Err(_) => QueryKind::Invalid,
        Ok(node) if uses_syntax(&node) => QueryKind::Syntax,
        Ok(_) => QueryKind::Plain,
    }
}

/// True if the AST is anything more than bare free-text words. A run of plain
/// words parses to `And([FreeText, …])`, which stays "plain"; a tag, number,
/// negation, or OR anywhere makes it a structured query.
fn uses_syntax(node: &Node) -> bool {
    match node {
        Node::FreeText(_) => false,
        Node::Tag { .. } | Node::Numeric { .. } | Node::Not(_) | Node::Or(_) => true,
        Node::And(items) => items.iter().any(uses_syntax),
    }
}
