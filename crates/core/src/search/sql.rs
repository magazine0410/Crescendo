use super::{CmpOp, Matcher, Node, NumOperand, SearchFilter};
use rusqlite::types::Value;

/// Short aliases a query may use in place of a full tag name. Applied once by
/// the parser's `normalize_tag`, so every lookup below sees a canonical name
/// and only that name has to be handled here. The custom browse filter and the
/// plugin manifest keep their canonical-only vocabulary (`browse_column`), so
/// an alias is a search-bar convenience and never a stored tag identity.
pub(super) fn canonical_tag(tag: &str) -> &str {
    match tag {
        "a" => "album",
        "aa" | "aart" => "albumartist",
        "aas" => "albumartistsort",
        "arr" => "arranger",
        "art" => "artist",
        "arts" | "as" => "artistsort",
        "bd" => "bitdepth",
        "br" => "bitrate",
        "cat" | "cn" => "catalognumber",
        "com" => "composer",
        "dir" => "dirname",
        "e" => "event",
        "f" => "format",
        "file" | "fn" => "filename",
        "g" => "genre",
        "gr" => "grouping",
        "l" => "length",
        "lyr" => "lyricist",
        "p" => "plays",
        "r" | "rat" => "rating",
        "sr" => "samplerate",
        "t" => "title",
        "tr" => "track",
        "ts" => "titlesort",
        "voc" => "vocalist",
        "y" => "year",
        other => other,
    }
}

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
        "composer" => "composer",
        "arranger" => "arranger",
        "lyricist" => "lyricist",
        "vocalist" => "vocalist",
        "catalognumber" | "catalog" => "catalognumber",
        "event" => "event",
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
        "bpm" => "bpm",
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
        "bpm" => "bpm",
        "bitrate" => "bitrate",
        "samplerate" => "sample_rate",
        "bitdepth" => "bit_depth",
        "plays" | "playcount" => "play_count",
        _ => return None,
    })
}

/// True if the tag compares as a number. The parser uses this to let a bare
/// `plays>2` / `year>=2010` start a comparison, which for a text tag stays
/// ordinary free text.
pub(super) fn is_numeric_tag(tag: &str) -> bool {
    num_expr(tag).is_some()
}

/// Guard for tags whose zero means "it never happened" instead of a measured
/// value. `plays<3` must not answer with the whole never-played library, so a
/// comparison on such a tag only sees rows above zero. A query that names 0
/// itself (`plays=0`, `!plays>0`) keeps its plain meaning and finds the
/// never-played tracks.
fn zero_is_absent(tag: &str) -> Option<&'static str> {
    match tag {
        "plays" | "playcount" => Some("play_count > 0"),
        _ => None,
    }
}

/// The `zero_is_absent` guard for one comparison chain, or None when the chain
/// does not need one. The chain is probed at 0 and at a few positive counts:
/// the guard is added only where zero would match *and* a real count would
/// match too. `plays<3` therefore means 1 or 2, while `plays<1` and `plays=0`
/// keep their plain meaning and find the never-played tracks.
fn zero_guard(operands: &[NumOperand], ops: &[CmpOp]) -> Option<&'static str> {
    let (tag, guard) = operands.iter().find_map(|o| match o {
        NumOperand::Tag(t) => zero_is_absent(t).map(|g| (t.as_str(), g)),
        NumOperand::Number(_) => None,
    })?;
    if !chain_holds(operands, ops, tag, 0.0) {
        return None;
    }
    // Counts are whole numbers, so the literals and their neighbours cover
    // every interval the chain can describe.
    let mut probes = vec![1.0];
    for o in operands {
        if let NumOperand::Number(n) = o {
            probes.extend([n - 1.0, *n, n + 1.0]);
        }
    }
    probes
        .iter()
        .any(|&x| x > 0.0 && chain_holds(operands, ops, tag, x))
        .then_some(guard)
}

/// Evaluate a comparison chain with `tag` set to `x`. A comparison against a
/// different tag has no known value; it counts as holding, so an unrelated
/// operand never decides the guard on its own.
fn chain_holds(operands: &[NumOperand], ops: &[CmpOp], tag: &str, x: f64) -> bool {
    let value = |o: &NumOperand| match o {
        NumOperand::Number(n) => Some(*n),
        NumOperand::Tag(t) => (t == tag).then_some(x),
    };
    ops.iter().enumerate().all(
        |(i, op)| match (value(&operands[i]), value(&operands[i + 1])) {
            (Some(a), Some(b)) => match op {
                CmpOp::Lt => a < b,
                CmpOp::Le => a <= b,
                CmpOp::Eq => a == b,
                CmpOp::Ne => a != b,
                CmpOp::Ge => a >= b,
                CmpOp::Gt => a > b,
            },
            _ => true,
        },
    )
}

pub fn compile(node: &Node) -> SearchFilter {
    let mut c = Compiler {
        sql: String::new(),
        params: Vec::new(),
    };
    c.node(node);
    SearchFilter {
        where_sql: c.sql,
        params: c.params,
    }
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
            Node::Tag {
                tag,
                matcher,
                negated,
            } => {
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
                let guard = zero_guard(operands, ops);
                if let Some(g) = guard {
                    self.sql.push_str(&format!("({g} AND "));
                }
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
                if guard.is_some() {
                    self.sql.push(')');
                }
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
            Matcher::Phrase {
                value,
                case_sensitive,
            } => {
                if *case_sensitive {
                    // instr() is a binary (case-sensitive) substring search.
                    // It is NULL for a NULL field, which a negation would keep
                    // as NULL and drop; ucontains() and regexp() say false.
                    self.sql
                        .push_str(&format!("instr(COALESCE({col}, ''), ?) > 0"));
                } else {
                    self.sql.push_str(&format!("ucontains({col}, ?)"));
                }
                self.params.push(Value::from(value.clone()));
            }
            Matcher::Regex {
                pattern,
                case_sensitive,
            } => {
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
