//! Playback ordering: a play context (ordered track ids) + an ad-hoc queue
//! that always takes precedence (foobar semantics), with shuffle (permuted
//! walk order) and repeat (off / all / one).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// One move along the walk, chosen ahead of taking it. A context move is kept
/// as a display index, so a shuffle toggled in between does not misplace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Repeat-one: the current track again.
    Repeat,
    Queued(i64),
    Context(usize),
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
    /// Shuffle weight by track id; unlisted tracks weigh 1. Empty means every
    /// shuffled order is equally likely.
    weights: HashMap<i64, f64>,
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
        let mut order: Vec<usize> = (0..n).collect();
        if self.shuffle && n > 1 {
            self.shuffle_slice(&mut order, &mut Xorshift::from_clock());
            // Current/starting track walks first so "shuffle then play" is
            // deterministic from the chosen track.
            if let Some(first) = first {
                if let Some(fpos) = order.iter().position(|&i| i == first) {
                    if self.weights.is_empty() {
                        order.swap(0, fpos);
                    } else {
                        // A swap would send the first draw to a random place.
                        order[..=fpos].rotate_right(1);
                    }
                }
            }
        }
        self.order = order;
    }

    /// Shuffle context indexes in place: uniformly, or by weight when any
    /// track has one.
    fn shuffle_slice(&self, indexes: &mut [usize], rng: &mut Xorshift) {
        if self.weights.is_empty() {
            // Fisher-Yates.
            for i in (1..indexes.len()).rev() {
                let j = (rng.next() % (i as u64 + 1)) as usize;
                indexes.swap(i, j);
            }
        } else {
            weighted_order(indexes, |i| self.weight_of(i), rng);
        }
    }

    fn weight_of(&self, index: usize) -> f64 {
        self.weights
            .get(&self.context[index])
            .copied()
            .unwrap_or(1.0)
    }

    /// Set the shuffle weight of each track id; unlisted tracks weigh 1, and
    /// an empty map makes every order equally likely. Under shuffle, the part
    /// of the walk not played yet is drawn again with the new weights, while
    /// the tracks already played and the current one keep their places.
    pub fn set_weights(&mut self, weights: HashMap<i64, f64>) {
        self.weights = weights;
        if self.shuffle {
            let start = self.pos.map_or(0, |p| p + 1).min(self.order.len());
            let mut rest = self.order.split_off(start);
            self.shuffle_slice(&mut rest, &mut Xorshift::from_clock());
            self.order.extend(rest);
        }
    }

    pub fn queue_add(&mut self, ids: &[i64]) {
        self.adhoc.extend_from_slice(ids);
    }

    /// Remove the queued tracks a view showed as `(position, id)`. The view
    /// can lag a pop at the head, which shifts every entry left. So an
    /// id is looked for at its position and then before it, and an entry that
    /// is no longer queued is left alone rather than removing its neighbour.
    pub fn queue_remove(&mut self, items: &[(usize, i64)]) {
        let mut remove = vec![false; self.adhoc.len()];
        for &(position, id) in items {
            let Some(last) = self.adhoc.len().checked_sub(1) else {
                break;
            };
            if let Some(j) = (0..=position.min(last))
                .rev()
                .find(|&j| self.adhoc[j] == id && !remove[j])
            {
                remove[j] = true;
            }
        }
        let mut index = 0;
        self.adhoc.retain(|_| {
            index += 1;
            !remove[index - 1]
        });
    }

    pub fn queue_clear(&mut self) {
        self.adhoc.clear();
    }

    pub fn queue_ids(&self) -> &[i64] {
        &self.adhoc
    }

    /// The move [`advance`](Self::advance) would make, if there is one.
    pub fn next_step(&self) -> Option<Step> {
        self.step_after([])
    }

    /// The move that would follow `reserved`, steps chosen but not taken yet.
    pub fn step_after(&self, reserved: impl IntoIterator<Item = Step>) -> Option<Step> {
        if self.repeat == RepeatMode::One {
            return self.current().map(|_| Step::Repeat);
        }
        let mut queued = 0;
        let mut pos = self.pos;
        for step in reserved {
            match step {
                Step::Repeat => {}
                Step::Queued(_) => queued += 1,
                Step::Context(index) => pos = self.order.iter().position(|&i| i == index),
            }
        }
        if let Some(&id) = self.adhoc.get(queued) {
            return Some(Step::Queued(id));
        }
        let n = self.order.len();
        if n == 0 {
            return None;
        }
        let next = match pos {
            None => 0,
            Some(p) if p + 1 < n => p + 1,
            Some(_) if self.repeat == RepeatMode::All => 0,
            Some(_) => return None,
        };
        Some(Step::Context(self.order[next]))
    }

    /// The track a step lands on.
    pub fn track_of(&self, step: Step) -> Option<i64> {
        match step {
            Step::Repeat => self.current(),
            Step::Queued(id) => Some(id),
            Step::Context(index) => self.context.get(index).copied(),
        }
    }

    /// Inspect the track that follows the current one without consuming it.
    pub fn peek_advance(&self) -> Option<i64> {
        self.track_of(self.next_step()?)
    }

    /// Land on a step chosen earlier, which may no longer be
    /// [`next_step`](Self::next_step). A queued track removed since then takes
    /// no other entry with it.
    pub fn take_step(&mut self, step: Step) {
        match step {
            Step::Repeat => {}
            Step::Queued(id) => {
                if self.adhoc.first() == Some(&id) {
                    self.adhoc.remove(0);
                }
                self.playing_adhoc = Some(id);
            }
            Step::Context(index) => {
                self.playing_adhoc = None;
                let target = self.order.iter().position(|&i| i == index);
                // Shuffle switched on since the step was chosen rebuilt the walk
                // around the track before it. Make the step next in that walk,
                // so jumping to it leaves no track behind it unvisited.
                if let (true, Some(p), Some(t)) = (self.shuffle, self.pos, target) {
                    if t > p + 1 {
                        self.order.swap(t, p + 1);
                        self.pos = Some(p + 1);
                        return;
                    }
                }
                self.pos = target;
            }
        }
    }

    /// The track that follows the current one; consumes queue/position state.
    /// With nothing to move to, the current track stays current, so Play
    /// resumes the one that just finished.
    pub fn advance(&mut self) -> Option<i64> {
        let step = self.next_step()?;
        self.take_step(step);
        self.current()
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
        let n = self.order.len();
        if n == 0 {
            return None;
        }
        self.playing_adhoc = None;
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

/// Cheap PRNG for shuffling (no rand dependency needed).
struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn from_clock() -> Self {
        Self::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9e3779b97f4a7c15),
        )
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in (0, 1), never exactly 0 or 1.
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
}

