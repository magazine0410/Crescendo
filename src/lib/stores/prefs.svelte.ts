import { isTauri, call } from "../api";

/** Keys mirrored into the backend `prefs` table. */
export interface Prefs {
  library_path: string;
  lastfm_api_key: string;
  titlebar_format: string;
  track_detail_format: string;
  scan_threads: number;
  show_waveform: boolean;
  library_view: "album_wall" | "track_list";
  track_separators: boolean;
}

const DEFAULTS: Prefs = {
  library_path: "/mnt/Mythic_Meme/Music/",
  lastfm_api_key: "",
  titlebar_format: "%albumartist_display% - %title_display%",
  track_detail_format: "",
  scan_threads: 4,
  show_waveform: false,
  library_view: "album_wall",
  track_separators: false,
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
    if (raw.scan_threads) v.scan_threads = parseInt(raw.scan_threads, 10) || 4;
    if (raw.show_waveform) v.show_waveform = raw.show_waveform === "true";
    if (raw.library_view === "track_list" || raw.library_view === "album_wall")
      v.library_view = raw.library_view;
    if (raw.track_separators) v.track_separators = raw.track_separators === "true";
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
