use super::sql::{canonical_tag, is_numeric_tag, is_time_tag, missing_column};
use super::{CmpOp, Matcher, Node, NumOperand, ParseError};

const MAX_QUERY_BYTES: usize = 16 * 1024;
const MAX_DEPTH: usize = 64;

pub fn parse(input: &str) -> Result<Node, ParseError> {
    if input.len() > MAX_QUERY_BYTES {
        return Err(ParseError {
            message: format!("query exceeds {MAX_QUERY_BYTES} bytes"),
            offset: 0,
        });
    }
    let mut p = Parser {
        chars: input.char_indices().collect(),
        pos: 0,
        depth: 0,
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
    if input.len() > MAX_QUERY_BYTES {
        return vec![Segment {
            start: 0,
            end: input.len(),
            node: None,
        }];
    }
    let mut p = Parser {
        chars: input.char_indices().collect(),
        pos: 0,
        depth: 0,
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
    depth: usize,
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
        self.nested(Self::term_inner)
    }

    fn nested<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.depth >= MAX_DEPTH {
            return self.err(format!("query nesting exceeds {MAX_DEPTH} levels"));
        }
        self.depth += 1;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    fn term_inner(&mut self) -> Result<Node, ParseError> {
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
            Some('@') if self.peek_at(1) == Some('(') => {
                self.pos += 2;
                self.extension()
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

    /// After `@(`: Quod Libet's `@(name: body)` query extensions. Only
    /// `missing` exists here: `@(missing: genre)`.
    fn extension(&mut self) -> Result<Node, ParseError> {
        self.skip_ws();
        let mut name = String::new();
        while let Some(c) = self.peek().filter(|c| c.is_alphanumeric() || *c == '_') {
            name.push(c);
            self.pos += 1;
        }
        self.skip_ws();
        if self.peek() != Some(':') {
            return self.err("expected ':' after the extension name, as in @(missing: genre)");
        }
        self.bump();
        if !name.eq_ignore_ascii_case("missing") {
            return self.err(format!(
                "unknown query extension '{name}'; only @(missing: tag) is supported"
            ));
        }
        self.skip_ws();
        let word = self.operand_word();
        if word.is_empty() {
            return self.err("expected a tag, as in @(missing: genre)");
        }
        let tag = normalize_tag(&word);
        if missing_column(&tag).is_none() {
            return self.err(format!("'{word}' is not a tag that can be missing"));
        }
        self.skip_ws();
        if self.peek() != Some(')') {
            return self.err("expected ')' after the tag");
        }
        self.bump();
        Ok(Node::Missing(tag))
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
        // so glue the rest of the word back on. So is a '!' after the start of
        // a word that does not begin `!=`: it negates only a whole term, and
        // names like `P!nk` or `Bad Apple!!` would otherwise lose their tail.
        loop {
            match self.peek() {
                Some('<' | '>') if num_operand_word(&word).is_none() => {}
                Some('!') if !word.is_empty() && self.peek_at(1) != Some('=') => {}
                _ => break,
            }
            word.push(self.bump().unwrap());
            word.push_str(&self.bareword(&['=', '!', '<', '>'])?);
        }
        self.skip_ws();
        let mut operand = num_operand_word(&word);
        let mut timed = parse_time_span(&word).is_some();
        // `1 week < added`: a span leading a range, its unit after a space.
        // Only a comparison after the unit makes it one, so `1 day` alone
        // stays text.
        if let (Some(NumOperand::Number(n)), false) = (&operand, timed) {
            let n = *n;
            let after_number = self.pos;
            let unit = time_unit(&self.operand_word());
            self.skip_ws();
            match unit {
                Some(seconds) if matches!(self.peek(), Some('<' | '>')) => {
                    operand = Some(NumOperand::Number(n * seconds));
                    timed = true;
                }
                _ => self.pos = after_number,
            }
        }
        let is_num_tag = matches!(operand, Some(NumOperand::Tag(_)));
        match self.peek() {
            // `plays<3`, and a literal leading a range (`1980<=year<2000`).
            Some('<' | '>') if operand.is_some() => self.chain_rest(operand.unwrap(), timed),
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

    /// Right-hand side of a tag match: one value, or values joined by `|` (any
    /// of them) and `&` (all of them), with `&` binding tighter, so
    /// `a|b&c` is `a` or both `b` and `c`. A `|` or `&` inside quotes or a
    /// regex is part of that value, and a space ends the list, so
    /// `a |(b, c)` stays two terms.
    fn value(&mut self) -> Result<Matcher, ParseError> {
        let first = self.all_of_values()?;
        if self.peek() != Some('|') {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.peek() == Some('|') {
            self.bump();
            items.push(self.all_of_values()?);
        }
        Ok(Matcher::AnyOf(items))
    }

    fn all_of_values(&mut self) -> Result<Matcher, ParseError> {
        let first = self.single_value()?;
        if self.peek() != Some('&') {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.peek() == Some('&') {
            self.bump();
            items.push(self.single_value()?);
        }
        Ok(Matcher::AllOf(items))
    }

    fn single_value(&mut self) -> Result<Matcher, ParseError> {
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
            Some('!') => self.err("'!' is not valid here; use tag!=value"),
            _ => {
                let w = self.bareword(&['|', '&'])?;
                if w.is_empty() {
                    self.err("expected a value")
                } else {
                    Ok(Matcher::Contains(w))
                }
            }
        }
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
        let (first, timed) = self.num_operand()?;
        self.chain_rest(first, timed)
    }

    /// The rest of a comparison chain, with the first operand already read.
    fn numeric_chain_from(&mut self, first: NumOperand) -> Result<Node, ParseError> {
        self.chain_rest(first, false)
    }

    /// `timed`: the first operand carried a unit of time.
    fn chain_rest(&mut self, first: NumOperand, mut timed: bool) -> Result<Node, ParseError> {
        let mut operands = vec![first];
        let mut ops = Vec::new();
        loop {
            self.skip_ws();
            let Some(op) = self.cmp_op() else { break };
            ops.push(op);
            self.skip_ws();
            let (operand, has_unit) = self.num_operand()?;
            timed |= has_unit;
            operands.push(operand);
        }
        if ops.is_empty() {
            return self.err("expected a comparison operator");
        }
        let with_time_tag = operands
            .iter()
            .any(|o| matches!(o, NumOperand::Tag(t) if is_time_tag(t)));
        if timed && !with_time_tag {
            return self.err("units of time compare only with added or lastplayed");
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

    /// One operand of a comparison: up to whitespace, a comma, a closing
    /// parenthesis or an operator.
    fn operand_word(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, ',' | ')' | '<' | '>' | '=' | '!') {
                break;
            }
            out.push(c);
            self.pos += 1;
        }
        out
    }

    /// Number, time literal (mm:ss or h:mm:ss), span of time (`3 days`,
    /// `3days`, in seconds), or tag name. The flag is set for a span of time.
    fn num_operand(&mut self) -> Result<(NumOperand, bool), ParseError> {
        self.skip_ws();
        let word = self.operand_word();
        if word.is_empty() {
            return self.err("expected a number or tag");
        }
        if let Some(secs) = parse_time_literal(&word) {
            return Ok((NumOperand::Number(secs), false));
        }
        if let Ok(n) = word.parse::<f64>() {
            // `3 days`: a unit may follow the number after a space.
            let after_number = self.pos;
            self.skip_ws();
            if let Some(seconds) = time_unit(&self.operand_word()) {
                return Ok((NumOperand::Number(n * seconds), true));
            }
            self.pos = after_number;
            return Ok((NumOperand::Number(n), false));
        }
        if let Some(secs) = parse_time_span(&word) {
            return Ok((NumOperand::Number(secs), true));
        }
        Ok((NumOperand::Tag(normalize_tag(&word)), false))
    }
}

/// Seconds in one unit of time, singular or plural, in any case. Months are
/// 30 days and years 365, as in Quod Libet.
fn time_unit(word: &str) -> Option<f64> {
    let word = word.to_lowercase();
    let singular = word.strip_suffix('s').unwrap_or(&word);
    Some(match singular {
        "second" => 1.0,
        "minute" => 60.0,
        "hour" => 3600.0,
        "day" => 86_400.0,
        "week" => 7.0 * 86_400.0,
        "month" => 30.0 * 86_400.0,
        "year" => 365.0 * 86_400.0,
        _ => return None,
    })
}

/// A number with its unit attached, `3days` -> 259200.
fn parse_time_span(word: &str) -> Option<f64> {
    let unit_at = word.find(|c: char| c.is_alphabetic())?;
    let n: f64 = word[..unit_at].parse().ok()?;
    Some(n * time_unit(&word[unit_at..])?)
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
    if let Some(secs) = parse_time_span(word) {
        return Some(NumOperand::Number(secs));
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