/// Reorder `items` so that each position is drawn from the items not drawn
/// yet with probability proportional to its weight (Efraimidis-Spirakis:
/// sort by `u^(1/w)`, compared here as `ln(u) / w`). Weights must be above 0.
fn weighted_order(items: &mut [usize], weight: impl Fn(usize) -> f64, rng: &mut Xorshift) {
    let mut keyed: Vec<(f64, usize)> = items
        .iter()
        .map(|&i| (rng.unit().ln() / weight(i), i))
        .collect();
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (slot, (_, i)) in items.iter_mut().zip(keyed) {
        *slot = i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean position of `track` over many weighted orders of 0..n.
    fn mean_position(n: usize, weights: &HashMap<usize, f64>, track: usize) -> f64 {
        let runs = 4000;
        let mut total = 0;
        for seed in 0..runs {
            let mut items: Vec<usize> = (0..n).collect();
            let mut rng = Xorshift::new(seed * 0x9e37_79b9 + 1);
            weighted_order(
                &mut items,
                |i| weights.get(&i).copied().unwrap_or(1.0),
                &mut rng,
            );
            total += items.iter().position(|&i| i == track).unwrap();
        }
        total as f64 / runs as f64
    }

    #[test]
    fn weighted_order_draws_heavier_tracks_earlier() {
        let w = |s| crate::rating::shuffle_weight(Some(s));
        let weights = HashMap::from([(0, w(5.0)), (1, w(4.0)), (2, w(1.0)), (3, w(2.0))]);
        let loved = mean_position(100, &weights, 0);
        let liked = mean_position(100, &weights, 1);
        let neutral = mean_position(100, &weights, 50);
        let disliked = mean_position(100, &weights, 3);
        let hated = mean_position(100, &weights, 2);
        assert!(
            loved < liked && liked < neutral,
            "{loved} {liked} {neutral}"
        );
        assert!(
            neutral < disliked && disliked < hated,
            "{neutral} {disliked} {hated}"
        );
        assert!(hated > 90.0, "a 1-star track comes near the end: {hated}");
    }

    #[test]
    fn weighted_shuffle_keeps_current_first_and_visits_all() {
        let mut q = PlayQueue::default();
        q.set_context((0..50).collect(), 7);
        q.weights = HashMap::from([(3, 4.0), (7, 0.02), (9, 0.5)]);
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(7));
        let mut seen = vec![7];
        while let Some(id) = q.advance() {
            seen.push(id);
        }
        seen.sort();
        assert_eq!(seen, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn new_weights_redraw_only_the_unplayed_rest() {
        let mut q = PlayQueue::default();
        q.set_context((0..50).collect(), 7);
        q.set_shuffle(true);
        let mut played = vec![7];
        for _ in 0..10 {
            played.push(q.advance().unwrap());
        }
        q.set_weights(HashMap::from([(1, 4.0), (2, 0.02)]));
        assert_eq!(q.current(), played.last().copied());
        let mut previous = played.clone();
        previous.pop();
        for &id in previous.iter().rev() {
            assert_eq!(q.previous(), Some(id), "played tracks keep their places");
        }
        for _ in 0..previous.len() {
            q.advance();
        }
        let mut seen = played.clone();
        while let Some(id) = q.advance() {
            seen.push(id);
        }
        seen.sort();
        assert_eq!(
            seen,
            (0..50).collect::<Vec<_>>(),
            "every track still plays once"
        );
    }

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
    fn a_step_chosen_before_shuffle_keeps_its_track_and_the_whole_walk() {
        let mut q = PlayQueue::default();
        q.set_context((0..50).collect(), 7);
        let step = q.next_step().unwrap();
        q.set_shuffle(true);
        q.take_step(step);
        assert_eq!(q.current(), Some(8));
        assert_eq!(q.context_index(), Some(8));
        let mut seen = vec![7, 8];
        while let Some(id) = q.advance() {
            seen.push(id);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 50, "no track is left behind the jump");
    }

    #[test]
    fn queue_remove_follows_entries_shifted_by_a_pop() {
        let mut q = PlayQueue::default();
        q.set_context(vec![1], 0);
        q.queue_add(&[10, 20, 30]);
        // The view still shows [10, 20, 30] when the handoff pops 10.
        q.advance();
        q.queue_remove(&[(1, 20)]);
        assert_eq!(q.queue_ids(), [30]);
        // An entry that is gone is not replaced by its neighbour.
        q.queue_remove(&[(0, 10)]);
        assert_eq!(q.queue_ids(), [30]);
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
    fn reaching_the_end_keeps_the_track_that_just_finished_current() {
        let mut q = PlayQueue::default();
        q.set_context(vec![1, 2], 1);
        q.queue_add(&[99]);
        assert_eq!(q.advance(), Some(99));
        assert_eq!(q.advance(), None);
        assert_eq!(q.current(), Some(99), "Play resumes the queued track");

        // A queued track played with no context behind it.
        let mut q = PlayQueue::default();
        q.queue_add(&[99]);
        assert_eq!(q.advance(), Some(99));
        assert_eq!(q.previous(), None);
        assert_eq!(q.current(), Some(99));
    }

    #[test]
    fn steps_can_be_chosen_ahead_of_steps_not_taken_yet() {
        let mut q = PlayQueue::default();
        q.set_context(vec![10, 20, 30], 0);
        q.queue_add(&[99]);
        let first = q.next_step().unwrap();
        assert_eq!(first, Step::Queued(99));
        let second = q.step_after([first]).unwrap();
        assert_eq!(second, Step::Context(1));
        let third = q.step_after([first, second]).unwrap();
        assert_eq!(third, Step::Context(2));
        assert_eq!(q.step_after([first, second, third]), None);
        assert_eq!(q.current(), Some(10), "choosing moves nothing");

        q.repeat = RepeatMode::All;
        assert_eq!(q.step_after([first, second, third]), Some(Step::Context(0)));
    }

    #[test]
    fn jump_to_updates_walk_position() {
        let mut q = PlayQueue::default();
        q.set_context(vec![5, 6, 7], 0);
        assert_eq!(q.jump_to(2), Some(7));
        assert_eq!(q.advance(), None); // end of context, repeat off
    }
}
