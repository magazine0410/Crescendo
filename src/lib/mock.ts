/**
 * Mock backend used only in plain-browser dev (`npm run dev` without Tauri).
 * Generates a deterministic fake library big enough to exercise the
 * virtualized lists, and implements the same command surface as src-tauri.
 */
import type { Album, PlaylistInfo, Track } from "./api";

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

let built = false;
const tracks: Track[] = [];
const albums: Album[] = [];
const playlists: PlaylistInfo[] = [
  { id: 1, name: "New Playlist", track_count: 27, total_duration_ms: 3_714_000 },
  { id: 2, name: "Chill", track_count: 14, total_duration_ms: 3_100_000 },
];
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
        grouping: a % 7 === 0 ? "Favorites" : null,
        track_no: t + 1,
        disc_no: 1,
        year,
        duration_ms: dur,
        format: "FLAC",
        album_id: a + 1,
        rating: null,
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

function browseCol(t: Track, mode: string): string {
  const v =
    mode === "genre" ? t.genre
    : mode === "artist" ? t.artist
    : mode === "album" ? t.album
    : t.grouping;
  return v ?? "";
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
      const mode = args.mode as string;
      const counts = new Map<string, number>();
      const sorts = new Map<string, string>();
      for (const t of tracks) {
        if (!matches(t, query)) continue;
        const v = browseCol(t, mode);
        counts.set(v, (counts.get(v) ?? 0) + 1);
        if (mode === "artist" && t.artistsort) sorts.set(v, t.artistsort);
      }
      return [...counts.entries()]
        .map(([v, n]): [string, number, string] => [v, n, sorts.get(v) ?? ""])
        .sort((a, b) => (a[2] || a[0]).localeCompare(b[2] || b[0])) as T;
    }
    case "browse_albums": {
      const mode = args.mode as string;
      const value = args.value as string | null;
      const ids = new Set<number>();
      for (const t of tracks) {
        if (!matches(t, query)) continue;
        if (value != null && browseCol(t, mode) !== value) continue;
        if (t.album_id != null) ids.add(t.album_id);
      }
      return albums.filter((a) => ids.has(a.id)) as T;
    }
    case "album_tracks":
      return tracks.filter((t) => t.album_id === (args.albumId as number)) as T;
    case "track_count":
      return tracks.filter((t) => matches(t, query)).length as T;
    case "tracks_range": {
      const filtered = tracks.filter((t) => matches(t, query));
      const offset = args.offset as number;
      const limit = args.limit as number;
      return filtered.slice(offset, offset + limit) as T;
    }
    case "search_validate":
      return ((args.query as string).includes('"') ? "mock: unbalanced quote?" : null) as T;
    case "playlist_list":
      return playlists as T;
    case "playlist_tracks": {
      const ids = playlistTracks.get(args.id as number) ?? [];
      return ids.map((id) => tracks[id - 1]).filter(Boolean) as T;
    }
    case "playlist_create": {
      const id = Math.max(0, ...playlists.map((p) => p.id)) + 1;
      playlists.push({ id, name: args.name as string, track_count: 0, total_duration_ms: 0 });
      playlistTracks.set(id, []);
      return id as T;
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
      return undefined as T;
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
  title: "Now's The Time",
  artist: "Charlie Parker",
  album: "The Essential Charlie Parker BestOF (1940-50)",
  albumartist: "Charlie Parker",
  titlesort: null,
  artistsort: null,
  albumartistsort: null,
  genre: "Jazz",
  grouping: null,
  track_no: 1,
  disc_no: 1,
  year: 1950,
  duration_ms: 184_000,
  format: "FLAC",
  album_id: 1,
  rating: null,
};
