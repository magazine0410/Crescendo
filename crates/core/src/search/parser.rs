use super::sql::{canonical_tag, is_numeric_tag};
use super::{CmpOp, Matcher, Node, NumOperand, ParseError};

pub fn parse(input: &str) -> Result<Node, ParseError> {
    let mut p = Parser {
        chars: input.char_indices().collect(),
        pos: 0,
    };
    let mut terms = Vec::new();
    loop {
        p.skip_ws();
        if p.eof() {
            break;
        }
        terms.push(p.term()?);
    }
    match terms.len() {
        0 => Ok(Node::And(vec![])),
        1 => Ok(terms.pop().unwrap()),
        _ => Ok(Node::And(terms)),
    }
}

/// One top-level term of a query with the byte range it covers, so the search
/// bar can tint each piece on its own instead of the whole input. The terms are
/// exactly the ones `parse` reads, so a span means the same thing there and
/// here; `node` is None for a term that does not parse.
pub struct Segment {
    pub start: usize,
    pub end: usize,
    pub node: Option<Node>,
}

/// Split a query into its top-level terms. A term that fails to parse ends
/// where the parser gave up (a missing quote therefore runs to the end of the
/// query, while a stray character stops at the next space), and the walk
/// continues after it, so one bad term never discolours the good ones.
pub fn segments(input: &str) -> Vec<Segment> {
    let mut p = Parser {
        chars: input.char_indices().collect(),
        pos: 0,
    };
    let mut out = Vec::new();
    loop {
        p.skip_ws();
        if p.eof() {
            break;
        }
        let start_pos = p.pos;
        let start = p.offset();
        let (node, end) = match p.term() {
            Ok(node) => (Some(node), p.offset()),
            Err(e) => {
                // Always consume at least one whole word, so a term that failed
                // without reading anything cannot loop here.
                if p.pos <= start_pos {
                    p.pos = start_pos;
                    while matches!(p.peek(), Some(c) if !c.is_whitespace()) {
                        p.pos += 1;
                    }
                    if p.pos == start_pos {
                        p.pos += 1;
                    }
                }
                while !p.eof() && p.offset() < e.offset {
                    p.pos += 1;
                }
                (None, p.offset().max(e.offset))
            }
        };
        let end = end.min(input.len());
        // A term stops after the whitespace it skipped looking for an operator;
        // that space belongs to no term.
        let end = input[start..end].trim_end().len() + start;
        out.push(Segment { start, end, node });
    }
    out
}

struct Parser {
    chars: Vec<(usize, char)>,
    pos: usize,
}

