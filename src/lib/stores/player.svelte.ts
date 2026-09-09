import { isTauri, call, type PlayerState, type Track, type RepeatMode, type PlayContext } from "../api";
import { mockNowPlaying } from "../mock";

class PlayerStore {
  status = $state<"stopped" | "playing" | "paused">("stopped");
  track = $state<Track | null>(null);
  positionMs = $state(0);
  durationMs = $state(0);
  volume = $state(0.8);
  muted = $state(false);
  shuffle = $state(false);
  repeat = $state<RepeatMode>("off");
  stopAfterCurrent = $state(false);
  contextIndex = $state<number | null>(null);
  contextLen = $state(0);
  error = $state<string | null>(null);
  /** true while the user drags the seek handle (position events pause) */
  seeking = $state(false);

  applyState(s: PlayerState) {
    this.error = null;
    this.status = s.status;
    this.track = s.track;
    this.shuffle = s.shuffle;
    this.repeat = s.repeat;
    this.stopAfterCurrent = s.stop_after_current;
    this.volume = s.volume;
    this.muted = s.muted;
    this.contextIndex = s.context_index;
    this.contextLen = s.context_len;
    if (s.track) this.durationMs = s.track.duration_ms;
    if (s.status === "stopped") {
      this.positionMs = 0;
      this.durationMs = 0;
    }
  }

  fail(message: string) {
    this.status = "stopped";
    this.track = null;
    this.positionMs = 0;
    this.durationMs = 0;
    this.stopAfterCurrent = false;
    this.contextIndex = null;
    this.contextLen = 0;
    this.seeking = false;
    this.error = message;
  }

  onPositionEvent(ms: number, durationMs: number) {
    if (this.seeking) return;
    this.positionMs = ms;
    if (durationMs > 0) this.durationMs = durationMs;
  }

  async playContext(ctx: PlayContext, startIndex: number, startTrackId?: number) {
    await call("player_play_context", { ctx, startIndex, startTrackId });
    if (!isTauri) {
      this.track = mockNowPlaying;
      this.durationMs = mockNowPlaying.duration_ms;
      this.positionMs = 0;
      this.status = "playing";
      return;
    }
  }

  /** Jump within the current context (double-click on an already-active view). */
  async jumpTo(index: number) {
    await call("player_jump_to", { index });
  }

  async togglePause() {
    if (!isTauri) {
      if (this.status === "playing") this.status = "paused";
      else if (this.track) this.status = "playing";
      return;
    }
    await call("player_toggle_pause");
  }

  async stop() {
    if (!isTauri) {
      this.status = "stopped";
      this.positionMs = 0;
      return;
    }
    await call("player_stop");
  }

  async next() {
    if (isTauri) await call("player_next");
  }

  async prev() {
    if (isTauri) await call("player_prev");
  }

  async seek(ms: number) {
    this.positionMs = ms;
    if (isTauri) await call("player_seek", { ms: Math.round(ms) });
  }

  async setVolume(v: number) {
    this.volume = Math.min(1, Math.max(0, v));
    if (this.volume > 0) this.muted = false;
    if (isTauri) await call("player_set_volume", { volume: this.volume });
  }

  async toggleMute() {
    this.muted = !this.muted;
    if (isTauri) await call("player_set_muted", { muted: this.muted });
  }

  async setShuffle(on: boolean) {
    this.shuffle = on;
    if (isTauri) await call("player_set_shuffle", { shuffle: on });
  }

  async cycleRepeat() {
    const order: RepeatMode[] = ["off", "all", "one"];
    this.repeat = order[(order.indexOf(this.repeat) + 1) % 3];
    if (isTauri) await call("player_cycle_repeat");
  }

  async toggleStopAfterCurrent() {
    this.stopAfterCurrent = !this.stopAfterCurrent;
    if (isTauri) await call("player_set_stop_after_current", { on: this.stopAfterCurrent });
  }
}

export const player = new PlayerStore();

if (!isTauri && typeof window !== "undefined") {
  // Browser-dev preview: show a paused mock track so the bar isn't empty.
  player.track = mockNowPlaying;
  player.durationMs = mockNowPlaying.duration_ms;
  player.positionMs = 53_000;
  player.status = "paused";
}
