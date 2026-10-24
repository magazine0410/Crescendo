//! Playback ordering: a play context (ordered track ids) + an ad-hoc queue
//! that always takes precedence (foobar semantics), with shuffle (permuted
//! walk order) and repeat (off / all / one).

use serde::{Deserialize, Serialize};

/// Resolve a clicked row against the concrete context being sent to playback.
/// A history write can reorder/filter the database between rendering and the
/// click. Keep duplicate playlist occurrences when the original index still
/// matches, otherwise find the same track or refuse a now-absent selection.
pub fn context_start_index(
    ids: &[i64],
    index: usize,
    track_id: Option<i64>,
) -> anyhow::Result<usize> {
    let Some(id) = track_id else { return Ok(index) };
    if ids.get(index) == Some(&id) {
        return Ok(index);
    }
    ids.iter()
        .position(|&candidate| candidate == id)
        .ok_or_else(|| anyhow::anyhow!("the selected track is no longer in this view"))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Default)]
pub struct PlayQueue {
    /// Track ids of the active context in display order.
    context: Vec<i64>,
    /// Walk order: indexes into `context` (identity, or a shuffle permutation).
    order: Vec<usize>,
    /// Position in `order` of the current track, if playing from context.
    pos: Option<usize>,
    /// Ad-hoc queued track ids; popped before the context advances.
    adhoc: Vec<i64>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// Set when the current track came from the ad-hoc queue.
    playing_adhoc: Option<i64>,
}

impl PlayQueue {
    pub fn set_context(&mut self, ids: Vec<i64>, start_index: usize) {
        self.context = ids;
        self.playing_adhoc = None;
        self.rebuild_order(Some(start_index));
        self.pos = self
            .order
            .iter()
            .position(|&i| i == start_index.min(self.context.len().saturating_sub(1)));
    }

    pub fn context_len(&self) -> usize {
        self.context.len()
    }

    /// Index into the *display* order of the current context track.
    pub fn context_index(&self) -> Option<usize> {
        if self.playing_adhoc.is_some() {
            return None;
        }
        self.pos.map(|p| self.order[p])
    }

    pub fn current(&self) -> Option<i64> {
        if let Some(id) = self.playing_adhoc {
            return Some(id);
        }
        self.pos.map(|p| self.context[self.order[p]])
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        // Keep the current track; rebuild the walk order around it.
        // An ad-hoc track hides context_index(), but pos still remembers the
        // interrupted context track. Preserve that position for resumption.
        let current_ctx_idx = self.pos.map(|p| self.order[p]);
        self.rebuild_order(current_ctx_idx);
        self.pos = current_ctx_idx.and_then(|idx| self.order.iter().position(|&i| i == idx));
    }

