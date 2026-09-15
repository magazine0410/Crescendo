//! ReplayGain playback policy. GStreamer supplies the file's decoded tags;
//! neither a library scan nor loudness analysis is needed to use them.

#[derive(Debug, Default, Clone, Copy)]
pub struct ReplayGain {
    pub track_gain: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_gain: Option<f64>,
    pub album_peak: Option<f64>,
}

impl ReplayGain {
    /// Track gain suits mixed/shuffled playback. Album gain is a fallback
    /// for files with only album tags. Invalid tags behave as missing tags.
    pub fn multiplier(&self) -> f64 {
        let valid_gain = |v: &f64| v.is_finite() && (-60.0..=60.0).contains(v);
        let valid_peak = |v: &f64| v.is_finite() && *v > 0.0;
        let track_peak = self.track_peak.filter(valid_peak);
        let album_peak = self.album_peak.filter(valid_peak);
        let (gain, peak) = if let Some(gain) = self.track_gain.filter(valid_gain) {
            (gain, track_peak.or(album_peak))
        } else if let Some(gain) = self.album_gain.filter(valid_gain) {
            (gain, album_peak.or(track_peak))
        } else {
            return 1.0;
        };
        let gain = 10.0_f64.powf(gain / 20.0);
        // A peak is a linear full-scale ratio, not dB. Do not boost past
        // full scale when a valid peak is available. No dynamic compression.
        match peak {
            Some(peak) => gain.min(1.0 / peak),
            None => gain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_is_opt_in_and_survives_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replaygain.db");
        {
            let db = crate::Db::open(&path).unwrap();
            assert!(!db.replaygain_enabled());
            db.prefs_set("replaygain_enabled", "true").unwrap();
        }
        {
            let db = crate::Db::open(&path).unwrap();
            assert!(db.replaygain_enabled());
            db.prefs_set("replaygain_enabled", "false").unwrap();
        }
        assert!(!crate::Db::open(&path).unwrap().replaygain_enabled());
    }

    #[test]
    fn track_gain_wins_and_album_gain_is_the_fallback() {
        let mut tags = ReplayGain {
            track_gain: Some(-6.020599913279624),
            album_gain: Some(-12.041199826559248),
            ..Default::default()
        };
        assert!((tags.multiplier() - 0.5).abs() < 1e-12);
        tags.track_gain = None;
        assert!((tags.multiplier() - 0.25).abs() < 1e-12);
        tags.album_gain = None;
        assert_eq!(tags.multiplier(), 1.0);
    }

    #[test]
    fn peak_caps_amplification_without_compressing_the_track() {
        let tags = ReplayGain {
            track_gain: Some(6.020599913279624),
            track_peak: Some(0.8),
            ..Default::default()
        };
        assert_eq!(tags.multiplier(), 1.25);
        assert_eq!(
            ReplayGain {
                track_peak: None,
                ..tags
            }
            .multiplier(),
            2.0
        );
        assert_eq!(
            ReplayGain {
                track_peak: Some(2.0),
                ..tags
            }
            .multiplier(),
            0.5
        );
        assert_eq!(
            ReplayGain {
                track_peak: None,
                album_peak: Some(0.8),
                ..tags
            }
            .multiplier(),
            1.25
        );
        // A lone peak is not a request for peak normalization.
        assert_eq!(
            ReplayGain {
                track_gain: None,
                ..tags
            }
            .multiplier(),
            1.0
        );
    }

    #[test]
    fn malformed_tags_cannot_produce_nonfinite_or_extreme_gain() {
        for invalid in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            10000.0,
            -10000.0,
        ] {
            assert_eq!(
                ReplayGain {
                    track_gain: Some(invalid),
                    ..Default::default()
                }
                .multiplier(),
                1.0
            );
        }
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
            assert_eq!(
                ReplayGain {
                    track_gain: Some(0.0),
                    track_peak: Some(invalid),
                    ..Default::default()
                }
                .multiplier(),
                1.0
            );
        }
    }
}
