//! Track ratings: 0.5 to 5 stars in half-star steps, stored in
//! `tracks.rating` as that number of stars. Unrated is NULL, never 0, so a
//! search such as `rating<3` does not match unrated tracks.
//!
//! Ratings live only in the database. The scanner's upsert never writes the
//! column, so a rescan keeps them.

use anyhow::{bail, Result};
use std::collections::HashMap;

pub const MIN_STARS: f64 = 0.5;
pub const MAX_STARS: f64 = 5.0;

/// Accept a rating only if it is a whole or half number of stars between
/// [`MIN_STARS`] and [`MAX_STARS`].
pub fn validate(stars: f64) -> Result<f64> {
    if !stars.is_finite() || !(MIN_STARS..=MAX_STARS).contains(&stars) {
        bail!("a rating must be between {MIN_STARS} and {MAX_STARS} stars");
    }
    if (stars * 2.0).fract() != 0.0 {
        bail!("a rating must be a whole or half number of stars");
    }
    Ok(stars)
}

/// How a track's rating weighs in weighted shuffle. Unrated and neutral
/// (3 to 3.5 stars) weigh 1, so a library with no ratings shuffles as true
/// random does. The value is always above zero.
pub fn shuffle_weight(stars: Option<f64>) -> f64 {
    match stars {
        None => 1.0,
        // 0.5 to 1.5: strongly disliked, played almost never.
        Some(s) if s < 2.0 => 0.02,
        // 2 to 2.5: slightly disliked.
        Some(s) if s < 3.0 => 0.5,
        // 3 to 3.5: neutral.
        Some(s) if s < 4.0 => 1.0,
        // 4: liked.
        Some(s) if s < 4.5 => 2.0,
        // 4.5: liked a lot.
        Some(s) if s < 5.0 => 3.0,
        // 5: loved.
        Some(_) => 4.0,
    }
}

/// Shuffle weights for a set of ratings by track id, for
/// [`PlayQueue::set_weights`](crate::playqueue::PlayQueue::set_weights).
/// Neutral ratings are left out, so a library rated only neutral shuffles as
/// true random does.
pub fn shuffle_weights(ratings: &HashMap<i64, f64>) -> HashMap<i64, f64> {
    ratings
        .iter()
        .map(|(&id, &stars)| (id, shuffle_weight(Some(stars))))
        .filter(|&(_, weight)| weight != 1.0)
        .collect()
}

/// The `shuffle_mode` preference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShuffleMode {
    /// Every order equally likely.
    #[default]
    Random,
    /// Higher-rated tracks are drawn earlier in each pass.
    Weighted,
}

impl ShuffleMode {
    pub const PREF_KEY: &'static str = "shuffle_mode";

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "random" => Some(Self::Random),
            "weighted" => Some(Self::Weighted),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_half_steps_only_within_range() {
        for ok in [0.5, 1.0, 3.5, 5.0] {
            assert_eq!(validate(ok).unwrap(), ok);
        }
        for bad in [0.0, 0.25, 3.3, 5.5, -1.0, f64::NAN, f64::INFINITY] {
            assert!(validate(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn weights_follow_the_rating_bands() {
        let w = |s| shuffle_weight(Some(s));
        assert_eq!(shuffle_weight(None), 1.0);
        assert_eq!(w(3.0), 1.0);
        assert_eq!(w(3.5), 1.0);
        assert_eq!(w(0.5), w(1.5));
        assert!(w(1.5) < w(2.0) && w(2.0) == w(2.5) && w(2.5) < w(3.0));
        assert!(w(3.5) < w(4.0) && w(4.0) < w(4.5) && w(4.5) < w(5.0));
        assert!(w(0.5) > 0.0);
    }
}