    fn rebuild_order(&mut self, first: Option<usize>) {
        let n = self.context.len();
        self.order = (0..n).collect();
        if self.shuffle && n > 1 {
            // Fisher-Yates with a cheap PRNG (no rand dep needed).
            let mut seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9e3779b97f4a7c15)
                | 1;
            for i in (1..n).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let j = (seed % (i as u64 + 1)) as usize;
                self.order.swap(i, j);
            }
            // Current/starting track walks first so "shuffle then play" is
            // deterministic from the chosen track.
            if let Some(first) = first {
                if let Some(fpos) = self.order.iter().position(|&i| i == first) {
                    self.order.swap(0, fpos);
                }
            }
        }
    }

    pub fn queue_add(&mut self, ids: &[i64]) {
        self.adhoc.extend_from_slice(ids);
    }

    pub fn queue_remove(&mut self, positions: &[usize]) {
        let mut keep = Vec::with_capacity(self.adhoc.len());
        for (i, id) in self.adhoc.iter().enumerate() {
            if !positions.contains(&i) {
                keep.push(*id);
            }
        }
        self.adhoc = keep;
    }

    pub fn queue_clear(&mut self) {
        self.adhoc.clear();
    }

    pub fn queue_ids(&self) -> &[i64] {
        &self.adhoc
    }

    /// Inspect the track that follows the current one without consuming it.
    pub fn peek_advance(&self) -> Option<i64> {
        if self.repeat == RepeatMode::One {
            return self.current();
        }
        if let Some(id) = self.adhoc.first() {
            return Some(*id);
        }
        let n = self.order.len();
        if n == 0 {
            return None;
        }
        let next = match self.pos {
            None => 0,
            Some(p) if p + 1 < n => p + 1,
            Some(_) if self.repeat == RepeatMode::All => 0,
            Some(_) => return None,
        };
        Some(self.context[self.order[next]])
    }

    /// The track that follows the current one; consumes queue/position state.
    pub fn advance(&mut self) -> Option<i64> {
        if self.repeat == RepeatMode::One {
            return self.current();
        }
        if !self.adhoc.is_empty() {
            let id = self.adhoc.remove(0);
            self.playing_adhoc = Some(id);
            return Some(id);
        }
        self.playing_adhoc = None;
        let n = self.order.len();
        if n == 0 {
            return None;
        }
        let next = match self.pos {
            None => 0,
            Some(p) if p + 1 < n => p + 1,
            Some(_) if self.repeat == RepeatMode::All => 0,
            Some(_) => return None,
        };
        self.pos = Some(next);
        Some(self.context[self.order[next]])
    }

    /// The track a manual Next goes to. Like [`advance`](Self::advance), except
    /// that repeat-one holds only a song that ends on its own, not one the user
    /// skips: the queue and context then walk as if repeat were off.
    pub fn skip(&mut self) -> Option<i64> {
        if self.repeat != RepeatMode::One {
            return self.advance();
        }
        self.repeat = RepeatMode::Off;
        let next = self.advance();
        self.repeat = RepeatMode::One;
        next
    }

    /// How many advances it takes before the walk reaches a track it already
    /// returned: one under repeat-one, otherwise every queued and context track.
    pub fn cycle_len(&self) -> usize {
        if self.repeat == RepeatMode::One {
            1
        } else {
            self.adhoc.len() + self.order.len()
        }
    }

    /// Previous track in the walk order (queue is not re-entered).
    pub fn previous(&mut self) -> Option<i64> {
        self.playing_adhoc = None;
        let n = self.order.len();
        if n == 0 {
            return None;
        }
        let prev = match self.pos {
            None => 0,
            Some(0) if self.repeat == RepeatMode::All => n - 1,
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.pos = Some(prev);
        Some(self.context[self.order[prev]])
    }

    /// Jump straight to a context display index (double-click on a row).
    pub fn jump_to(&mut self, display_index: usize) -> Option<i64> {
        if display_index >= self.context.len() {
            return None;
        }
        self.playing_adhoc = None;
        self.pos = self.order.iter().position(|&i| i == display_index);
        Some(self.context[display_index])
    }

    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.repeat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_takes_precedence_then_context_resumes() {
        let mut q = PlayQueue::default();
        q.set_context(vec![10, 20, 30, 40], 0);
        assert_eq!(q.current(), Some(10));
        q.queue_add(&[99, 98]);
        assert_eq!(q.advance(), Some(99)); // queue first
        assert_eq!(q.advance(), Some(98));
        assert_eq!(q.advance(), Some(20)); // context resumes where it left off
        assert_eq!(q.advance(), Some(30));
    }

    #[test]
    fn repeat_all_wraps_and_off_stops() {
        let mut q = PlayQueue::default();
        q.set_context(vec![1, 2], 1);
        assert_eq!(q.advance(), None);
        q.repeat = RepeatMode::All;
        assert_eq!(q.advance(), Some(1));
    }

    #[test]
    fn repeat_one_holds() {
        let mut q = PlayQueue::default();
        q.set_context(vec![1, 2], 0);
        q.repeat = RepeatMode::One;
        assert_eq!(q.advance(), Some(1));
        assert_eq!(q.advance(), Some(1));
    }

    #[test]
    fn skip_leaves_a_repeated_track_and_reaches_the_queue() {
        let mut q = PlayQueue::default();
        q.set_context(vec![1, 2], 0);
        q.repeat = RepeatMode::One;
        q.queue_add(&[99]);
        assert_eq!(q.skip(), Some(99));
        assert_eq!(q.skip(), Some(2));
        // Repeat-one still holds the track the skip landed on.
        assert_eq!(q.advance(), Some(2));
        assert_eq!(q.repeat, RepeatMode::One);
        // At the end of the context a skip stops, as with repeat off.
        assert_eq!(q.skip(), None);
    }

    #[test]
    fn cycle_len_counts_queue_and_context_but_one_under_repeat_one() {
        let mut q = PlayQueue::default();
        q.set_context(vec![1, 2, 3], 0);
        q.queue_add(&[99]);
        assert_eq!(q.cycle_len(), 4);
        q.repeat = RepeatMode::One;
        assert_eq!(q.cycle_len(), 1);
    }

    #[test]
    fn peeking_does_not_consume_the_next_track() {
        let mut q = PlayQueue::default();
        q.set_context(vec![10, 20, 30], 0);
        q.queue_add(&[99]);

        assert_eq!(q.peek_advance(), Some(99));
        assert_eq!(q.peek_advance(), Some(99));
        assert_eq!(q.advance(), Some(99));
        assert_eq!(q.peek_advance(), Some(20));
        assert_eq!(q.current(), Some(99));
    }

    #[test]
    fn shuffle_during_adhoc_preserves_context_resume_position() {
        let mut q = PlayQueue::default();
        q.set_context(vec![10, 20, 30, 40], 1);
        q.queue_add(&[99, 100]);
        assert_eq!(q.advance(), Some(99));
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(99));
        q.set_shuffle(false);
        assert_eq!(q.advance(), Some(100));
        assert_eq!(q.advance(), Some(30));
    }

    #[test]
    fn shuffle_keeps_current_and_visits_all() {
        let mut q = PlayQueue::default();
        q.set_context((0..50).collect(), 7);
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(7));
        let mut seen = vec![q.current().unwrap()];
        while let Some(id) = q.advance() {
            seen.push(id);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 50, "shuffle walk must visit every track once");
    }

    #[test]
    fn jump_to_updates_walk_position() {
        let mut q = PlayQueue::default();
        q.set_context(vec![5, 6, 7], 0);
        assert_eq!(q.jump_to(2), Some(7));
        assert_eq!(q.advance(), None); // end of context, repeat off
    }
}
