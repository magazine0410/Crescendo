//! The player's transport state: what the pipeline holds, the walk it is on,
//! and the gapless handoff between the two threads that change them.
//!
//! The player thread and GStreamer's streaming thread share one [`Playback`]
//! behind one lock. The rule that keeps them straight is that the queue always
//! describes the audible track. `about-to-finish` only records the step it gave
//! playbin a uri for; the queue takes that step when the new stream is heard,
//! and any explicit change simply forgets it.
//!
//! playbin3 fires `about-to-finish` when a file has been read, about two seconds
//! before it ends, and reads ahead through whatever uri it is given. So a file
//! shorter than that hands over again while the previous handoff is unheard,
//! and handoffs form a chain. Stream-starts arrive once per file, in order.

use crate::playqueue::{PlayQueue, Step};
use serde::Serialize;
use std::collections::VecDeque;

/// Stop rather than keep skipping once this many tracks in a row failed to
/// play: past that, the music folder is most likely unmounted.
pub const MAX_CONSECUTIVE_FAILURES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Its uri is set, but its stream-start has not arrived.
    Starting,
    Audible,
}

/// A track whose uri is in playbin, not heard yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handoff {
    pub track: i64,
    step: Step,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Phase {
    /// Nothing is loaded. The queue's current track is where Play resumes.
    #[default]
    Stopped,
    /// An explicit change is replacing the stream, so no handoff may start.
    /// `from` is how the outgoing track was playing while its play is credited.
    Switching { from: Status },
    Loaded {
        track: i64,
        stage: Stage,
        paused: bool,
        /// In the order playbin will reach them.
        handoffs: VecDeque<Handoff>,
    },
    /// A flushing seek with a handoff reserved would jump into the next file, so
    /// a seek then reloads the audible one and moves it once it has prerolled.
    /// No handoff may start until then: the same seek would jump into that too.
    Reloading { track: i64, paused: bool },
}

/// What a stream-start message meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStarted {
    /// A stream started by [`Playback::loaded`], or nothing new.
    Opened,
    /// The reserved track is now heard and the queue has moved to it.
    HandedOver,
    /// Stop-after-current was armed once the uri had been handed over. The
    /// caller must stop; the queue stays on the track it was armed on.
    Stop,
}

#[derive(Debug, Default)]
pub struct Playback {
    queue: PlayQueue,
    phase: Phase,
    /// Changes whenever the track that follows may have. A callback that
    /// looked up a path before a change has its handoff refused after it.
    generation: u64,
    /// One-shot "stop after this song". Declines the handoff so EOS fires;
    /// it belongs to one stream, so any explicit change clears it.
    stop_after_current: bool,
}

