import {
  call,
  type DropImportResult,
  type M3uImportResult,
  type PlaylistInfo,
  type PlaylistTracks,
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
  /** Selected playlists, in sidebar order. Usually one; Ctrl/Shift-click in
      the sidebar selects several and the view shows them as one list. */
  selectedIds = $state<number[]>([]);
  /** Base for Shift-range selects: the last plainly clicked or Ctrl-toggled
      playlist. Null until something is clicked. */
  anchorId = $state<number | null>(null);
  /** Tracks of each selected playlist, one group per playlist, in the same
      order as `selectedIds`. */
  groups = $state<PlaylistTracks[]>([]);
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

  /** The tracks of every selected playlist, back to back. This flat order is
      what a row's play index counts along, and what `PlayContext` resolves to
      on the Rust side — the two must not drift apart. */
  tracks = $derived(this.groups.flatMap((g) => g.tracks));

  /** The single selected playlist, or null when several (or none) are. Used
      where an action only makes sense for one playlist, such as being a
      drop target for dragged files. */
  get selectedId(): number | null {
    return this.selectedIds.length === 1 ? this.selectedIds[0] : null;
  }

  isSelected(id: number): boolean {
    return this.selectedIds.includes(id);
  }

  async init() {
    await this.refresh();
    if (this.selectedIds.length === 0 && this.list.length > 0) {
      await this.select(this.list[0].id);
    }
  }

  async refresh() {
    this.list = await call<PlaylistInfo[]>("playlist_list");
    // Drop any selection that no longer exists, then reload what is left.
    const alive = this.selectedIds.filter((id) => this.list.some((p) => p.id === id));
    await this.applySelection(alive);
  }

  async select(id: number) {
    this.anchorId = id;
    await this.applySelection([id]);
  }

  /** Ctrl+click: add or remove one playlist from the selection. */
  async toggle(id: number) {
    this.anchorId = id;
    await this.applySelection(
      this.isSelected(id)
        ? this.selectedIds.filter((x) => x !== id)
        : [...this.selectedIds, id],
    );
  }

  /** Shift+click: select the visible range between the anchor and this one,
      replacing the selection. Without a usable anchor — none yet, or it is
      filtered out of view — acts like a plain click. The anchor stays, so a
      second Shift+click re-ranges from the same base. */
  async selectRange(id: number) {
    const visible = this.visible.map((p) => p.id);
    const a = this.anchorId === null ? -1 : visible.indexOf(this.anchorId);
    const b = visible.indexOf(id);
    if (a < 0 || b < 0) return this.select(id);
    const [lo, hi] = a <= b ? [a, b] : [b, a];
    await this.applySelection(visible.slice(lo, hi + 1));
  }

  /** Store the selection in sidebar order and load its tracks in one call. */
  private async applySelection(ids: number[]) {
    const order = this.visible.map((p) => p.id);
    this.selectedIds = [...ids].sort((x, y) => order.indexOf(x) - order.indexOf(y));
    if (this.selectedIds.length === 0) {
      this.groups = [];
      return;
    }
    this.groups = await call<PlaylistTracks[]>("playlist_tracks_multi", {
      ids: this.selectedIds,
    });
  }

  selected(): PlaylistInfo | null {
    const id = this.selectedId;
    return id == null ? null : (this.list.find((p) => p.id === id) ?? null);
  }

  /** The selected playlists themselves, in view order. */
  selectedPlaylists(): PlaylistInfo[] {
    return this.selectedIds
      .map((id) => this.list.find((p) => p.id === id))
      .filter((p): p is PlaylistInfo => p !== undefined);
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
    // refresh() drops it from the selection; the rest of a multi-selection
    // stays on screen.
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
