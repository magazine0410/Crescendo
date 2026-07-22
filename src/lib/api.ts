/**
 * Typed bridge to the Tauri backend.
 *
 * When running in a plain browser (`vite dev` without Tauri, used for UI
 * work), `isTauri` is false and calls are routed to the mock backend in
 * `mock.ts` so every view stays workable.
 */
import { invoke } from "@tauri-apps/api/core";

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface Track {
  id: number;
  path: string;
  title: string | null;
  artist: string | null;
  album: string | null;
  albumartist: string | null;
  titlesort: string | null;
  artistsort: string | null;
  albumartistsort: string | null;
  genre: string | null;
  grouping: string | null;
  track_no: number | null;
  disc_no: number | null;
  year: number | null;
  duration_ms: number;
  format: string;
  album_id: number | null;
  rating: number | null;
  play_count: number;
}

export interface Album {
  id: number;
  album: string | null;
  albumartist: string | null;
  albumartist_sort: string | null;
  year: number | null;
  genre: string | null;
  track_count: number;
  total_duration_ms: number;
}

/** One album run in the Track-list view (see Db::track_groups). */
export interface TrackGroup {
  album_id: number | null;
  album: string | null;
  artist: string | null;
  artist_sort: string | null;
  year: number | null;
  count: number;
}

export interface PlaylistInfo {
  id: number;
  name: string;
  track_count: number;
  total_duration_ms: number;
}

export interface M3uImportResult {
  playlist_id: number;
  name: string;
  added: number;
  missing: string[];
}

export interface LibraryStats {
  tracks: number;
  albums: number;
  total_duration_ms: number;
  formats: Record<string, number>;
}

export interface ScanProgress {
  scanned: number;
  added: number;
  updated: number;
  removed: number;
  errors: number;
  current_path: string;
  done: boolean;
}

export type RepeatMode = "off" | "all" | "one";

export interface PlayerState {
  status: "stopped" | "playing" | "paused";
  track: Track | null;
  context_index: number | null;
  context_len: number;
  shuffle: boolean;
  repeat: RepeatMode;
  volume: number;
  muted: boolean;
  stop_after_current: boolean;
}

export interface ArtistInfo {
  name: string;
  bio_summary: string | null;
  bio_full: string | null;
  listeners: number | null;
  playcount: number | null;
  tags: string[];
  similar: string[];
  url: string | null;
}

export interface AlbumInfo {
  name: string;
  artist: string;
  summary: string | null;
  full: string | null;
  listeners: number | null;
  playcount: number | null;
  tags: string[];
  url: string | null;
}

/** The left-panel filter category. `custom` is a user-defined single-tag filter
    (see LibraryStore.customTag); it is the only mode that carries extra data. */
export type BrowseMode = "genre" | "artist" | "album" | "grouping" | "custom";

/** Wire form of the backend `BrowseMode` enum. The four fixed modes serialize as
    their lowercase name; `custom` serializes as `{ custom: "year" }` (serde
    externally-tagged newtype variant). Never send the bare string "custom" — the
    Rust enum has no such unit variant and deserialization would fail. Build this
    with `library.wireMode`. */
export type WireBrowseMode =
  | "genre"
  | "artist"
  | "album"
  | "grouping"
  | { custom: string };

/** Context descriptor: what ordered set of tracks playback walks through. */
export type PlayContext =
  | { type: "album"; id: number }
  | { type: "playlist"; id: number }
  | { type: "library_view"; query: string | null; sort: string | null }
  | {
      type: "value_tracks";
      mode: WireBrowseMode;
      value: string;
      query: string | null;
      /** Collapse multi-theme grouping tags under their primary theme. */
      collapse: boolean;
    };

export function artUrl(
  kind: "album" | "track",
  id: number,
  size: "thumb" | "full" = "thumb",
): string {
  if (!isTauri) return "/eole/no_cover.png";
  return `crescendo-art://localhost/${kind}/${id}?size=${size}`;
}

/** Loudness-waveform peaks (0..255, ~500 samples) for a track's audio. */
export async function waveform(trackId: number): Promise<number[]> {
  return call<number[]>("waveform_get", { trackId });
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri) {
    const { mockCall } = await import("./mock");
    return mockCall<T>(cmd, args ?? {});
  }
  return invoke<T>(cmd, args);
}