impl Playback {
    pub fn queue(&self) -> &PlayQueue {
        &self.queue
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn stop_after_current(&self) -> bool {
        self.stop_after_current
    }

    pub fn status(&self) -> Status {
        match self.phase {
            Phase::Stopped => Status::Stopped,
            Phase::Switching { from } => from,
            Phase::Loaded { paused: true, .. } | Phase::Reloading { paused: true, .. } => {
                Status::Paused
            }
            Phase::Loaded { paused: false, .. } | Phase::Reloading { paused: false, .. } => {
                Status::Playing
            }
        }
    }

    /// The track loaded in playbin.
    pub fn track(&self) -> Option<i64> {
        match self.phase {
            Phase::Loaded { track, .. } | Phase::Reloading { track, .. } => Some(track),
            _ => None,
        }
    }

    /// Whether playbin holds a uri not heard yet. A flushing seek now would
    /// jump into that stream rather than stay on the audible one.
    pub fn handoff_reserved(&self) -> bool {
        matches!(&self.phase, Phase::Loaded { handoffs, .. } if !handoffs.is_empty())
    }

    // ---- Streaming thread ----

    /// What follows the last track playbin already has a uri for.
    fn upcoming(&self) -> Option<Handoff> {
        let Phase::Loaded { handoffs, .. } = &self.phase else {
            return None;
        };
        let step = self
            .queue
            .step_after(handoffs.iter().map(|handoff| handoff.step))?;
        let track = match step {
            Step::Repeat => handoffs.back().map(|handoff| handoff.track),
            _ => None,
        };
        let track = track.or_else(|| self.queue.track_of(step))?;
        Some(Handoff { track, step })
    }

    /// The track `about-to-finish` should look up, with the generation to hand
    /// back to [`reserve_handoff`](Self::reserve_handoff) afterwards.
    pub fn handoff_candidate(&self) -> Option<(u64, i64)> {
        if self.stop_after_current {
            return None;
        }
        Some((self.generation, self.upcoming()?.track))
    }

    /// Record the step whose uri the caller sets while it still holds the lock.
    /// False if anything changed since the candidate was read.
    pub fn reserve_handoff(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.stop_after_current {
            return false;
        }
        let Some(upcoming) = self.upcoming() else {
            return false;
        };
        let Phase::Loaded { handoffs, .. } = &mut self.phase else {
            return false;
        };
        handoffs.push_back(upcoming);
        self.generation = self.generation.wrapping_add(1);
        true
    }

    // ---- Player thread: bus messages ----

    pub fn stream_started(&mut self) -> StreamStarted {
        let stop_armed = self.stop_after_current;
        let Phase::Loaded {
            track,
            stage,
            handoffs,
            ..
        } = &mut self.phase
        else {
            return StreamStarted::Opened;
        };
        // A short file can be read to its end, and hand over, before its own
        // stream-start is popped. That message belongs to the stage, not the
        // handoff, which then waits for the next one.
        if *stage == Stage::Starting {
            *stage = Stage::Audible;
            return StreamStarted::Opened;
        }
        let Some(reserved) = handoffs.pop_front() else {
            return StreamStarted::Opened;
        };
        if stop_armed {
            self.begin_switch();
            return StreamStarted::Stop;
        }
        *track = reserved.track;
        self.queue.take_step(reserved.step);
        StreamStarted::HandedOver
    }

    /// EOS, reached without a handoff. The track to start, or None to stop.
    pub fn ended(&mut self) -> Option<i64> {
        let stop_armed = self.stop_after_current;
        self.begin_switch();
        if stop_armed {
            return None;
        }
        self.queue.advance()
    }

    /// A playback error, the `failures`th in a row. Where it moves to, or None
    /// to stop because every track the walk can reach next has just failed.
    /// Without this, repeat-one retried a broken file forever and repeat-all
    /// cycled the whole context while the drive was unmounted.
    pub fn failed(&mut self, failures: usize) -> Option<i64> {
        self.begin_switch();
        let limit = self.queue.cycle_len().clamp(1, MAX_CONSECUTIVE_FAILURES);
        if failures >= limit {
            return None;
        }
        self.queue.advance()
    }

    // ---- Player thread: commands ----

    /// Each of these that returns a track has entered [`Phase::Switching`]; the
    /// caller then either loads it or stops, which are the only ways out.
    fn begin_switch(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.stop_after_current = false;
        self.phase = Phase::Switching {
            from: self.status(),
        };
    }

    pub fn play_context(&mut self, ids: Vec<i64>, start_index: usize) -> Option<i64> {
        if ids.is_empty() {
            return None;
        }
        self.begin_switch();
        self.queue.set_context(ids, start_index);
        self.queue.current()
    }

    /// Out of range changes nothing, not even a handoff in flight: playbin
    /// already holds its uri.
    pub fn jump_to(&mut self, display_index: usize) -> Option<i64> {
        if display_index >= self.queue.context_len() {
            return None;
        }
        self.begin_switch();
        self.queue.jump_to(display_index)
    }

    /// Play from stopped: the resume point, else the top of the context.
    pub fn play_from_stopped(&mut self) -> Option<i64> {
        let id = self.queue.current().or_else(|| self.queue.advance())?;
        self.begin_switch();
        Some(id)
    }

    /// Manual Next: leaves a repeated track, and starts what the handoff would
    /// have, because both choose from the audible track.
    pub fn next(&mut self) -> Option<i64> {
        self.begin_switch();
        self.queue.skip()
    }

    pub fn previous(&mut self) -> Option<i64> {
        self.begin_switch();
        self.queue.previous()
    }

    pub fn begin_stop(&mut self) {
        self.begin_switch();
    }

    pub fn stopped(&mut self) {
        self.phase = Phase::Stopped;
    }

    /// Call while holding the lock across setting the uri, so a late callback
    /// cannot land a stale uri after it.
    pub fn loaded(&mut self, track: i64) {
        self.phase = Phase::Loaded {
            track,
            stage: Stage::Starting,
            paused: false,
            handoffs: VecDeque::new(),
        };
    }

    pub fn set_paused(&mut self, on: bool) {
        if let Phase::Loaded { paused, .. } | Phase::Reloading { paused, .. } = &mut self.phase {
            *paused = on;
        }
    }

    /// Start reloading the audible track, forgetting its handoffs. The track to
    /// load; the caller sets its uri, and calls [`reloaded`](Self::reloaded)
    /// after moving it. Stop-after-current stands, since this is the same song.
    pub fn begin_reload(&mut self) -> Option<i64> {
        let Phase::Loaded { track, paused, .. } = self.phase else {
            return None;
        };
        self.generation = self.generation.wrapping_add(1);
        self.phase = Phase::Reloading { track, paused };
        Some(track)
    }

    /// Whether the caller should resume playing the moved stream.
    pub fn reloaded(&mut self) -> bool {
        let Phase::Reloading { track, paused } = self.phase else {
            return false;
        };
        self.phase = Phase::Loaded {
            track,
            stage: Stage::Audible,
            paused,
            handoffs: VecDeque::new(),
        };
        !paused
    }

    /// Repeat, shuffle and queue changes. Handoffs already reserved stand,
    /// since playbin holds their uris; one still being looked up is refused and
    /// the track ends through EOS instead.
    pub fn edit_queue<T>(&mut self, edit: impl FnOnce(&mut PlayQueue) -> T) -> T {
        self.generation = self.generation.wrapping_add(1);
        edit(&mut self.queue)
    }

    pub fn set_stop_after_current(&mut self, on: bool) {
        self.generation = self.generation.wrapping_add(1);
        self.stop_after_current = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playqueue::RepeatMode;

    /// Playing `ids[start]`, its stream started.
    fn playing(ids: &[i64], start: usize) -> Playback {
        let mut p = Playback::default();
        let id = p.play_context(ids.to_vec(), start).unwrap();
        p.loaded(id);
        assert_eq!(p.stream_started(), StreamStarted::Opened);
        p
    }

    /// `about-to-finish`, undisturbed. Returns the track it handed over.
    fn reserve(p: &mut Playback) -> i64 {
        let (generation, id) = p.handoff_candidate().unwrap();
        assert!(p.reserve_handoff(generation));
        id
    }

    #[test]
    fn the_queue_names_the_audible_track_until_the_handoff_is_heard() {
        let mut p = playing(&[10, 20], 0);
        p.edit_queue(|q| q.queue_add(&[99]));
        assert_eq!(reserve(&mut p), 99);

        // What a publish in this window reports.
        assert_eq!(p.queue().current(), Some(10));
        assert_eq!(p.queue().context_index(), Some(0));
        assert_eq!(p.queue().queue_ids(), [99]);
        assert_eq!(p.track(), Some(10));

        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.queue().current(), Some(99));
        assert_eq!(p.queue().queue_ids(), [0_i64; 0]);
        assert_eq!(p.track(), Some(99));
    }

    #[test]
    fn explicit_changes_during_a_handoff_start_from_the_audible_track() {
        let mut p = playing(&[10, 20, 30], 1);
        reserve(&mut p);
        assert_eq!(p.next(), Some(30), "Next starts what the handoff chose");

        let mut p = playing(&[10, 20, 30], 1);
        reserve(&mut p);
        assert_eq!(p.previous(), Some(10), "Prev goes back from what was heard");

        let mut p = playing(&[10, 20, 30], 1);
        p.edit_queue(|q| q.queue_add(&[99]));
        reserve(&mut p);
        p.begin_stop();
        p.stopped();
        assert_eq!(p.queue().current(), Some(20));
        assert_eq!(p.queue().queue_ids(), [99], "the queued track stays queued");
        assert_eq!(p.play_from_stopped(), Some(20));
    }

    #[test]
    fn a_repeat_change_during_a_handoff_cannot_lose_or_replay_a_track() {
        // All, reserved 20, then One: Next must reach 20, not skip over it.
        let mut p = playing(&[10, 20, 30], 0);
        p.edit_queue(|q| q.repeat = RepeatMode::All);
        reserve(&mut p);
        p.edit_queue(|q| q.repeat = RepeatMode::One);
        assert_eq!(p.next(), Some(20));

        // One, reserved 10 again, then Off: Next must leave 10.
        let mut p = playing(&[10, 20, 30], 0);
        p.edit_queue(|q| q.repeat = RepeatMode::One);
        assert_eq!(reserve(&mut p), 10);
        p.edit_queue(|q| q.repeat = RepeatMode::Off);
        assert_eq!(p.next(), Some(20));
    }

    #[test]
    fn next_under_repeat_one_leaves_the_track_but_the_handoff_repeats_it() {
        let mut p = playing(&[10, 20], 0);
        p.edit_queue(|q| q.repeat = RepeatMode::One);
        assert_eq!(reserve(&mut p), 10);
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.queue().current(), Some(10));

        assert_eq!(reserve(&mut p), 10);
        assert_eq!(p.next(), Some(20));
    }

