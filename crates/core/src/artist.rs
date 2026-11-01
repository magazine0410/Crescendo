//! Multi-artist values. One artist value often credits several artists, as in
//! `めらみぽっぷ; 七誌` or `天宮みや，姫城碧海`. The artist facet counts and
//! selects each credited artist on its own (see `FacetPlan` in queries.rs);
//! the stored value itself is never rewritten.

/// Characters that separate artists: semicolon and fullwidth semicolon,
/// fullwidth comma and ideographic comma. The ASCII comma and slash are not
/// separators because they occur inside names (`N/R`, `K2, voiced by 小峠舞`),
/// and neither is `&`, which is part of many band names.
pub const SEPARATORS: [char; 4] = [';', '；', '，', '、'];

/// Also separates artists, in any letter case and with or without spaces
/// around it (`ZYTOKINE feat. 綾倉盟`, `ガネメfeat.TaSk`).
pub const FEAT: &str = "feat.";

/// Whether `FEAT` starts at byte `i`, and not as the end of a longer Latin
/// word such as `defeat.`.
fn feat_at(value: &str, i: usize) -> bool {
    value
        .get(i..i + FEAT.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(FEAT))
        && !value[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

/// The artists one value credits, trimmed, in order, without empty parts or
/// exact repeats. A separator inside brackets does not split, so a group
/// listed with its members (`愚者トリオ（みあ、io、mikage）`) stays one artist.
/// A value with no artist text at all yields nothing.
pub fn split(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in value.char_indices() {
        if i < start {
            continue; // inside a `FEAT` just split at
        }
        match c {
            '(' | '（' => depth += 1,
            ')' | '）' => depth = depth.saturating_sub(1),
            _ if depth == 0 && SEPARATORS.contains(&c) => {
                parts.push(&value[start..i]);
                start = i + c.len_utf8();
            }
            _ if depth == 0 && feat_at(value, i) => {
                parts.push(&value[start..i]);
                start = i + FEAT.len();
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    let mut out: Vec<&str> = Vec::new();
    for part in parts.into_iter().map(str::trim) {
        if !part.is_empty() && !out.contains(&part) {
            out.push(part);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_every_separator() {
        assert_eq!(
            split("Amane; Kurosaki Sakuya"),
            ["Amane", "Kurosaki Sakuya"]
        );
        assert_eq!(
            split("かませ虎;HiZuMi；Mister Yoda"),
            ["かませ虎", "HiZuMi", "Mister Yoda"]
        );
        assert_eq!(split("天宮みや，姫城碧海"), ["天宮みや", "姫城碧海"]);
        assert_eq!(split("青砥雫、えあろげる"), ["青砥雫", "えあろげる"]);
    }

    #[test]
    fn splits_at_feat() {
        assert_eq!(
            split("IOSYS feat. ビートまりお & あまね"),
            ["IOSYS", "ビートまりお & あまね"]
        );
        assert_eq!(split("ガネメfeat.TaSk"), ["ガネメ", "TaSk"]);
        assert_eq!(split("Babbe Music Feat. Sasi"), ["Babbe Music", "Sasi"]);
        assert_eq!(split("魂音泉 FEAT. みぃ；抹"), ["魂音泉", "みぃ", "抹"]);
        // Not the end of a longer Latin word, and not inside brackets.
        assert_eq!(split("Sweet Defeat."), ["Sweet Defeat."]);
        assert_eq!(
            split("love solfege (feat. 霜月はるか)"),
            ["love solfege (feat. 霜月はるか)"]
        );
        assert_eq!(split("feat."), Vec::<&str>::new());
    }

    #[test]
    fn keeps_names_with_other_punctuation_whole() {
        for name in [
            "COOL&CREATE",
            "さゆり, Silver Forest & 五条下位",
            "N/R(えぬあーる)",
            "A／I",
            "ヘルマン・ネッケ",
            "A-One×幽閉サテライト",
            "DJ Featurecast",
        ] {
            assert_eq!(split(name), [name]);
        }
    }

    #[test]
    fn does_not_split_inside_brackets() {
        assert_eq!(
            split("愚者トリオ（みあ、io、mikage）"),
            ["愚者トリオ（みあ、io、mikage）"]
        );
        assert_eq!(
            split("霧雨 魔理沙(大瀬良あい);アリス(笑兵衛、ココ)；古明地 こいし(ココ)"),
            [
                "霧雨 魔理沙(大瀬良あい)",
                "アリス(笑兵衛、ココ)",
                "古明地 こいし(ココ)"
            ]
        );
        // A stray closing bracket does not stop later separators splitting.
        assert_eq!(split("a) b; c"), ["a) b", "c"]);
    }

    #[test]
    fn drops_empty_parts_and_exact_repeats() {
        assert_eq!(split(" 天音 ; ;天音、amane "), ["天音", "amane"]);
        assert_eq!(split("ZUN"), ["ZUN"]);
        assert!(split("").is_empty());
        assert!(split(" ; 、 ").is_empty());
    }
}
