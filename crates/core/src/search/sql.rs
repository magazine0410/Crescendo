use super::{CmpOp, Matcher, Node, NumOperand, SearchFilter};
use rusqlite::types::Value;

/// String-tag -> column. Unknown tags match nothing (Quod Libet behavior).
fn str_col(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "title" => "title",
        "artist" => "artist",
        "album" => "album",
        "albumartist" => "albumartist",
        "titlesort" => "titlesort",
        "artistsort" => "artistsort",
        "albumartistsort" => "albumartistsort",
        "genre" => "genre",
        "grouping" => "grouping",
        "format" | "codec" => "format",
        "dirname" => "dir",
        "filename" => "filename",
        "path" => "path",
        _ => return None,
    })
}

/// Resolve a user-typed tag to a real, browsable `tracks` column for the custom
/// left-panel filter. Reuses the search string-tag vocabulary, plus the numeric
/// tags that are backed by a plain column (excluding computed exprs like
/// `length`). The result is always a hardcoded column literal — never the raw
/// input — so it is safe to interpolate into SQL. `None` = unbrowsable tag,
/// which the caller turns into an empty value list.
pub(crate) fn browse_column(tag: &str) -> Option<&'static str> {
    let t = tag.trim().to_lowercase();
    if let Some(c) = str_col(&t) {
        return Some(c);
    }
    Some(match t.as_str() {
        "track" | "tracknumber" => "track_no",
        "disc" | "discnumber" => "disc_no",
        "year" | "date" => "year",
        "rating" => "rating",
        "bitrate" => "bitrate",
        "samplerate" => "sample_rate",
        "bitdepth" => "bit_depth",
        _ => return None,
    })
}

/// Numeric-tag -> SQL expression yielding a number.
fn num_expr(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "track" | "tracknumber" => "track_no",
        "disc" | "discnumber" => "disc_no",
        "year" | "date" => "year",
        "length" => "(duration_ms / 1000.0)",
        "rating" => "rating",
        "bitrate" => "bitrate",
        "samplerate" => "sample_rate",
        "bitdepth" => "bit_depth",
        _ => return None,
    })
}

pub fn compile(node: &Node) -> SearchFilter {
    let mut c = Compiler { sql: String::new(), params: Vec::new() };
    c.node(node);
    SearchFilter { where_sql: c.sql, params: c.params }
}

struct Compiler {
    sql: String,
    params: Vec<Value>,
}

impl Compiler {
    fn node(&mut self, node: &Node) {
        match node {
            Node::And(items) => self.list(items, " AND ", "1"),
            Node::Or(items) => self.list(items, " OR ", "0"),
            Node::Not(inner) => {
                self.sql.push_str("NOT (");
                self.node(inner);
                self.sql.push(')');
            }
            Node::FreeText(text) => {
                // search_blob is Rust-lowercased at insert time; lowercase the
                // needle here (SQLite lower() is ASCII-only).
                self.sql.push_str("instr(search_blob, ?) > 0");
                self.params.push(Value::from(text.to_lowercase()));
            }
            Node::Tag { tag, matcher, negated } => {
                if *negated {
                    self.sql.push_str("NOT (");
                }
                match str_col(tag) {
                    Some(col) => self.matcher(col, matcher),
                    None => self.sql.push_str("0"),
                }
                if *negated {
                    self.sql.push(')');
                }
            }
            Node::Numeric { operands, ops } => {
                self.sql.push('(');
                for (i, op) in ops.iter().enumerate() {
                    if i > 0 {
                        self.sql.push_str(" AND ");
                    }
                    self.num_operand(&operands[i]);
                    self.sql.push_str(match op {
                        CmpOp::Lt => " < ",
                        CmpOp::Le => " <= ",
                        CmpOp::Eq => " = ",
                        CmpOp::Ne => " != ",
                        CmpOp::Ge => " >= ",
                        CmpOp::Gt => " > ",
                    });
                    self.num_operand(&operands[i + 1]);
                }
                self.sql.push(')');
            }
        }
    }

    fn list(&mut self, items: &[Node], joiner: &str, empty: &str) {
        if items.is_empty() {
            self.sql.push_str(empty);
            return;
        }
        self.sql.push('(');
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                self.sql.push_str(joiner);
            }
            self.node(item);
        }
        self.sql.push(')');
    }

    fn matcher(&mut self, col: &str, m: &Matcher) {
        match m {
            Matcher::Contains(value) => {
                self.sql.push_str(&format!("ucontains({col}, ?)"));
                self.params.push(Value::from(value.clone()));
            }
            Matcher::Exact { value, case_sensitive } => {
                if *case_sensitive {
                    self.sql.push_str(&format!("{col} = ?"));
                    self.params.push(Value::from(value.clone()));
                } else {
                    self.sql.push_str(&format!("ueq({col}, ?)"));
                    self.params.push(Value::from(value.clone()));
                }
            }
            Matcher::Regex { pattern, case_sensitive } => {
                let pattern = if *case_sensitive {
                    pattern.clone()
                } else {
                    format!("(?i){pattern}")
                };
                self.sql.push_str(&format!("regexp(?, {col})"));
                self.params.push(Value::from(pattern));
            }
            Matcher::AnyOf(items) => self.matcher_list(col, items, " OR ", "0"),
            Matcher::AllOf(items) => self.matcher_list(col, items, " AND ", "1"),
        }
    }

    fn matcher_list(&mut self, col: &str, items: &[Matcher], joiner: &str, empty: &str) {
        if items.is_empty() {
            self.sql.push_str(empty);
            return;
        }
        self.sql.push('(');
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                self.sql.push_str(joiner);
            }
            self.matcher(col, item);
        }
        self.sql.push(')');
    }

    fn num_operand(&mut self, op: &NumOperand) {
        match op {
            NumOperand::Number(n) => {
                self.sql.push('?');
                self.params.push(Value::from(*n));
            }
            NumOperand::Tag(tag) => match num_expr(tag) {
                Some(expr) => self.sql.push_str(expr),
                None => self.sql.push_str("NULL"),
            },
        }
    }
}
