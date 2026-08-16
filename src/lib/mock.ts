/**
 * Mock backend used only in plain-browser dev (`npm run dev` without Tauri).
 * Generates a deterministic fake library big enough to exercise the
 * virtualized lists, and implements the same command surface as src-tauri.
 */
import type { Album, CollectionInfo, PlaylistInfo, SortKey, Track, TrackSort } from "./api";
import packageInfo from "../../package.json";

const GENRES = ["Jazz", "Soundtrack", "Pop Rock", "Doujin", "Electronic", "Metal", null];
// [artist, artistsort] — sort tag only where the name is Japanese
const ARTISTS: [string, string | null][] = [
  ["Charlie Parker", null],
  ["Joe Hisaishi", null],
  ["Maroon 5", null],
  ["ZUN", null],
  ["Aphex Twin", null],
  ["Boards of Canada", null],
  ["Iron Maiden", null],
  ["上海アリス幻樂団", "Shanghai Alice Gengakudan"],
  ["幽閉サテライト", "Yuuhei Satellite"],
  ["Moby", null],
];

const ALBUM_COUNT = 400;
const TRACKS_PER_ALBUM = 12;

// Grouping tags with semicolon-joined multi-theme variants, so the GROUPING
// "collapse multi-tagged" toggle has something to fold: the three `"Old Adam"
// Bar…` strings collapse to one umbrella, the two `Native Faith…` to another.
const GROUPINGS: (string | null)[] = [
  null,
  '"Old Adam" Bar',
  '"Old Adam" Bar; A Drunkard\'s Lemuria',
  '"Old Adam" Bar; Hangover of Bedfellows Dreaming Differently',
  "Native Faith",
  "Native Faith; Faith is for the Transient People",
  "U.N. Owen Was Her?",
];

let built = false;
const tracks: Track[] = [];
const albums: Album[] = [];
const playlists: PlaylistInfo[] = [
  { id: 1, name: "New Playlist", track_count: 27, total_duration_ms: 3_714_000, collection_id: null },
  { id: 2, name: "Chill", track_count: 14, total_duration_ms: 3_100_000, collection_id: null },
];
const collections: Pick<CollectionInfo, "id" | "name" | "collapsed">[] = [];
const playlistTracks = new Map<number, number[]>();
const queue: number[] = [];

function build() {
  if (built) return;
  built = true;
  let id = 1;
  for (let a = 0; a < ALBUM_COUNT; a++) {
    const [artist, artistsort] = ARTISTS[a % ARTISTS.length];
    const jp = artistsort != null;
    const genre = GENRES[a % GENRES.length];
    const year = 1960 + (a % 60);
    const albumName = `Album ${a + 1} of ${artist}`;
    let total = 0;
    for (let t = 0; t < TRACKS_PER_ALBUM; t++) {
      const dur = 90_000 + ((a * 31 + t * 47) % 240) * 1000;
      total += dur;
      tracks.push({
        id: id++,
        path: `/mock/${artist}/${albumName}/${t + 1}.flac`,
        title: jp ? `曲その${t + 1}（${albumName}）` : `Track ${t + 1} (${albumName})`,
        artist,
        album: albumName,
        albumartist: artist,
        titlesort: jp ? `Kyoku Sono ${t + 1} (${albumName})` : null,
        artistsort,
        albumartistsort: artistsort,
        genre,
        grouping: GROUPINGS[(a * 3 + t) % GROUPINGS.length],
        track_no: t + 1,
        disc_no: 1,
        year,
        duration_ms: dur,
        format: "FLAC",
        album_id: a + 1,
        rating: null,
        // Sprinkle some non-zero play counts so the "Plays: N" line is visible
        // in `npm run dev`.
        play_count: (a + t) % 3 === 0 ? ((a * 5 + t) % 4) + 1 : 0,
        // One gone-from-disk track inside each mock playlist, so the "File
        // missing" marking is visible. Kept to a couple of tracks because
        // these mock queries, unlike the backend's, do not filter missing
        // tracks out of the library views.
        missing: (a === 0 && t === 3) || (a === 3 && t === 5),
      });
    }
    albums.push({
      id: a + 1,
      album: albumName,
      albumartist: artist,
      albumartist_sort: artistsort,
      year,
      genre,
      track_count: TRACKS_PER_ALBUM,
      total_duration_ms: total,
    });
  }
  playlistTracks.set(1, tracks.slice(0, 27).map((t) => t.id));
  playlistTracks.set(2, tracks.slice(40, 54).map((t) => t.id));
}

