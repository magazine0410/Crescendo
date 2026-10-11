import {
  call,
  type CollectionInfo,
  type DropImportResult,
  type M3uImportResult,
  type PlayContext,
  type PlaylistInfo,
  type PlaylistTracks,
  type Track,
} from "../api";
import { prefs, type PlaylistSort } from "./prefs.svelte";

/** Sidebar order. Names go through localeCompare — playlists carry no sort
    tags, and this library is heavily non-ASCII. Track counts go largest
    first, with the name breaking ties. */
function comparePlaylists(key: Exclude<PlaylistSort, "manual">) {
  return (a: PlaylistInfo, b: PlaylistInfo) => compareBy(key, a.name, a.track_count, b.name, b.track_count);
}

function compareBy(
  key: Exclude<PlaylistSort, "manual">,
  aName: string,
  aCount: number,
  bName: string,
  bCount: number,
) {
  if (key === "count" && aCount !== bCount) return bCount - aCount;
  return aName.localeCompare(bName, undefined, { numeric: true, sensitivity: "base" });
}

/** One drawn sidebar row. A collection carries the playlists inside it, which
    are drawn as their own rows when it is open. */
export type SidebarRow =
  | { kind: "collection"; collection: CollectionInfo; children: PlaylistInfo[]; expanded: boolean }
  | { kind: "playlist"; playlist: PlaylistInfo }
  | { kind: "separator" };

const rowName = (r: SidebarRow) =>
  r.kind === "collection" ? r.collection.name : r.kind === "playlist" ? r.playlist.name : "";

const rowCount = (r: SidebarRow) =>
  r.kind === "collection"
    ? r.collection.track_count
    : r.kind === "playlist"
      ? r.playlist.track_count
      : 0;

/** Two groups of rows with a separator between them, unless one group is
    empty — a sidebar must not open or end with a separator. */
function withSeparator(first: SidebarRow[], second: SidebarRow[]): SidebarRow[] {
  if (first.length === 0 || second.length === 0) return [...first, ...second];
  return [...first, { kind: "separator" }, ...second];
}

/** Collections put into the playlists' own order, each one where a playlist
    of that name would be. The playlists keep their stored (manual) order:
    no stored order spans both kinds, so only the names can place them. */
