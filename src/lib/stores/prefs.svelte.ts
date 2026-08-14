import { isTauri, call } from "../api";

/** Sidebar order of the playlist list. "manual" = the stored order. */
export type PlaylistSort = "manual" | "name" | "count";

/** Where collections sit in the playlist sidebar: gathered above the loose
    playlists under a separator, sorted in among them by name, or gathered
    below them under a separator. */
export type CollectionsPlacement = "above" | "within" | "below";

export type VisualizerMode = "bars" | "projectm_en_d" | "projectm_preset" | "projectm_import";

/** Keys mirrored into the backend `prefs` table. */
export interface Prefs {
  library_path: string;
  lastfm_api_key: string;
  /** Send the playing track's title/artist/album/duration to VocaDB and
      LRCLIB when no local lyrics exist (Now Playing). Off keeps lyrics to
      sidecar files and embedded tags only. */
  lyrics_online: boolean;
  /** Substitute the romanized *sort tag wherever a title or artist that
      contains CJK text renders (see format.ts). Off shows the original tags. */
  prefer_romanized: boolean;
  titlebar_format: string;
  track_detail_format: string;
  /** Optional smaller second line under track_detail (same %tag% syntax).
      Blank = single-line rows, unchanged from before. */
  track_detail_format_2: string;
  scan_threads: number;
  show_waveform: boolean;
  library_view: "album_wall" | "track_list";
  /** Which tab opens on launch. */
  default_tab: "library" | "playlists";
  track_separators: boolean;
  /** Custom GROUPING filter: show each full (semicolon-joined) tag as its own row
      instead of collapsing multi-theme tags under their primary theme. */
  grouping_split_multi: boolean;
  /** Raw text of the user-defined "custom" left-panel filter, e.g. `%year%`.
      Parsed to a single tag in LibraryStore; blank = unset (tab reads "CUSTOM"). */
  custom_browse_tag: string;
  /** Bar color for the Visualizer tab, `#rrggbb`. Defaults to the theme accent. */
  visualizer_color: string;
  /** Active Visualizer-tab engine/source. */
  visualizer_mode: VisualizerMode;
  /** Bundled preset id, or imported preset id for the chosen/imported modes. */
  visualizer_preset: string;
  /** Last user-selected MilkDrop folder, reloaded when the import mode returns. */
  visualizer_import_path: string;
  /** Track-list sort, encoded `key:secondary:asc|desc` (see library store's
      parseTrackSort). Blank = default LIBRARY_SORT order. */
  track_sort: string;
  /** Last sort applied to the playlist sidebar. */
  playlist_sort: PlaylistSort;
  /** Keep `playlist_sort` applied, so playlists made later sort into place
      instead of landing at the end. */
  playlist_sort_locked: boolean;
  /** Where collections are shown in the playlist sidebar. */
  collections_placement: CollectionsPlacement;
}

const DEFAULTS: Prefs = {
  library_path: "/mnt/Mythic_Meme/Music/",
  lastfm_api_key: "",
  lyrics_online: true,
  prefer_romanized: true,
  titlebar_format: "%albumartist% - %title%",
  track_detail_format: "",
  track_detail_format_2: "",
  scan_threads: 4,
  show_waveform: false,
  library_view: "album_wall",
  default_tab: "playlists",
  track_separators: false,
  grouping_split_multi: false,
  custom_browse_tag: "",
  visualizer_color: "#ffaf32", // == --accent (theme orange)
  visualizer_mode: "bars",
  visualizer_preset: "",
  visualizer_import_path: "",
  track_sort: "",
  playlist_sort: "manual",
  playlist_sort_locked: false,
  collections_placement: "above",
};

/** `%title_display%` -> `%title%` for the three retired _display tags. */
function foldDisplayTags(pattern: string): string {
  return pattern.replace(/%(title|artist|albumartist)_display%/gi, "%$1%");
}

class PrefsStore {
  values = $state<Prefs>({ ...DEFAULTS });
  loaded = $state(false);