function matches(t: Track, query: string | null | undefined): boolean {
  if (!query || !query.trim()) return true;
  const q = query.toLowerCase();
  return [t.title, t.artist, t.album, t.albumartist, t.genre].some((f) =>
    f?.toLowerCase().includes(q),
  );
}

/** Wire form of BrowseMode (see api.ts): a fixed mode name, or the custom tag. */
type WireMode = string | { custom: string };

function browseCol(t: Track, mode: string): string {
  const v =
    mode === "genre" ? t.genre
    : mode === "artist" ? t.artist
    : mode === "album" ? t.album
    : mode === "albumartist" ? t.albumartist
    : t.grouping;
  return v ?? "";
}

/** Map a resolved custom tag to a Track field getter, mirroring the backend's
    browse_column whitelist over the mock's available columns. null = unbrowsable
    (→ empty value list). Integer columns stringify like the backend's TEXT cast. */
function customField(tag: string): ((t: Track) => string) | null {
  switch (tag.trim().toLowerCase()) {
    case "year":
    case "date": return (t) => (t.year != null ? String(t.year) : "");
    case "genre": return (t) => t.genre ?? "";
    case "artist": return (t) => t.artist ?? "";
    case "album": return (t) => t.album ?? "";
    case "albumartist": return (t) => t.albumartist ?? "";
    case "grouping": return (t) => t.grouping ?? "";
    case "title": return (t) => t.title ?? "";
    case "format":
    case "codec": return (t) => t.format ?? "";
    // Whole-minute duration bucket (mirrors the backend DURATION_BUCKET).
    case "duration":
    case "length": return (t) => String(Math.floor(t.duration_ms / 60000));
    case "track":
    case "tracknumber": return (t) => (t.track_no != null ? String(t.track_no) : "");
    case "disc":
    case "discnumber": return (t) => (t.disc_no != null ? String(t.disc_no) : "");
    case "rating": return (t) => (t.rating != null ? String(t.rating) : "");
    case "lastplayed":
    case "last_played": return lastPlayedBucket;
    case "playcount":
    case "play_count":
    case "plays": return (t) => String(t.play_count);
    default: return null;
  }
}

/** Stand-in play history for the dev backend, which records none: a track's id
    decides how long ago it was "last played", so the %lastplayed% buckets and
    the Last-played sort both have something to show. One track in six was never
    played (null). */
function mockDaysAgo(t: Track): number | null {
  const DAYS = [0, 3, 14, 100, 800, null] as const;
  return DAYS[t.id % 6];
}

/** Bucket number of a track's last play, mirroring last_played_bucket() in
    queries.rs: rolling day counts, narrowest span wins, "5" = never played. */
function lastPlayedBucket(t: Track): string {
  const d = mockDaysAgo(t);
  if (d == null) return "5";
  return String(d <= 0 ? 0 : d < 7 ? 1 : d < 30 ? 2 : d < 365 ? 3 : 4);
}

/** Primary theme of a grouping tag: text before the first ';' (trimmed). */
function firstSeg(v: string): string {
  const i = v.indexOf(";");
  return (i >= 0 ? v.slice(0, i) : v).trim();
}

/** The value a track buckets under. Collapses grouping to its primary theme when
    `collapse` is set (mirrors GROUPING_FIRST_SEG); routes custom modes through
    the resolved tag (unbrowsable tag → "", never selected in practice). */
function browseKey(t: Track, mode: WireMode, collapse: boolean): string {
  if (typeof mode === "object") {
    const f = customField(mode.custom);
    const v = f ? f(t) : "";
    return mode.custom.trim().toLowerCase() === "grouping" && collapse ? firstSeg(v) : v;
  }
  const v = browseCol(t, mode);
  return mode === "grouping" && collapse ? firstSeg(v) : v;
}

/** Track-list filter: search query AND optional left-panel mode values (a
    track matches when its value is any of the multi-selected ones). */
function trackListMatch(t: Track, args: Record<string, unknown>): boolean {
  if (!matches(t, args.query as string | null | undefined)) return false;
  const values = args.values as string[] | null | undefined;
  return (
    values == null ||
    values.length === 0 ||
    values.includes(browseKey(t, args.mode as WireMode, args.collapse as boolean))
  );
}