function mergeByName(cols: SidebarRow[], loose: SidebarRow[]): SidebarRow[] {
  const pending = [...cols].sort((a, b) => compareBy("name", rowName(a), 0, rowName(b), 0));
  const out: SidebarRow[] = [];
  let i = 0;
  for (const row of loose) {
    while (i < pending.length && compareBy("name", rowName(pending[i]), 0, rowName(row), 0) <= 0)
      out.push(pending[i++]);
    out.push(row);
  }
  out.push(...pending.slice(i));
  return out;
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
  /** Action contexts and drop targets must describe the rows on screen while
      a different sidebar selection is loading. */
  renderedIds = $derived(this.groups.map((group) => group.playlist_id));
  get renderedId(): number | null {
    return this.renderedIds.length === 1 ? this.renderedIds[0] : null;
  }
  selectionError = $state<string | null>(null);
  lastImport = $state<M3uImportResult | null>(null);
  /** Playlist the file-manager drag is currently over (highlighted). */
  dropTargetId = $state<number | null>(null);
  /** Outcome of the last drop, shown in the sidebar until dismissed. */
  dropReport = $state<string | null>(null);
  /** Sidebar filter text; matches playlist names, case-insensitive. */
  filter = $state("");
  /** One-shot intent: scroll the open playlist to whatever is playing now.
      See LibraryStore.pendingJump — the two views hold one flag each, so a
      jump asked for from the Now playing or Visualizer tab lands on whichever
      of them is opened next. */
  pendingJump = $state(false);
  /** Viewport offset per selection, retained while an album is visited in
      another tab. A plain Map, not $state: it is written on every scroll frame
      and read only when the view remounts or the selection changes, so making
      it reactive would re-render the list while it scrolls. */
  private viewScrolls = new Map<string, number>();
  /** The selection whose rows are on screen right now. It moves with `groups`,
      not with `selectedIds`: the outgoing playlist stays rendered until the
      incoming one's tracks arrive, and a scroll during that wait belongs to
      what the user can actually see. */
  private renderedKey = $state("");
  /** Bumped with `renderedKey`, so the view re-applies the offset held for the
      newly rendered playlists instead of keeping the previous one. */
  viewScrollKey = $state(0);
  /** Named groups of playlists, in their stored order. */
  collections = $state<CollectionInfo[]>([]);
  /** Ignore list/track responses superseded by a newer event or selection. */
  private refreshReqId = 0;
  private selectionReqId = 0;

  /** The sidebar's rows: collections with their members, loose playlists, and
      the separator between the two groups when collections sit above or
      below them. Honors the filter, the locked sort, and where collections
      are shown. */
  sidebar = $derived.by((): SidebarRow[] => {
    const f = this.filter.trim().toLowerCase();
    const hit = (name: string) => !f || name.toLowerCase().includes(f);
    const key = prefs.values.playlist_sort;
    const sorted = prefs.values.playlist_sort_locked && key !== "manual" ? key : null;

    const cols: SidebarRow[] = [];
    for (const c of this.collections) {
      const members = this.list.filter((p) => p.collection_id === c.id);
      // A collection whose own name matches shows everything inside it;
      // otherwise only the members that match.
      const children = hit(c.name) ? members : members.filter((p) => hit(p.name));
      if (children.length === 0 && !hit(c.name)) continue;
      if (sorted) children.sort(comparePlaylists(sorted));
      // While filtering, open every collection so the matches are on screen.
      cols.push({ kind: "collection", collection: c, children, expanded: !c.collapsed || !!f });
    }
    const loose: SidebarRow[] = this.list
      .filter((p) => p.collection_id == null && hit(p.name))
      .map((playlist) => ({ kind: "playlist", playlist }));

    const cmp = sorted
      ? (a: SidebarRow, b: SidebarRow) =>
          compareBy(sorted, rowName(a), rowCount(a), rowName(b), rowCount(b))
      : null;
    if (cmp) {
      cols.sort(cmp);
      loose.sort(cmp);
    }

    switch (prefs.values.collections_placement) {
      case "within":
        // One list, no separator. A locked sort keys both kinds the same way;
        // without one, the names put each collection into place.
        return cmp ? [...cols, ...loose].sort(cmp) : mergeByName(cols, loose);
      case "below":
        return withSeparator(loose, cols);
      default:
        return withSeparator(cols, loose);
    }
  });

  /** Playlists the sidebar actually draws, in drawn order — what a Shift-range
      spans and what the selection is ordered by. Members of a folded
      collection are not in it, because they are not on screen. */
  visible = $derived.by(() => {
    const out: PlaylistInfo[] = [];
    for (const row of this.sidebar) {
      if (row.kind === "playlist") out.push(row.playlist);
      else if (row.kind === "collection" && row.expanded) out.push(...row.children);
    }
    return out;
  });

  /** Playlists the filter keeps, whether or not their collection is folded.
      This is what the header counts: folding is a way of looking at the list,
      not a way of leaving playlists out of it. */
  matching = $derived.by(() => {
    const out: PlaylistInfo[] = [];
    for (const row of this.sidebar) {
      if (row.kind === "playlist") out.push(row.playlist);
      else if (row.kind === "collection") out.push(...row.children);
    }
    return out;
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
    const request = ++this.refreshReqId;
    const [list, collections] = await Promise.all([
      call<PlaylistInfo[]>("playlist_list"),
      call<CollectionInfo[]>("collection_list"),
    ]);
    if (request !== this.refreshReqId) return;
    this.list = list;
    this.collections = collections;
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
    const request = ++this.selectionReqId;
    const order = this.visible.map((p) => p.id);
    const selectedIds = [...ids].sort((x, y) => order.indexOf(x) - order.indexOf(y));
    this.selectedIds = selectedIds;
    this.selectionError = null;
    if (selectedIds.length === 0) {
      this.groups = [];
      this.markRendered([]);
      return;
    }
    try {
      const groups = await call<PlaylistTracks[]>("playlist_tracks_multi", {
        ids: selectedIds,
      });
      if (request !== this.selectionReqId) return;
      this.groups = groups;
      this.markRendered(selectedIds);
    } catch (error) {
      if (request !== this.selectionReqId) return;
      this.selectedIds = [...this.renderedIds];
      this.selectionError = `Could not load playlist: ${String(error)}`;
    }
  }

  /** Hand the viewport over to a freshly rendered selection, in one step with
      the rows themselves. Re-applying an offset any earlier would clamp it to
      the outgoing list's height, and crediting a scroll any earlier would file
      the outgoing list's offset under the incoming one. */
  private markRendered(ids: number[]) {
    this.renderedKey = ids.join(",");
    this.viewScrollKey++;
  }

  /** Stored offset of the rendered selection; 0 for one not scrolled yet. */
  get viewScroll(): number {
    return this.viewScrolls.get(this.renderedKey) ?? 0;
  }

  setViewScroll(y: number) {
    this.viewScrolls.set(this.renderedKey, y);
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
    await this.placeSorted(id);
    await this.select(id);
    return id;
  }

  async rename(id: number, name: string) {
    await call("playlist_rename", { id, name });
    await this.refresh();
    await this.placeSorted(id);
  }

  /** Move one playlist to where the chosen sort puts it — before the first
      playlist that sorts after it — leaving every other playlist in place.
      The backend appends new playlists to the stored order, so without this
      a playlist made (or renamed) under a sort would sit at the bottom until
      the sort was applied again. Under a manual order nothing moves. */
  private async placeSorted(id: number) {
    const key = prefs.values.playlist_sort;
    if (key === "manual") return;
    const playlist = this.list.find((p) => p.id === id);
    if (!playlist) return;
    const cmp = comparePlaylists(key);
    const rest = this.list.filter((p) => p.id !== id);
    let at = rest.findIndex((p) => cmp(playlist, p) < 0);
    if (at < 0) at = rest.length;
    rest.splice(at, 0, playlist);
    if (rest.some((p, i) => p !== this.list[i])) await this.persistOrder(rest);
  }

  async remove(id: number) {
    await call("playlist_delete", { id });
    // refresh() drops it from the selection; the rest of a multi-selection
    // stays on screen.
    await this.refresh();
  }

  /** Put playlists into the collection called `name`, creating it when the
      name is new. A blank name takes them out of their collection. */
  async setCollection(playlistIds: number[], name: string) {
    await call("collection_set", { playlistIds, name });
    await this.refresh();
  }

  async renameCollection(id: number, name: string) {
    await call("collection_rename", { id, name });
    await this.refresh();
  }

  /** Drop the group. Its playlists stay, loose again. */
  async removeCollection(id: number) {
    await call("collection_delete", { id });
    await this.refresh();
  }

  /** Fold or unfold a collection. Patched locally as well as stored, so the
      row responds to the click without waiting for a round trip. */
  async toggleCollection(id: number) {
    const c = this.collections.find((c) => c.id === id);
    if (!c) return;
    c.collapsed = !c.collapsed;
    await call("collection_set_collapsed", { id, collapsed: c.collapsed });
  }

  async addTracks(id: number, trackIds: number[]) {
    await call("playlist_add_tracks", { id, trackIds });
    await this.refresh();
  }

  /** Append every track a context names (an album, or a filter-panel value).
      The backend resolves it, so a stale projection is rejected the same way
      it is for playback and the queue. */
  async addContext(id: number, ctx: PlayContext) {
    await call<number>("playlist_add_context", { id, ctx });
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
