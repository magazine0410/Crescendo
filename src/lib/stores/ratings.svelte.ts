import { SvelteMap } from "svelte/reactivity";
import { call, type Track } from "$lib/api";

/** Ratings set during this session, by track id. Views keep their own copies
    of a track (library pages, album tracks, playlists), so the new value lives
    here and every copy reads it through `of` instead of being reloaded. */
class RatingStore {
  private changed = new SvelteMap<number, number | null>();
  /** The latest write per track, so an older failed write never rolls back a
      newer one. */
  private latest = new Map<number, number>();
  private writes = 0;

  /** The track's rating in stars, or null when unrated. */
  of(track: Track): number | null {
    return this.changed.has(track.id) ? (this.changed.get(track.id) ?? null) : track.rating;
  }

  /** Set a rating (0.5 to 5 in half steps), or clear it with null. Shown at
      once, and taken back if the backend refuses it. */
  async set(track: Track, rating: number | null): Promise<void> {
    const before = this.of(track);
    const write = ++this.writes;
    this.latest.set(track.id, write);
    this.changed.set(track.id, rating);
    try {
      await call("track_set_rating", { trackId: track.id, rating });
    } catch (error) {
      if (this.latest.get(track.id) === write) this.changed.set(track.id, before);
      console.error("rating not saved:", error);
    }
  }
}

export const ratings = new RatingStore();