/** Mirror the backend LIBRARY_SORT so the dev track list groups by album. */
function libSort(a: Track, b: Track): number {
  return (
    (a.albumartist ?? "").localeCompare(b.albumartist ?? "") ||
    (a.year ?? 0) - (b.year ?? 0) ||
    (a.album ?? "").localeCompare(b.album ?? "") ||
    (a.disc_no ?? 0) - (b.disc_no ?? 0) ||
    (a.track_no ?? 0) - (b.track_no ?? 0) ||
    a.path.localeCompare(b.path)
  );
}

/** Mirrors the backend `prefer_romanized` pref (default on), which decides
    whether ordering follows the romanized *sort tags. `prefs_set` keeps it
    current; the dev backend stores nothing else. */
let preferRomanized = true;

/** Mirrors the backend `lyrics_online` pref (default on). The mock track has
    no sidecar file or embedded tag, so switching it off leaves it with no
    lyrics at all — same as the real backend would. */
let lyricsOnline = true;

/** Sort key of a title: the romanized tag only while it is what the row shows. */
function titleKey(t: Track): string {
  return (preferRomanized ? t.titlesort || t.title : t.title) || "";
}

/** One explicit sort key's comparison (mirrors queries.rs sort_key_sql). */
function keyCmp(a: Track, b: Track, key: SortKey): number {
  switch (key) {
    case "year": return (a.year ?? 0) - (b.year ?? 0);
    case "duration": return a.duration_ms - b.duration_ms;
    case "title":
      return titleKey(a).localeCompare(titleKey(b));
    case "albumartist": return (a.albumartist ?? "").localeCompare(b.albumartist ?? "");
    case "artist": return (a.artist ?? "").localeCompare(b.artist ?? "");
    case "album": return (a.album ?? "").localeCompare(b.album ?? "");
    case "genre": return (a.genre ?? "").localeCompare(b.genre ?? "");
    case "grouping": return (a.grouping ?? "").localeCompare(b.grouping ?? "");
    // Ascending = oldest first; a track that was never played has no time at
    // all and sorts below every real one (SQLite puts NULLs first ascending).
    case "lastplayed": {
      const at = (t: Track) => {
        const d = mockDaysAgo(t);
        return d == null ? -Infinity : -d;
      };
      return at(a) === at(b) ? 0 : at(a) < at(b) ? -1 : 1;
    }
  }
}

/** Track-list comparator honoring the explicit sort (mirrors queries.rs
    order_sql): chosen key(s) with their directions (secondary follows the
    primary unless set on its own), then libSort for remaining ties (keys
    compared equal already, so no filtering needed). */
function viewSort(args: Record<string, unknown>): (a: Track, b: Track) => number {
  const s = args.sort as TrackSort | null | undefined;
  if (!s) return libSort;
  const dir = s.descending ? -1 : 1;
  const secDir = (s.secondary_descending ?? s.descending) ? -1 : 1;
  return (a, b) => {
    let c = dir * keyCmp(a, b, s.key);
    if (c === 0 && s.secondary && s.secondary !== s.key)
      c = secDir * keyCmp(a, b, s.secondary);
    return c || libSort(a, b);
  };
}

