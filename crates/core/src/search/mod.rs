//! Quod Libet-style search queries, compiled to SQL WHERE fragments.
//!
//! Supported subset (see docs/searching in Quod Libet):
//! - free text: `nausicaa wind` (terms AND-ed, matched against a
//!   pre-lowercased blob of title/artist/album/albumartist/genre)
//! - tag match: `artist=parker`, `genre!=rock`, `artist="Charlie Parker"`,
//!   `artist="BoA"c` (quoted = exact, `c` = case-sensitive)
//! - regex: `album=/shadow/`, `/Boa/c` (case-insensitive unless `c`)
//! - boolean: `&(a, b)`, `|(a, b)`, `!term`, value lists `genre=|(jazz, soul)`
//! - numeric: `#(year >= 1990)`, `#(10 <= track < 100)`, `#(length > 2:00)`

mod parser;
mod sql;

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
    Exact { value: String, case_sensitive: bool },
    Regex { pattern: String, case_sensitive: bool },
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