    #[test]
    fn a_cleared_queue_does_not_regain_the_reserved_entry() {
        let mut p = playing(&[10, 20], 0);
        p.edit_queue(|q| q.queue_add(&[99]));
        reserve(&mut p);
        p.edit_queue(PlayQueue::queue_clear);
        p.begin_stop();
        p.stopped();
        assert_eq!(p.queue().queue_ids(), [0_i64; 0]);
    }

    #[test]
    fn a_reserved_entry_removed_from_the_queue_still_plays_and_takes_no_other() {
        let mut p = playing(&[10, 20], 0);
        p.edit_queue(|q| q.queue_add(&[99, 98]));
        reserve(&mut p);
        p.edit_queue(|q| q.queue_remove(&[(0, 99)]));
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(
            p.queue().current(),
            Some(99),
            "playbin already holds its uri"
        );
        assert_eq!(p.queue().queue_ids(), [98]);
    }

    #[test]
    fn short_files_chain_handoffs_and_each_is_taken_when_heard() {
        let mut p = playing(&[10, 20, 30, 40], 0);
        p.edit_queue(|q| q.queue_add(&[99]));
        assert_eq!(reserve(&mut p), 99);
        assert_eq!(reserve(&mut p), 20);
        assert_eq!(reserve(&mut p), 30);
        assert_eq!(p.queue().current(), Some(10));
        assert_eq!(p.queue().queue_ids(), [99]);

        for expected in [99, 20, 30] {
            assert_eq!(p.stream_started(), StreamStarted::HandedOver);
            assert_eq!(p.queue().current(), Some(expected));
            assert_eq!(p.track(), Some(expected));
        }
        assert_eq!(p.queue().queue_ids(), [0_i64; 0]);
        assert_eq!(p.stream_started(), StreamStarted::Opened);
    }