export async function mockCall<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  build();
  const query = args.query as string | null | undefined;
  switch (cmd) {
    case "library_stats":
      return {
        tracks: tracks.length,
        albums: albums.length,
        total_duration_ms: tracks.reduce((s, t) => s + t.duration_ms, 0),
        formats: { FLAC: tracks.length },
      } as T;
    case "browse_values": {
      const mode = args.mode as WireMode;
      const collapse = args.collapse as boolean;
      // Unresolvable custom tag → empty list (mirrors the backend).
      if (typeof mode === "object" && customField(mode.custom) === null) return [] as T;
      const counts = new Map<string, number>();
      const sorts = new Map<string, string>();
      for (const t of tracks) {
        if (!matches(t, query)) continue;
        const v = browseKey(t, mode, collapse);
        counts.set(v, (counts.get(v) ?? 0) + 1);
        if (mode === "artist" && t.artistsort) sorts.set(v, t.artistsort);
        if (mode === "albumartist" && t.albumartistsort) sorts.set(v, t.albumartistsort);
      }
      // Duration buckets order numerically ("10" after "3") and play counts do
      // the same in reverse (most played first), like the backend.
      const tag =
        typeof mode === "object" ? mode.custom.trim().toLowerCase() : "";
      const numeric = ["duration", "length"].includes(tag);
      const descending = ["playcount", "play_count", "plays"].includes(tag);
      return [...counts.entries()]
        .map(([v, n]): [string, number, string] => [v, n, sorts.get(v) ?? ""])
        .sort((a, b) =>
          numeric
            ? Number(a[0]) - Number(b[0])
            : descending
              ? Number(b[0]) - Number(a[0])
              : preferRomanized
                ? (a[2] || a[0]).localeCompare(b[2] || b[0])
                : a[0].localeCompare(b[0]),
        ) as T;
    }
    case "browse_albums": {
      const mode = args.mode as WireMode;
      const values = (args.values as string[] | null) ?? [];
      const collapse = args.collapse as boolean;
      const ids = new Set<number>();
      for (const t of tracks) {
        if (!matches(t, query)) continue;
        if (values.length > 0 && !values.includes(browseKey(t, mode, collapse))) continue;
        if (t.album_id != null) ids.add(t.album_id);
      }
      return albums.filter((a) => ids.has(a.id)) as T;
    }
    case "album_tracks":
      return tracks.filter((t) => t.album_id === (args.albumId as number)) as T;
    case "track_count":
      return tracks.filter((t) => trackListMatch(t, args)).length as T;
    case "tracks_range": {
      const filtered = tracks.filter((t) => trackListMatch(t, args)).sort(viewSort(args));
      const offset = args.offset as number;
      const limit = args.limit as number;
      return filtered.slice(offset, offset + limit) as T;
    }
    case "album_by_id":
      return (albums.find((a) => a.id === (args.albumId as number)) ?? null) as T;
    case "track_groups": {
      const filtered = tracks.filter((t) => trackListMatch(t, args)).sort(viewSort(args));
      const groups: {
        album_id: number | null;
        album: string | null;
        artist: string | null;
        artist_sort: string | null;
        year: number | null;
        count: number;
      }[] = [];
      let cur: (typeof groups)[number] | null = null;
      for (const t of filtered) {
        if (!cur || cur.album_id !== t.album_id) {
          cur = {
            album_id: t.album_id,
            album: t.album,
            artist: t.albumartist ?? t.artist,
            artist_sort: t.albumartistsort ?? t.artistsort,
            year: t.year,
            count: 0,
          };
          groups.push(cur);
        }
        cur.count++;
      }
      return groups as T;
    }
    case "track_index": {
      const filtered = tracks.filter((t) => trackListMatch(t, args)).sort(viewSort(args));
      const idx = filtered.findIndex((t) => t.id === (args.trackId as number));
      return (idx < 0 ? null : idx) as T;
    }
    case "search_classify": {
      const q = (args.query as string).trim();
      if (!q) return "plain" as T;
      // crude stand-in: unbalanced quote = invalid, any operator = syntax.
      const quotes = (q.match(/"/g) ?? []).length;
      if (quotes % 2 !== 0) return "invalid" as T;
      return (/[=#&|!/<>]|"/.test(q) ? "syntax" : "plain") as T;
    }
    case "playlist_list":
      return playlists as T;
    case "playlist_tracks": {
      const ids = playlistTracks.get(args.id as number) ?? [];
      return ids.map((id) => tracks[id - 1]).filter(Boolean) as T;
    }
    case "collection_list":
      return collections.map((c) => {
        const members = playlists.filter((p) => p.collection_id === c.id);
        return {
          ...c,
          playlist_count: members.length,
          // Plain sum: a track in two member playlists counts twice.
          track_count: members.reduce((s, p) => s + p.track_count, 0),
          total_duration_ms: members.reduce((s, p) => s + p.total_duration_ms, 0),
        };
      }) as T;
    case "collection_set": {
      const ids = args.playlistIds as number[];
      const name = (args.name as string).trim();
      let target: number | null = null;
      if (name) {
        const found = collections.find((c) => c.name.toLowerCase() === name.toLowerCase());
        target = found ? found.id : Math.max(0, ...collections.map((c) => c.id)) + 1;
        if (!found) collections.push({ id: target, name, collapsed: false });
      }
      for (const p of playlists) if (ids.includes(p.id)) p.collection_id = target;
      // Drop collections that have no members left, as the backend does.
      for (let i = collections.length - 1; i >= 0; i--) {
        if (!playlists.some((p) => p.collection_id === collections[i].id)) collections.splice(i, 1);
      }
      return target as T;
    }
    case "collection_rename": {
      const c = collections.find((c) => c.id === args.id);
      if (c) c.name = (args.name as string).trim();
      return undefined as T;
    }
    case "collection_delete": {
      const i = collections.findIndex((c) => c.id === args.id);
      if (i >= 0) collections.splice(i, 1);
      for (const p of playlists) if (p.collection_id === args.id) p.collection_id = null;
      return undefined as T;
    }
    case "collection_set_collapsed": {
      const c = collections.find((c) => c.id === args.id);
      if (c) c.collapsed = args.collapsed as boolean;
      return undefined as T;
    }
    case "playlist_create": {
      const id = Math.max(0, ...playlists.map((p) => p.id)) + 1;
      playlists.push({ id, name: args.name as string, track_count: 0, total_duration_ms: 0, collection_id: null });
      playlistTracks.set(id, []);
      return id as T;
    }
    case "playlist_tracks_multi": {
      const ids = args.ids as number[];
      return ids.map((id) => ({
        playlist_id: id,
        tracks: (playlistTracks.get(id) ?? []).map((t) => tracks[t - 1]).filter(Boolean),
      })) as T;
    }
    case "playlists_set_order": {
      const ids = args.ids as number[];
      const rank = new Map(ids.map((id, i) => [id, i]));
      // Unlisted playlists keep their place at the end, as in the backend.
      playlists.sort((a, b) => (rank.get(a.id) ?? Infinity) - (rank.get(b.id) ?? Infinity));
      return undefined as T;
    }
    case "playlist_rename": {
      const p = playlists.find((p) => p.id === args.id);
      if (p) p.name = args.name as string;
      return undefined as T;
    }
    case "playlist_delete": {
      const i = playlists.findIndex((p) => p.id === args.id);
      if (i >= 0) playlists.splice(i, 1);
      return undefined as T;
    }
    case "playlist_add_tracks": {
      const id = args.id as number;
      const ids = playlistTracks.get(id) ?? [];
      const present = new Set(ids);
      for (const tid of args.trackIds as number[]) {
        if (!present.has(tid)) {
          ids.push(tid);
          present.add(tid);
        }
      }
      playlistTracks.set(id, ids);
      const p = playlists.find((p) => p.id === id);
      if (p) {
        p.track_count = ids.length;
        p.total_duration_ms = ids.reduce((s, t) => s + (tracks[t - 1]?.duration_ms ?? 0), 0);
      }
      return undefined as T;
    }
    // The browser has no filesystem, so a dropped path can only be reported
    // back as "not audio" — enough to keep the UI path exercised.
    case "playlist_add_paths":
      return { added: 0, duplicates: 0, imported: 0, failed: [] } as T;
    case "playlist_remove_tracks": {
      const id = args.id as number;
      const gone = new Set(args.positions as number[]);
      const ids = (playlistTracks.get(id) ?? []).filter((_, i) => !gone.has(i));
      playlistTracks.set(id, ids);
      const p = playlists.find((p) => p.id === id);
      if (p) {
        p.track_count = ids.length;
        p.total_duration_ms = ids.reduce((s, t) => s + (tracks[t - 1]?.duration_ms ?? 0), 0);
      }
      return undefined as T;
    }
    case "track_playlist_ids": {
      const tid = args.trackId as number;
      const out: number[] = [];
      for (const [pid, ids] of playlistTracks) if (ids.includes(tid)) out.push(pid);
      return out as T;
    }
    case "queue_add":
      queue.push(...(args.trackIds as number[]));
      return undefined as T;
    case "queue_remove": {
      const positions = new Set(args.positions as number[]);
      for (let i = queue.length - 1; i >= 0; i--) if (positions.has(i)) queue.splice(i, 1);
      return undefined as T;
    }
    case "queue_clear":
      queue.length = 0;
      return undefined as T;
    case "queue_list":
      return queue.map((id) => tracks[id - 1]).filter(Boolean) as T;
    case "prefs_get_all":
      return {} as T;
    case "prefs_set":
      if (args.key === "prefer_romanized") preferRomanized = args.value !== "false";
      if (args.key === "lyrics_online") lyricsOnline = args.value !== "false";
      return undefined as T;
    case "about_info":
      return {
        version: packageInfo.version,
        repository_url: "https://github.com/magazine0410/Crescendo",
        dependencies: [
          { name: "Tauri", version: "2.11.5" },
          { name: "Tauri JS API", version: "2.11.1" },
          { name: "Tauri Dialog", version: "2.7.2" },
          { name: "Tauri Opener", version: "2.5.4" },
          { name: "Svelte", version: "5.56.6" },
          { name: "SvelteKit", version: "2.70.0" },
          { name: "Vite", version: "6.4.3" },
          { name: "GStreamer bindings", version: "0.24.5" },
          { name: "projectM WebAssembly", version: "4.1.5" },
          { name: "Tokio", version: "1.53.0" },
          { name: "rusqlite", version: "0.32.1" },
          { name: "Lofty", version: "0.22.4" },
          { name: "image", version: "0.25.10" },
          { name: "ureq", version: "2.12.1" },
          { name: "walkdir", version: "2.5.0" },
          { name: "regex", version: "1.13.1" },
          { name: "Serde", version: "1.0.229" },
        ],
      } as T;
    case "player_set_spectrum_enabled":
      return undefined as T;
    // The projectM commands (player_set_projectm_enabled,
    // player_take_projectm_pcm, projectm_choose_folder, projectm_restore_folder, projectm_load_preset,
    // projectm_load_asset) have no mock on purpose: every call site is behind
    // an `isTauri` guard, and the Visualizer tab drives its own preview audio
    // in the browser. Add mocks here if any of those guards ever go away.
    case "waveform_get": {
      // Synthetic loudness envelope so the waveform is visible in `npm run dev`.
      const n = 500;
      const out: number[] = [];
      for (let i = 0; i < n; i++) {
        const t = i / n;
        const env = Math.sin(t * Math.PI); // fade in/out over the track
        const detail = 0.45 + 0.55 * Math.abs(Math.sin(i * 0.7) * Math.cos(i * 0.11));
        out.push(Math.round(env * detail * 255));
      }
      return out as T;
    }
    case "lastfm_artist_info":
    case "lastfm_album_info":
      return null as T;
    case "lastfm_test_key":
      return Boolean((args as { apiKey?: string } | undefined)?.apiKey?.trim()) as T;
    case "lyrics_for_track": {
      if (!lyricsOnline) return null as T;
      // Long enough to scroll, and timed across the mock track's 3:04 so the
      // synced view actually follows along in the browser preview.
      const verse = [
        "Sukoshi no kaze ni fukare",
        "Sukoshi no ame ni utare",
        "Toki no sugiyuku mama ni",
        "Ikiru koto no imi o shiru",
        "",
        "Yagate yoru ga akete",
        "Asahi ga mado o terasu",
      ];
      const lines = Array.from({ length: 4 }, (_, verseNo) =>
        verse.map((text, i) => ({ ms: (verseNo * verse.length + i) * 6000, text })),
      ).flat();
      return { text: lines.map((l) => l.text).join("\n").trim(), lines, source: "Mock" } as T;
    }
    case "autostart_get":
      return false as T;
    case "autostart_set":
      return undefined as T;
    case "scan_start":
    case "scan_cancel":
    case "player_play_context":
    case "player_jump_to":
      return undefined as T;
    default:
      console.warn(`mockCall: unhandled command ${cmd}`, args);
      return undefined as T;
  }
}

export const mockNowPlaying: Track = {
  id: 1,
  path: "/mock/Charlie Parker/The Essential/01.flac",
  title: "時の過ぎゆくままに",
  artist: "Charlie Parker",
  album: "The Essential Charlie Parker BestOF (1940-50)",
  albumartist: "Charlie Parker",
  titlesort: "Toki no Sugiyuku Mama ni",
  artistsort: null,
  albumartistsort: null,
  genre: "Jazz",
  grouping: '"Old Adam" Bar; Hangover of Bedfellows Dreaming Differently; A Particularly Long Grouping Tag for Previewing the Two-Line Ellipsis',
  track_no: 1,
  disc_no: 1,
  year: 1950,
  duration_ms: 184_000,
  format: "FLAC",
  album_id: 1,
  rating: null,
  play_count: 3,
  missing: false,
};