  async load() {
    if (!isTauri) {
      this.loaded = true;
      return;
    }
    const raw = await call<Record<string, string>>("prefs_get_all");
    const v: Prefs = { ...DEFAULTS };
    if (raw.library_path) v.library_path = raw.library_path;
    if (raw.lastfm_api_key) v.lastfm_api_key = raw.lastfm_api_key;
    if (raw.lyrics_online) v.lyrics_online = raw.lyrics_online === "true";
    if (raw.prefer_romanized) v.prefer_romanized = raw.prefer_romanized === "true";
    // %title_display% and friends became the prefer_romanized toggle: fold any
    // stored pattern back onto the plain tags, which now follow that toggle.
    for (const key of ["titlebar_format", "track_detail_format", "track_detail_format_2"] as const) {
      if (!raw[key]) continue;
      const folded = foldDisplayTags(raw[key]);
      v[key] = folded;
      if (folded !== raw[key]) await call("prefs_set", { key, value: folded });
    }
    if (raw.scan_threads) v.scan_threads = parseInt(raw.scan_threads, 10) || 4;
    if (raw.show_waveform) v.show_waveform = raw.show_waveform === "true";
    if (raw.library_view === "track_list" || raw.library_view === "album_wall")
      v.library_view = raw.library_view;
    if (raw.default_tab === "library" || raw.default_tab === "playlists")
      v.default_tab = raw.default_tab;
    if (raw.track_separators) v.track_separators = raw.track_separators === "true";
    if (raw.grouping_split_multi)
      v.grouping_split_multi = raw.grouping_split_multi === "true";
    if (raw.custom_browse_tag) {
      // ALBUM ARTIST became a fixed tab; migrate that former custom choice to
      // GROUPING so existing five-tab layouts become the intended direct swap.
      const custom = raw.custom_browse_tag.trim().toLowerCase();
      if (custom === "albumartist" || custom === "%albumartist%") {
        v.custom_browse_tag = "%grouping%";
        await call("prefs_set", { key: "custom_browse_tag", value: "%grouping%" });
      } else {
        v.custom_browse_tag = raw.custom_browse_tag;
      }
    }
    if (raw.visualizer_color) v.visualizer_color = raw.visualizer_color;
    if (
      raw.visualizer_mode === "bars" ||
      raw.visualizer_mode === "projectm_en_d" ||
      raw.visualizer_mode === "projectm_preset" ||
      raw.visualizer_mode === "projectm_import"
    )
      v.visualizer_mode = raw.visualizer_mode;
    if (raw.visualizer_preset) v.visualizer_preset = raw.visualizer_preset;
    if (raw.visualizer_import_path) v.visualizer_import_path = raw.visualizer_import_path;
    if (raw.track_sort) v.track_sort = raw.track_sort;
    if (raw.playlist_sort === "name" || raw.playlist_sort === "count")
      v.playlist_sort = raw.playlist_sort;
    if (raw.playlist_sort_locked) v.playlist_sort_locked = raw.playlist_sort_locked === "true";
    // "inline" was the earlier name of "above": collections first, but with
    // no separator. Read it as "above" so existing sidebars keep their shape.
    const placement =
      raw.collections_placement === "inline" ? "above" : raw.collections_placement;
    if (placement === "above" || placement === "within" || placement === "below")
      v.collections_placement = placement;
    this.values = v;
    this.loaded = true;
  }

  async set<K extends keyof Prefs>(key: K, value: Prefs[K]) {
    this.values[key] = value;
    // Also in the browser: mock.ts mirrors the prefs the dev backend needs
    // (prefer_romanized decides its ordering), and ignores the rest.
    await call("prefs_set", { key, value: String(value) });
  }

  /** UI layout numbers, persisted with a shared prefix. */
  async setLayout(key: string, px: number) {
    if (isTauri) await call("prefs_set", { key: `layout.${key}`, value: String(px) });
  }

  async getLayout(): Promise<Record<string, number>> {
    if (!isTauri) return {};
    const raw = await call<Record<string, string>>("prefs_get_all");
    const out: Record<string, number> = {};
    for (const [k, v] of Object.entries(raw)) {
      if (k.startsWith("layout.")) {
        const n = parseFloat(v);
        if (!Number.isNaN(n)) out[k.slice(7)] = n;
      }
    }
    return out;
  }
}

export const prefs = new PrefsStore();
