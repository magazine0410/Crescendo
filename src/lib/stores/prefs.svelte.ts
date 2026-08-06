import { isTauri, call } from "../api";

/** Keys mirrored into the backend `prefs` table. */
export interface Prefs {
  library_path: string;
  lastfm_api_key: string;
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
  /** Track-list sort, encoded `key:secondary:asc|desc` (see library store's
      parseTrackSort). Blank = default LIBRARY_SORT order. */
  track_sort: string;
}

const DEFAULTS: Prefs = {
  library_path: "/mnt/Mythic_Meme/Music/",
  lastfm_api_key: "",
  titlebar_format: "%albumartist_display% - %title_display%",
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
  track_sort: "",
};

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
    if (raw.titlebar_format) v.titlebar_format = raw.titlebar_format;
    if (raw.track_detail_format) v.track_detail_format = raw.track_detail_format;
    if (raw.track_detail_format_2) v.track_detail_format_2 = raw.track_detail_format_2;
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
    if (raw.track_sort) v.track_sort = raw.track_sort;
    this.values = v;
    this.loaded = true;
  }

  async set<K extends keyof Prefs>(key: K, value: Prefs[K]) {
    this.values[key] = value;
    if (isTauri) await call("prefs_set", { key, value: String(value) });
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
