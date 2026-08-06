import {
  call,
  type DropImportResult,
  type M3uImportResult,
  type PlaylistInfo,
  type Track,
} from "../api";
import { prefs, type PlaylistSort } from "./prefs.svelte";

/** Sidebar order. Names go through localeCompare — playlists carry no sort
    tags, and this library is heavily non-ASCII. Track counts go largest
    first, with the name breaking ties. */
function comparePlaylists(key: Exclude<PlaylistSort, "manual">) {
  return (a: PlaylistInfo, b: PlaylistInfo) => {
    if (key === "count" && a.track_count !== b.track_count) {
      return b.track_count - a.track_count;
    }
    return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
  };
}

class PlaylistsStore {
  list = $state<PlaylistInfo[]>([]);
  selectedId = $state<number | null>(null);
  tracks = $state<Track[]>([]);
  lastImport = $state<M3uImportResult | null>(null);
  /** Playlist the file-manager drag is currently over (highlighted). */
  dropTargetId = $state<number | null>(null);
  /** Outcome of the last drop, shown in the sidebar until dismissed. */
  dropReport = $state<string | null>(null);
  /** Sidebar filter text; matches playlist names, case-insensitive. */
  filter = $state("");

  /** What the sidebar draws: names narrowed by `filter`, in the locked sort
      order when one is active, otherwise in the stored order. */
  visible = $derived.by(() => {
    const f = this.filter.trim().toLowerCase();
    const rows = f ? this.list.filter((p) => p.name.toLowerCase().includes(f)) : [...this.list];
    const key = prefs.values.playlist_sort;
    if (prefs.values.playlist_sort_locked && key !== "manual") rows.sort(comparePlaylists(key));
    return rows;
  });

  async init() {
    await this.refresh();
    if (this.selectedId == null && this.list.length > 0) {
      await this.select(this.list[0].id);
    }
  }

  async refresh() {
    this.list = await call<PlaylistInfo[]>("playlist_list");
    if (this.selectedId != null) {
      if (this.list.some((p) => p.id === this.selectedId)) {
        this.tracks = await call<Track[]>("playlist_tracks", { id: this.selectedId });
      } else {
        this.selectedId = null;
        this.tracks = [];
      }
    }
  }

  async select(id: number) {
    this.selectedId = id;
    this.tracks = await call<Track[]>("playlist_tracks", { id });
  }

  selected(): PlaylistInfo | null {
    return this.list.find((p) => p.id === this.selectedId) ?? null;
  }

  /** Patch a track's play counter in the open playlist so its "Plays: N" line
      updates live when the player credits a play (a track can appear more than
      once, so patch every occurrence). */
  bumpPlayCount(id: number, plays: number) {
    for (const t of this.tracks) if (t.id === id) t.play_count = plays;
  }

  /** Sort the sidebar now and remember the choice. The order is written to
      the database as well, so it holds after a restart even when the sort is
      not locked. */
  async applySort(key: Exclude<PlaylistSort, "manual">) {
    await this.persistOrder([...this.list].sort(comparePlaylists(key)));
    await prefs.set("playlist_sort", key);
  }

  /** Locked, new playlists sort into place instead of landing at the end.
      Unlocking freezes the order that is on screen, so nothing jumps. */
  async setSortLocked(locked: boolean) {
    const key = prefs.values.playlist_sort;
    if (!locked && key !== "manual") {
      await this.persistOrder([...this.list].sort(comparePlaylists(key)));
    }
    await prefs.set("playlist_sort_locked", locked);
  }

  private async persistOrder(ordered: PlaylistInfo[]) {
    this.list = ordered;
    await call("playlists_set_order", { ids: ordered.map((p) => p.id) });
  }

  async create(name: string): Promise<number> {
    const id = await call<number>("playlist_create", { name });
    await this.refresh();
    await this.select(id);
    return id;
  }

  async rename(id: number, name: string) {
    await call("playlist_rename", { id, name });
    await this.refresh();
  }

  async remove(id: number) {
    await call("playlist_delete", { id });
    if (this.selectedId === id) {
      this.selectedId = null;
      this.tracks = [];
    }
    await this.refresh();
  }

  async addTracks(id: number, trackIds: number[]) {
    await call("playlist_add_tracks", { id, trackIds });
    await this.refresh();
  }

  /** Append files/folders dropped from the file manager. Unknown files are
      read into the library first; tracks the playlist already holds are
      skipped. Reports the outcome through `dropReport`. */
  async addPaths(id: number, paths: string[]) {
    this.dropReport = "Adding dropped files…";
    try {
      const r = await call<DropImportResult>("playlist_add_paths", { id, paths });
      const name = this.list.find((p) => p.id === id)?.name ?? "playlist";
      const parts: string[] = [];
      parts.push(r.added === 1 ? "Added 1 track" : `Added ${r.added} tracks`);
      if (r.imported > 0) parts.push(`${r.imported} new to the library`);
      if (r.duplicates > 0) parts.push(`${r.duplicates} already in the playlist`);
      if (r.failed.length > 0) parts.push(`${r.failed.length} unreadable`);
      this.dropReport =
        r.added === 0 && r.duplicates === 0 && r.failed.length === 0
          ? "Nothing to add — no audio files in the drop"
          : `${parts.join(", ")} → "${name}"`;
      await this.refresh();
      return r;
    } catch (e) {
      this.dropReport = `Could not add the dropped files: ${e}`;
      throw e;
    }
  }

  async removeTracks(id: number, positions: number[]) {
    await call("playlist_remove_tracks", { id, positions });
    await this.refresh();
  }

  async importM3u(path: string): Promise<M3uImportResult> {
    const result = await call<M3uImportResult>("playlist_import_m3u", { path });
    this.lastImport = result;
    await this.refresh();
    await this.select(result.playlist_id);
    return result;
  }
}

export const playlists = new PlaylistsStore();
