//! Actual playback progress, independent of seek position and network state.
#[derive(Default)]
pub struct ListeningProgress {
    pub heard_ms: u64,
    previous_ms: Option<u64>,
}

impl ListeningProgress {
    /// Only credit advancing audio; jumps and long suspended intervals cannot
    /// manufacture listening time. Call reset_position after an explicit seek.
    pub fn sample(&mut self, position_ms: u64, elapsed_ms: u64, playing: bool) {
        if playing {
            if let Some(previous) = self.previous_ms {
                let delta = position_ms.saturating_sub(previous);
                if delta <= elapsed_ms.saturating_add(1000) {
                    self.heard_ms += delta.min(elapsed_ms);
                }
            }
        }
        self.previous_ms = Some(position_ms);
    }

    pub fn reset_position(&mut self) {
        self.previous_ms = None;
    }

    pub fn history_due(&self, duration_ms: u64) -> bool {
        duration_ms > 0 && self.heard_ms > duration_ms / 2
    }

    pub fn scrobble_due(&self, duration_ms: u64) -> bool {
        duration_ms > 30_000 && self.heard_ms >= (duration_ms / 2).min(240_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeks_pauses_and_stalls_do_not_count_as_listening() {
        let mut p = ListeningProgress::default();
        p.sample(0, 0, true);
        p.sample(1000, 1000, true);
        p.sample(180_000, 250, true); // unannounced position discontinuity
        p.sample(180_000, 30_000, false);
        p.reset_position(); // explicit backward seek
        p.sample(0, 250, true);
        p.sample(0, 5000, true); // buffering
        p.sample(1000, 1000, true);
        assert_eq!(p.heard_ms, 2000);
    }

    #[test]
    fn lastfm_thresholds_differ_from_local_history() {
        let mut p = ListeningProgress::default();
        p.heard_ms = 15_000;
        assert!(!p.scrobble_due(30_000));
        p.heard_ms = 120_000;
        assert!(p.scrobble_due(240_000));
        assert!(!p.history_due(240_000));
        p.heard_ms = 240_000;
        assert!(p.scrobble_due(1_200_000));
        assert!(!p.history_due(1_200_000));
    }
}
