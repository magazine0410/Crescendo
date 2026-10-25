//! Multi-genre values. A stored genre may list several genres — one tag value
//! such as `Bass House/Future House` or `其他电子，House，Trance`, or several
//! GENRE fields the scanner joined with `JOINER`. The genre facet counts and
//! selects each listed genre on its own (see `FacetPlan` in queries.rs); the
//! stored value itself is never rewritten.

/// Characters that separate genres inside one value: comma, slash and
/// semicolon, each with its fullwidth form, plus the big solidus (U+29F8).
pub const SEPARATORS: [char; 7] = [',', '，', '/', '／', '⧸', ';', '；'];

/// Joins a file's separate GENRE fields into the one stored value. It is a
/// separator itself, so the facet splits the fields apart again.
pub const JOINER: &str = "; ";

/// The genres one stored value lists, trimmed, in order, without empty parts
/// or exact repeats. A value with no genre text at all yields nothing.
pub fn split(value: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for part in value.split(SEPARATORS).map(str::trim) {
        if !part.is_empty() && !out.contains(&part) {
            out.push(part);
        }
    }
    out
}

/// The stored value for a file's GENRE fields: each trimmed, empty and
/// repeated fields dropped, joined with `JOINER`. `None` when nothing is left.
pub fn join_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut kept: Vec<&str> = Vec::new();
    for field in fields.into_iter().map(str::trim) {
        if !field.is_empty() && !kept.contains(&field) {
            kept.push(field);
        }
    }
    (!kept.is_empty()).then(|| kept.join(JOINER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_every_separator() {
        assert_eq!(
            split("其他电子，House，Trance，J-Pop"),
            ["其他电子", "House", "Trance", "J-Pop"]
        );
        assert_eq!(
            split("Bass House/Future House"),
            ["Bass House", "Future House"]
        );
        assert_eq!(split("Rock／Metal⧸Pop"), ["Rock", "Metal", "Pop"]);
        assert_eq!(
            split("Speedcore, Happy Hardcore"),
            ["Speedcore", "Happy Hardcore"]
        );
        assert_eq!(
            split("Indie;Alternative；Shoegaze"),
            ["Indie", "Alternative", "Shoegaze"]
        );
    }

    #[test]
    fn drops_empty_parts_and_exact_repeats() {
        assert_eq!(split(" Rock , ,Rock; rock "), ["Rock", "rock"]);
        assert_eq!(split("Jazz"), ["Jazz"]);
        assert!(split("").is_empty());
        assert!(split(" / ， ").is_empty());
    }

    #[test]
    fn joins_fields_with_a_separator_split_reads_back() {
        let joined = join_fields(["J-Pop", " Rock ", "", "J-Pop"]).unwrap();
        assert_eq!(joined, "J-Pop; Rock");
        assert_eq!(split(&joined), ["J-Pop", "Rock"]);
        assert_eq!(join_fields(["  ", ""]), None);
    }
}