impl Parser {
    fn eof(&self) -> bool {
        self.pos >= self.chars.len()
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|&(_, c)| c)
    }
    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).map(|&(_, c)| c)
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }
    fn offset(&self) -> usize {
        self.chars
            .get(self.pos)
            .map(|&(i, _)| i)
            .unwrap_or_else(|| {
                self.chars
                    .last()
                    .map(|&(i, c)| i + c.len_utf8())
                    .unwrap_or(0)
            })
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }
    fn err<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            message: message.into(),
            offset: self.offset(),
        })
    }
    fn expect(&mut self, c: char) -> Result<(), ParseError> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            self.err(format!("expected '{c}'"))
        }
    }

    fn term(&mut self) -> Result<Node, ParseError> {
        self.skip_ws();
        match self.peek() {
            None => self.err("unexpected end of query"),
            Some('!') => {
                self.bump();
                let inner = self.term()?;
                Ok(Node::Not(Box::new(inner)))
            }
            Some('&') if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                let list = self.term_list()?;
                Ok(Node::And(list))
            }
            Some('|') if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                let list = self.term_list()?;
                Ok(Node::Or(list))
            }
            Some('#') if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                self.numeric()
            }
            Some('/') => {
                // bare regex as free text is rare; treat as regex over blob-ish
                // fields is not supported — error clearly instead.
                self.err("regex must follow a tag (e.g. artist=/pattern/)")
            }
            Some('"') => {
                let (value, _flags) = self.quoted()?;
                Ok(Node::FreeText(value))
            }
            Some(_) => self.word_or_tagmatch(),
        }
    }

    fn term_list(&mut self) -> Result<Vec<Node>, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            items.push(self.term()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return self.err("unclosed '('"),
                _ => {} // allow space-separated terms inside &()/|() like QL
            }
        }
        Ok(items)
    }

    /// A bare word, which may turn out to be `tag=value` / `tag!=value`, or a
    /// comparison on a numeric tag (`plays>2`, `year>=2010`).
    fn word_or_tagmatch(&mut self) -> Result<Node, ParseError> {
        let mut word = self.bareword(&['=', '!', '<', '>'])?;
        // '<' and '>' start a comparison only after a numeric operand — a
        // numeric tag or a literal (`plays>2`, `1980<=year`). Anywhere else
        // they are ordinary characters of the word (titles do contain them),
        // so glue the rest of the word back on.
        while matches!(self.peek(), Some('<' | '>')) && num_operand_word(&word).is_none() {
            word.push(self.bump().unwrap());
            word.push_str(&self.bareword(&['=', '!', '<', '>'])?);
        }
        self.skip_ws();
        let operand = num_operand_word(&word);
        let is_num_tag = matches!(operand, Some(NumOperand::Tag(_)));
        match self.peek() {
            // `plays<3`, and a literal leading a range (`1980<=year<2000`).
            Some('<' | '>') if operand.is_some() => self.numeric_chain_from(operand.unwrap()),
            // `plays=3` / `plays!=3`: a numeric tag compares as a number,
            // never as a substring.
            Some('=') if is_num_tag => self.numeric_chain_from(operand.unwrap()),
            Some('!') if is_num_tag && self.peek_at(1) == Some('=') => {
                self.numeric_chain_from(operand.unwrap())
            }
            Some('=') => {
                self.bump();
                let matcher = self.value()?;
                Ok(Node::Tag {
                    tag: normalize_tag(&word),
                    matcher,
                    negated: false,
                })
            }
            Some('!') if self.peek_at(1) == Some('=') => {
                self.pos += 2;
                let matcher = self.value()?;
                Ok(Node::Tag {
                    tag: normalize_tag(&word),
                    matcher,
                    negated: true,
                })
            }
            _ => {
                if word.is_empty() {
                    self.err("unexpected character")
                } else {
                    Ok(Node::FreeText(word))
                }
            }
        }
    }

    /// Right-hand side of a tag match.
    fn value(&mut self) -> Result<Matcher, ParseError> {
        self.skip_ws();
        match self.peek() {
            Some('"') => {
                let (value, flags) = self.quoted()?;
                Ok(Matcher::Phrase {
                    value,
                    case_sensitive: flags.contains('c'),
                })
            }
            Some('/') => {
                let (pattern, flags) = self.regex()?;
                Ok(Matcher::Regex {
                    pattern,
                    case_sensitive: flags.contains('c'),
                })
            }
            Some('|') if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                Ok(Matcher::AnyOf(self.value_list()?))
            }
            Some('&') if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                Ok(Matcher::AllOf(self.value_list()?))
            }
            Some('!') => self.err("'!' is not valid here; use tag!=value"),
            _ => {
                let w = self.bareword(&[])?;
                if w.is_empty() {
                    self.err("expected a value")
                } else {
                    Ok(Matcher::Contains(w))
                }
            }
        }
    }

    fn value_list(&mut self) -> Result<Vec<Matcher>, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return self.err("unclosed '('"),
                _ => return self.err("expected ',' or ')'"),
            }
        }
        Ok(items)
    }

    /// Word ending at whitespace, ',', ')' or any of `extra_stops`.
    fn bareword(&mut self, extra_stops: &[char]) -> Result<String, ParseError> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == ',' || c == ')' || extra_stops.contains(&c) {
                break;
            }
            out.push(c);
            self.pos += 1;
        }
        Ok(out)
    }

    /// `"..."` with backslash escapes, plus trailing flags (c, d).
    fn quoted(&mut self) -> Result<(String, String), ParseError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return self.err("unterminated string"),
                Some('\\') => match self.bump() {
                    Some(c) => out.push(c),
                    None => return self.err("unterminated escape"),
                },
                Some('"') => break,
                Some(c) => out.push(c),
            }
        }
        Ok((out, self.flags()))
    }

    /// `/.../` with escapes for `/`, plus trailing flags (c, i, d).
    fn regex(&mut self) -> Result<(String, String), ParseError> {
        self.expect('/')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return self.err("unterminated regex"),
                Some('\\') => match self.bump() {
                    Some('/') => out.push('/'),
                    Some(c) => {
                        out.push('\\');
                        out.push(c);
                    }
                    None => return self.err("unterminated escape"),
                },
                Some('/') => break,
                Some(c) => out.push(c),
            }
        }
        Ok((out, self.flags()))
    }

    fn flags(&mut self) -> String {
        let mut flags = String::new();
        while let Some(c) = self.peek() {
            if c == 'c' || c == 'i' || c == 'd' {
                flags.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        flags
    }

    /// Inside `#( ... )`: chained comparisons separated by commas.
    fn numeric(&mut self) -> Result<Node, ParseError> {
        let mut exprs = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            exprs.push(self.numeric_chain()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(')') => {
                    self.bump();
                    break;
                }
                None => return self.err("unclosed '#('"),
                _ => return self.err("expected ',' or ')' in #()"),
            }
        }
        match exprs.len() {
            0 => self.err("empty #()"),
            1 => Ok(exprs.pop().unwrap()),
            _ => Ok(Node::And(exprs)),
        }
    }

    fn numeric_chain(&mut self) -> Result<Node, ParseError> {
        let first = self.num_operand()?;
        self.numeric_chain_from(first)
    }

    /// The rest of a comparison chain, with the first operand already read.
    fn numeric_chain_from(&mut self, first: NumOperand) -> Result<Node, ParseError> {
        let mut operands = vec![first];
        let mut ops = Vec::new();
        loop {
            self.skip_ws();
            let Some(op) = self.cmp_op() else { break };
            ops.push(op);
            self.skip_ws();
            operands.push(self.num_operand()?);
        }
        if ops.is_empty() {
            return self.err("expected a comparison operator");
        }
        Ok(Node::Numeric { operands, ops })
    }

    fn cmp_op(&mut self) -> Option<CmpOp> {
        let op = match (self.peek()?, self.peek_at(1)) {
            ('<', Some('=')) => (CmpOp::Le, 2),
            ('<', _) => (CmpOp::Lt, 1),
            ('>', Some('=')) => (CmpOp::Ge, 2),
            ('>', _) => (CmpOp::Gt, 1),
            ('!', Some('=')) => (CmpOp::Ne, 2),
            ('=', _) => (CmpOp::Eq, 1),
            _ => return None,
        };
        self.pos += op.1;
        Some(op.0)
    }

    /// Number, time literal (mm:ss or h:mm:ss), or tag name.
    fn num_operand(&mut self) -> Result<NumOperand, ParseError> {
        self.skip_ws();
        let word = {
            let mut out = String::new();
            while let Some(c) = self.peek() {
                if c.is_whitespace() || matches!(c, ',' | ')' | '<' | '>' | '=' | '!') {
                    break;
                }
                out.push(c);
                self.pos += 1;
            }
            out
        };
        if word.is_empty() {
            return self.err("expected a number or tag");
        }
        if let Some(secs) = parse_time_literal(&word) {
            return Ok(NumOperand::Number(secs));
        }
        if let Ok(n) = word.parse::<f64>() {
            return Ok(NumOperand::Number(n));
        }
        Ok(NumOperand::Tag(normalize_tag(&word)))
    }
}

/// The word as the left side of a bare comparison: a numeric tag, a number, or
/// a time literal. None for anything else, which keeps `<` and `>` as plain
/// text there.
fn num_operand_word(word: &str) -> Option<NumOperand> {
    if word.is_empty() {
        return None;
    }
    if let Some(secs) = parse_time_literal(word) {
        return Some(NumOperand::Number(secs));
    }
    if let Ok(n) = word.parse::<f64>() {
        return Some(NumOperand::Number(n));
    }
    let tag = normalize_tag(word);
    is_numeric_tag(&tag).then(|| NumOperand::Tag(tag))
}

/// `2:00` -> 120, `1:02:03` -> 3723.
fn parse_time_literal(s: &str) -> Option<f64> {
    if !s.contains(':') {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let mut total = 0f64;
    for p in &parts {
        let n: f64 = p.parse().ok()?;
        total = total * 60.0 + n;
    }
    Some(total)
}

fn normalize_tag(tag: &str) -> String {
    let name = tag.trim().trim_start_matches('~').to_lowercase();
    canonical_tag(&name).to_string()
}
