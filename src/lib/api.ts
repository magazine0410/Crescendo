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
  composer: string | null;
  arranger: string | null;
  lyricist: string | null;
  vocalist: string | null;
  catalognumber: string | null;
  event: string | null;
  bpm: number | null;
  track_no: number | null;
  disc_no: number | null;
  year: number | null;
  duration_ms: number;
  format: string;
  album_id: number | null;
  rating: number | null;
  play_count: number;
  /** The file was not on disk at the last scan. Playlists keep such tracks
      and mark them; library views never return them. */
  missing: boolean;
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
  /** Collection this playlist belongs to, if any. */
  collection_id: number | null;
}

/** A named group of playlists in the sidebar. Counts are plain sums of the
    members, so a track in two of them counts twice. */
export interface CollectionInfo {
  id: number;
  name: string;
  collapsed: boolean;
  playlist_count: number;
  track_count: number;
  total_duration_ms: number;
}

/** One playlist's tracks, as returned per selected playlist. */
export interface PlaylistTracks {
  playlist_id: number;
  tracks: Track[];
}

export interface M3uImportResult {
  playlist_id: number;
  name: string;
  added: number;
  missing: string[];
}

/** Outcome of dropping files/folders onto a playlist. */
export interface DropImportResult {
  added: number;
  duplicates: number;
  imported: number;
  failed: string[];
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

export type PluginKind = "declarative" | "wasm";

interface PluginSettingBase {
  key: string;
  label: string;
  description: string;
  affects_projection: boolean;
}

export type PluginSetting =
  | (PluginSettingBase & { type: "boolean"; default: boolean; value: boolean })
  | (PluginSettingBase & {
      type: "string";
      default: string;
      min_length: number | null;
      max_length: number | null;
      value: string;
    })
  | (PluginSettingBase & {
      type: "number";
      default: number;
      min: number | null;
      max: number | null;
      step: number | null;
      value: number;
    })
  | (PluginSettingBase & {
      type: "select";
      default: string;
      options: { value: string; label: string }[];
      value: string;
    });

export interface PluginPendingStatus {
  operation: "update" | "uninstall";
  target_version: string | null;
}

export interface PluginInfo {
  id: string;
  name: string;
  version: string | null;
  description: string;
  authors: string[];
  license: string | null;
  homepage: string | null;
  kind: PluginKind | null;
  api_version: string | null;
  min_crescendo_version: string | null;
  package_hash: string | null;
  desired_enabled: boolean;
  loaded: boolean;
  compatible: boolean;
  compatibility_error: string | null;
  error: string | null;
  pending: PluginPendingStatus | null;
  settings: PluginSetting[];
  permissions: string[];
  network_domains: string[];
  grants_approved: boolean;
  commands: string[];
  runtime_error: string | null;
  /** The component is instantiated and its init succeeded. */
  runtime_ready: boolean;
  /** Too many runtime failures this session; the runtime refuses further
      calls. Not the same as having a `runtime_error`, which records only the
      last failure. */
  runtime_unhealthy: boolean;
  diagnostics: string[];
}

export interface PluginSnapshot {
  plugins: PluginInfo[];
  loaded_plugin_ids: string[];
  safe_mode: boolean;
  safe_mode_reason: string | null;
  errors: string[];
  restart_required: boolean;
  projection_revision: number;
  menu_controls: PluginMenuControl[];
  /** Loaded declarative plugin enabling outbound file dragging. */
  file_drag_provider: string | null;
  grouping_migration_pending: boolean;
}

export interface PluginMenuControl {
  plugin_id: string;
  id: string;
  slot: "library.filter";
  label: string;
  target_tag: string;
  action: "toggle_setting" | "run_command";
  /** Set for a toggle action only. */
  setting_key: string | null;
  /** Set for a command action only. */
  command: string | null;
  /** The tick of a toggle action; null for a command action, which shows no
      state. */
  checked: boolean | null;
}

export interface PluginMutationResult {
  changed: boolean;
  message: string | null;
  snapshot: PluginSnapshot;
}

export interface PreferenceMutationResult {
  projection_revision: number;
  projection_changed: boolean;
}

export interface BrowseValuesResult {
  projection_revision: number;
  values: BrowseValue[];
}

export interface StaleProjectionError {
  kind: "stale_projection";
  current_revision: number;
}

/** Tauri rejects serializable command errors as objects. Browser mocks throw
    the same shape so race recovery follows one path in both environments. */
export function staleProjectionRevision(reason: unknown): number | null {
  if (
    typeof reason === "object" &&
    reason !== null &&
    (reason as { kind?: unknown }).kind === "stale_projection" &&
    typeof (reason as { current_revision?: unknown }).current_revision === "number"
  ) {
    return (reason as StaleProjectionError).current_revision;
  }
  return null;
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

/** One timed line of an LRC source. */
export interface LyricLine {
  ms: number;
  /** Empty for the blank lines that mark an instrumental gap. */
  text: string;
}

export interface LyricsResult {
  /** Plain-text rendering; all an unsynced source carries. */
  text: string;
  /** Timed lines in time order, empty when the source had no timestamps. */
  lines: LyricLine[];
  source: string;
}

export interface AboutDependency {
  name: string;
  version: string;
}

export interface AboutInfo {
  version: string;
  repository_url: string;
  dependencies: AboutDependency[];
}

export interface UpdateCheck {
  current_version: string;
  available: boolean;
  latest_version: string | null;
  release_notes: string | null;
  package_name: string | null;
  downloaded: boolean;
}

export interface UpdateInstallResult {
  version: string;
  package_removed: boolean;
}

export interface ProjectMPresetFile {
  id: string;
  name: string;
  data: string;
}

export interface ProjectMPresetEntry {
  id: string;
  name: string;
  bytes: number;
}

export interface ProjectMAssetEntry {
  id: string;
  name: string;
  bytes: number;
}

export interface ProjectMImport {
  grant: string;
  folder_name: string;
  presets: ProjectMPresetEntry[];
  assets: ProjectMAssetEntry[];
  warnings: string[];
}

/** The left-panel filter category. `custom` is a user-defined single-tag filter
    (see LibraryStore.customTag); it is the only mode that carries extra data. */
export type BrowseMode =
  | "genre"
  | "artist"
  | "album"
  | "albumartist"
  | "custom";

/** Wire form of the backend `BrowseMode` enum. The fixed modes serialize as
    their lowercase name; `custom` serializes as `{ custom: "year" }` (serde
    externally-tagged newtype variant). Never send the bare string "custom" — the
    Rust enum has no such unit variant and deserialization would fail. Build this
    with `library.wireMode`. */
export type WireBrowseMode =
  | "genre"
  | "artist"
  | "album"
  | "albumartist"
  | { custom: string };

/** One projected row in the left-panel facet value list. `sort_value` is
    present only when a facet has a distinct display/sort label. */
export interface BrowseValue {
  value: string;
  count: number;
  sort_value: string | null;
}

/** User-selectable Track-list sort key (backend `SortKey`). */
export type SortKey =
  | "albumartist"
  | "artist"
  | "album"
  | "genre"
  | "grouping"
  | "year"
  | "title"
  | "duration"
  | "lastplayed";

/** Explicit Track-list sort (backend `TrackSort`); null = default LIBRARY_SORT.
    The secondary key's direction follows the primary until set on its own
    (`secondary_descending` null = follow); remaining ties fall back to the
    default order minus the chosen keys. */
export interface TrackSort {
  key: SortKey;
  secondary: SortKey | null;
  descending: boolean;
  secondary_descending: boolean | null;
}

/** Context descriptor: what ordered set of tracks playback walks through. */
export type PlayContext =
  | { type: "album"; id: number }
  | { type: "playlist"; id: number }
  | { type: "filtered_playlist"; id: number; query: string }
  /** Several playlists shown as one list; they play back to back in order. */
  | { type: "playlists"; ids: number[] }
  | { type: "filtered_playlists"; ids: number[]; query: string }
  | {
      type: "library_view";
      query: string | null;
      projection_revision: number;
      sort: TrackSort | null;
    }
  | {
      type: "value_tracks";
      mode: WireBrowseMode;
      /** The left-panel selection (one or more Ctrl-multi-selected values). */
      values: string[];
      query: string | null;
      projection_revision: number;
      sort: TrackSort | null;
    };

/** A native outbound drag can name explicit rows or let Rust resolve one of
    the same bounded context descriptors used by playback. */
export type FileDragSource =
  | { type: "track_ids"; ids: number[] }
  | { type: "context"; ctx: PlayContext };

export interface FileDragResult {
  count: number;
  truncated: boolean;
}

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