    #[test]
    fn a_chain_ends_where_the_walk_does() {
        let mut p = playing(&[10, 20], 0);
        assert_eq!(reserve(&mut p), 20);
        assert_eq!(p.handoff_candidate(), None);
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.handoff_candidate(), None);
    }

    #[test]
    fn explicit_changes_forget_a_whole_chain() {
        let mut p = playing(&[10, 20, 30, 40], 0);
        reserve(&mut p);
        reserve(&mut p);
        assert_eq!(p.next(), Some(20));
        p.loaded(20);
        assert_eq!(p.stream_started(), StreamStarted::Opened);
        assert_eq!(
            p.stream_started(),
            StreamStarted::Opened,
            "no stale handoff"
        );
        assert_eq!(p.queue().current(), Some(20));
    }

    #[test]
    fn repeat_one_set_during_a_chain_repeats_the_track_it_follows() {
        let mut p = playing(&[10, 20, 30], 0);
        assert_eq!(reserve(&mut p), 20);
        p.edit_queue(|q| q.repeat = RepeatMode::One);
        assert_eq!(reserve(&mut p), 20, "20 is what is being read");
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.queue().current(), Some(20));
    }

    #[test]
    fn a_short_track_can_hand_over_before_its_own_stream_start() {
        let mut p = Playback::default();
        let id = p.play_context(vec![10, 20], 0).unwrap();
        p.loaded(id);
        assert_eq!(reserve(&mut p), 20);

        assert_eq!(p.stream_started(), StreamStarted::Opened);
        assert_eq!(p.queue().current(), Some(10), "that message was track 10's");
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.queue().current(), Some(20));
    }

    #[test]
    fn a_callback_cannot_outlive_the_state_it_read() {
        // A queue edit during the path lookup.
        let mut p = playing(&[10, 20], 0);
        let (generation, _) = p.handoff_candidate().unwrap();
        p.edit_queue(|q| q.queue_add(&[99]));
        assert!(!p.reserve_handoff(generation));

        // An explicit change during the lookup, finished or not.
        let mut p = playing(&[10, 20, 30], 0);
        let (generation, _) = p.handoff_candidate().unwrap();
        let id = p.next().unwrap();
        assert!(p.handoff_candidate().is_none(), "barred while switching");
        assert!(!p.reserve_handoff(generation));
        p.loaded(id);
        assert!(!p.reserve_handoff(generation));
        assert!(p.handoff_candidate().is_some(), "open again once loaded");

        // A callback that lost to another: the uri it looked up is taken.
        let mut p = playing(&[10, 20, 30], 0);
        let (generation, _) = p.handoff_candidate().unwrap();
        assert!(p.reserve_handoff(generation));
        assert!(!p.reserve_handoff(generation));

        // A handoff heard during the lookup does not move what follows.
        let mut p = playing(&[10, 20, 30], 0);
        reserve(&mut p);
        let (generation, id) = p.handoff_candidate().unwrap();
        assert_eq!(id, 30);
        p.stream_started();
        assert!(p.reserve_handoff(generation));
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.queue().current(), Some(30));

        let mut p = Playback::default();
        p.edit_queue(|q| q.set_context(vec![10, 20], 0));
        assert!(p.handoff_candidate().is_none(), "nothing is loaded");
    }

    #[test]
    fn stop_after_current_before_the_handoff_declines_it_and_stops_at_eos() {
        let mut p = playing(&[10, 20], 0);
        p.set_stop_after_current(true);
        assert!(p.handoff_candidate().is_none());
        assert_eq!(p.ended(), None);
        assert!(!p.stop_after_current());
        assert_eq!(p.queue().current(), Some(10));
    }

    #[test]
    fn stop_after_current_after_the_handoff_stops_on_the_armed_track() {
        let mut p = playing(&[10, 20], 0);
        p.edit_queue(|q| q.queue_add(&[99]));
        reserve(&mut p);
        p.set_stop_after_current(true);
        assert_eq!(p.stream_started(), StreamStarted::Stop);
        assert!(!p.stop_after_current());
        assert_eq!(p.queue().current(), Some(10));
        assert_eq!(p.queue().queue_ids(), [99]);
    }

    #[test]
    fn stop_after_current_waits_for_the_handoff_not_a_short_tracks_own_start() {
        let mut p = Playback::default();
        let id = p.play_context(vec![10, 20], 0).unwrap();
        p.loaded(id);
        reserve(&mut p);
        p.set_stop_after_current(true);
        assert_eq!(p.stream_started(), StreamStarted::Opened);
        assert!(p.stop_after_current());
        assert_eq!(p.stream_started(), StreamStarted::Stop);
    }

    #[test]
    fn stop_after_current_belongs_to_one_stream() {
        let mut p = playing(&[10, 20, 30], 0);
        p.set_stop_after_current(true);
        assert_eq!(p.next(), Some(20));
        assert!(!p.stop_after_current());

        let mut p = playing(&[10, 20, 30], 0);
        p.set_stop_after_current(true);
        assert_eq!(p.failed(1), Some(20), "an error moves on as usual");
        assert!(!p.stop_after_current());
    }

    #[test]
    fn eos_without_a_handoff_walks_on_or_stops() {
        let mut p = playing(&[10, 20], 0);
        assert_eq!(p.ended(), Some(20));
        p.loaded(20);
        assert_eq!(p.ended(), None);
        assert_eq!(p.queue().current(), Some(20));
    }

    #[test]
    fn giving_up_during_a_handoff_keeps_the_queue_as_it_was() {
        let mut p = playing(&[10], 0);
        p.edit_queue(|q| q.queue_add(&[99]));
        reserve(&mut p);
        assert_eq!(p.failed(2), None);
        p.begin_stop();
        p.stopped();
        assert_eq!(p.queue().current(), Some(10));
        assert_eq!(p.queue().queue_ids(), [99]);
    }

    #[test]
    fn a_failing_track_is_not_retried_forever() {
        let mut p = playing(&[10, 20, 30], 0);
        p.edit_queue(|q| q.repeat = RepeatMode::One);
        assert_eq!(p.failed(1), None);

        p.edit_queue(|q| q.repeat = RepeatMode::All);
        assert_eq!(p.failed(1), Some(20));
        assert_eq!(p.failed(2), Some(30));
        // Every track has now failed once; wrapping would start the loop over.
        assert_eq!(p.failed(3), None);

        let mut p = playing(&(0..1000).collect::<Vec<_>>(), 0);
        assert_eq!(p.failed(MAX_CONSECUTIVE_FAILURES - 1), Some(1));
        assert_eq!(p.failed(MAX_CONSECUTIVE_FAILURES), None);
    }

    #[test]
    fn status_follows_the_phase() {
        let mut p = Playback::default();
        assert_eq!(p.status(), Status::Stopped);
        let id = p.play_context(vec![10, 20], 0).unwrap();
        p.loaded(id);
        assert_eq!(p.status(), Status::Playing);
        p.set_paused(true);
        assert_eq!(p.status(), Status::Paused);

        // The outgoing track is credited as it was playing.
        p.begin_stop();
        assert_eq!(p.status(), Status::Paused);
        assert_eq!(p.track(), None);
        p.stopped();
        assert_eq!(p.status(), Status::Stopped);
        p.set_paused(true);
        assert_eq!(p.status(), Status::Stopped);
    }

    #[test]
    fn a_reload_forgets_handoffs_and_bars_new_ones_until_it_has_moved() {
        let mut p = playing(&[10, 20, 30], 0);
        p.edit_queue(|q| q.queue_add(&[99]));
        reserve(&mut p);
        reserve(&mut p);
        let (stale, _) = p.handoff_candidate().unwrap();

        assert_eq!(p.begin_reload(), Some(10));
        assert!(!p.handoff_reserved());
        assert_eq!(p.handoff_candidate(), None);
        assert!(!p.reserve_handoff(stale));
        assert_eq!(p.track(), Some(10));
        assert_eq!(p.queue().current(), Some(10));
        assert_eq!(p.queue().queue_ids(), [99], "still queued, not skipped");
        assert_eq!(p.status(), Status::Playing);

        // The reloaded file's own stream-start, read during the preroll.
        assert_eq!(p.stream_started(), StreamStarted::Opened);
        assert_eq!(p.queue().current(), Some(10));

        assert!(p.reloaded());
        assert_eq!(
            reserve(&mut p),
            99,
            "the walk carries on from the same place"
        );
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);
        assert_eq!(p.queue().current(), Some(99));
    }

    #[test]
    fn a_reload_keeps_the_pause_state_and_follows_a_change_to_it() {
        let mut p = playing(&[10, 20], 0);
        p.set_paused(true);
        p.begin_reload();
        assert_eq!(p.status(), Status::Paused);
        assert!(!p.reloaded(), "stays paused");

        let mut p = playing(&[10, 20], 0);
        p.begin_reload();
        p.set_paused(true);
        assert_eq!(p.status(), Status::Paused);
        p.set_paused(false);
        assert!(p.reloaded());
        assert_eq!(p.status(), Status::Playing);
    }

    #[test]
    fn stop_after_current_survives_a_reload_of_the_same_song() {
        let mut p = playing(&[10, 20], 0);
        reserve(&mut p);
        p.set_stop_after_current(true);
        p.begin_reload();
        p.reloaded();
        assert!(p.stop_after_current());
        assert_eq!(p.handoff_candidate(), None, "declines, so EOS will stop");
        assert_eq!(p.ended(), None);
        assert_eq!(p.queue().current(), Some(10));
    }

    #[test]
    fn explicit_changes_during_a_reload_win() {
        let mut p = playing(&[10, 20, 30], 0);
        p.begin_reload();
        assert_eq!(p.next(), Some(20));
        assert!(!p.reloaded(), "nothing left to move");
        p.loaded(20);
        assert_eq!(p.track(), Some(20));

        let mut p = playing(&[10, 20], 0);
        p.begin_reload();
        p.begin_stop();
        p.stopped();
        assert_eq!(p.status(), Status::Stopped);
        assert_eq!(p.begin_reload(), None, "nothing is loaded");
    }

    #[test]
    fn empty_and_out_of_range_requests_change_nothing() {
        let mut p = playing(&[10, 20], 0);
        reserve(&mut p);
        assert_eq!(p.jump_to(2), None);
        assert_eq!(p.play_context(Vec::new(), 0), None);
        assert_eq!(p.stream_started(), StreamStarted::HandedOver);

        let mut p = Playback::default();
        assert_eq!(p.play_from_stopped(), None);
        assert_eq!(p.phase(), &Phase::Stopped);
    }
}
