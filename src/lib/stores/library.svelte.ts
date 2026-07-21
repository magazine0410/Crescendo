import {
  call,
  type Album,
  type BrowseMode,
  type LibraryStats,
  type ScanProgress,
  type Track,
  type TrackGroup,
} from "../api";
import { prefs } from "./prefs.svelte";

/** Rows fetched per windowed page in the Track-list view. */
const TRACK_PAGE = 200;

/** A visible row in the Track-list view: an album separator or a track. */
export type TrackRow = { sep: TrackGroup; albumId: number | null } | { trackIndex: number };

/** Flat row layout for album separators: maps flat row index <-> track index via
    per-group prefix sums, so the virtualized list needs no full row array. */
interface RowModel {
  groups: TrackGroup[];
  hasSep: boolean[];
  rowStart: Float64Array; // rowStart[g] = flat row where group g begins
  trackStart: Float64Array; // trackStart[g] = tracks before group g
  totalRows: number;
}

function buildRowModel(groups: TrackGroup[]): RowModel {
  const n = groups.length;
  const hasSep = new Array<boolean>(n);
  const rowStart = new Float64Array(n + 1);
  const trackStart = new Float64Array(n + 1);
  let row = 0;
  let trk = 0;
  for (let g = 0; g < n; g++) {
    const sep = groups[g].album_id != null; // NULL-album runs get no clickable header
    hasSep[g] = sep;
    rowStart[g] = row;
    trackStart[g] = trk;
    row += (sep ? 1 : 0) + groups[g].count;
    trk += groups[g].count;
  }
  rowStart[n] = row;
  trackStart[n] = trk;
  return { groups, hasSep, rowStart, trackStart, totalRows: row };
}

class LibraryStore {
  mode = $state<BrowseMode>("artist");
  /** [value, track count, sort/display value ("" when none)] */
  values = $state<[string, number, string][]>([]);
  selectedValue = $state<string | null>(null);
  albums = $state<Album[]>([]);
  selectedAlbum = $state<Album | null>(null);
  albumTracks = $state<Track[]>([]);
  stats = $state<LibraryStats | null>(null);
  query = $state<string | null>(null);

  /** Windowed full-library track list (Track-list view). `tracks[i]` is
      undefined until its page loads. `trackEpoch` bumps on every reset so the
      view can re-fetch its visible range; `loadedPages` dedupes page fetches
      and `trackReqId` discards responses that a filter change has superseded. */
  trackCount = $state(0);
  tracks = $state<(Track | undefined)[]>([]);
  trackEpoch = $state(0);
  /** Non-null when album separators are on: the flat row layout over `tracks`. */
  trackModel = $state<RowModel | null>(null);
  private pagePromises = new Map<number, Promise<void>>();
  private trackReqId = 0;

  /** Track-list selection, by (flat) track index. `selectionAnchor` is the
      base row for Shift-range selects. Reset whenever the view reloads. */
  selection = $state<Set<number>>(new Set());
  selectionAnchor = $state<number | null>(null);

  scanning = $state(false);
  scanProgress = $state<ScanProgress | null>(null);
  scanError = $state<string | null>(null);

  /** Whether the GROUPING view is collapsed to primary themes right now.
      Only grouping mode collapses; other modes always match exactly. */
  get groupingCollapse(): boolean {
    return this.mode === "grouping" && !prefs.values.grouping_split_multi;
  }

  async init() {
    await this.refresh();
  }

  async refresh() {
    this.stats = await call<LibraryStats>("library_stats");
    await this.loadValues();
    await this.loadAlbums();
    if (this.selectedAlbum) await this.selectAlbum(this.selectedAlbum);
  }

  async setMode(mode: BrowseMode) {
    this.mode = mode;
    this.selectedValue = null;
    this.selectedAlbum = null;
    this.albumTracks = [];
    await this.loadValues();
    await this.loadAlbums();
  }

  async setQuery(q: string | null) {
    const norm = q && q.trim() ? q : null;
    if (norm === this.query) return;
    this.query = norm;
    await this.loadValues();
    // Drop a selection the new search filtered away.
    if (
      this.selectedValue !== null &&
      !this.values.some(([v]) => v === this.selectedValue)
    ) {
      this.selectedValue = null;
      this.selectedAlbum = null;
      this.albumTracks = [];
    }
    await this.loadAlbums();
  }

  async loadValues() {
    try {
      this.values = await call<[string, number, string][]>("browse_values", {
        mode: this.mode,
        query: this.query,
        collapse: this.groupingCollapse,
      });
    } catch (e) {
      // invalid search query mid-typing — keep the previous list
      console.debug("browse_values failed:", e);
    }
  }

  async selectValue(v: string | null) {
    this.selectedValue = v;
    this.selectedAlbum = null;
    this.albumTracks = [];
    await this.loadAlbums();
  }

  async loadAlbums() {
    try {
      this.albums = await call<Album[]>("browse_albums", {
        mode: this.mode,
        value: this.selectedValue,
        query: this.query,
        collapse: this.groupingCollapse,
      });
    } catch (e) {
      console.debug("browse_albums failed:", e);
    }
  }

  async selectAlbum(a: Album | null) {
    this.selectedAlbum = a;
    this.albumTracks = a ? await call<Track[]>("album_tracks", { albumId: a.id }) : [];
  }

  /** Flat row count of the Track-list view (tracks, plus separators when on). */
  get trackListRowCount(): number {
    return this.trackModel ? this.trackModel.totalRows : this.trackCount;
  }

  /** Resolve a flat row index to a separator or a track index. */
  rowAt(i: number): TrackRow {
    const m = this.trackModel;
    if (!m) return { trackIndex: i };
    const g = this.groupIndexAtRow(i);
    const local = i - m.rowStart[g];
    if (m.hasSep[g] && local === 0) return { sep: m.groups[g], albumId: m.groups[g].album_id };
    return { trackIndex: m.trackStart[g] + (local - (m.hasSep[g] ? 1 : 0)) };
  }

  private groupIndexAtRow(i: number): number {
    const rs = this.trackModel!.rowStart;
    let lo = 0;
    let hi = rs.length - 2; // last valid group index
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (rs[mid] <= i) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  /** Track-index span covering a flat row range (whole groups at the edges). */
  private trackRangeForRows(firstRow: number, lastRow: number): [number, number] {
    const m = this.trackModel;
    if (!m) return [firstRow, lastRow];
    const g1 = this.groupIndexAtRow(Math.max(0, firstRow));
    const g2 = this.groupIndexAtRow(Math.min(lastRow, m.totalRows - 1));
    const tFirst = m.trackStart[g1];
    const tLast = m.trackStart[g2 + 1] - 1;
    return [tFirst, Math.max(tFirst, tLast)];
  }

  /** Recount + reset the Track-list view for the current (mode, value, query).
      When `withGroups`, also loads album runs for separators. Bumps trackEpoch so
      the mounted view re-fetches its visible range. */
  async reloadTrackView(withGroups: boolean) {
    const reqId = ++this.trackReqId;
    let count: number;
    try {
      count = await call<number>("track_count", {
        query: this.query,
        mode: this.mode,
        value: this.selectedValue,
        collapse: this.groupingCollapse,
      });
    } catch (e) {
      console.debug("track_count failed:", e);
      return;
    }
    if (reqId !== this.trackReqId) return; // superseded by a newer reset

    let model: RowModel | null = null;
    if (withGroups) {
      try {
        const groups = await call<TrackGroup[]>("track_groups", {
          query: this.query,
          mode: this.mode,
          value: this.selectedValue,
          collapse: this.groupingCollapse,
        });
        if (reqId !== this.trackReqId) return;
        model = buildRowModel(groups);
      } catch (e) {
        console.debug("track_groups failed:", e);
        // fall back to a flat list rather than leaving the view empty
      }
    }
    // Reset the page cache and the sparse array together, so an in-flight
    // loadTrackPage from the async window above can't leave a page marked
    // "loaded" against the array we're about to replace (which would blank
    // that page until a scroll re-triggered it — the "empty top after
    // returning from an album" bug).
    this.trackModel = model;
    this.trackCount = count;
    this.pagePromises.clear();
    this.tracks = new Array(count);
    this.clearSelection();
    this.trackEpoch++;
  }

  // --- Track-list selection ---------------------------------------------

  isSelected(index: number): boolean {
    return this.selection.has(index);
  }

  clearSelection() {
    if (this.selection.size > 0) this.selection = new Set();
    this.selectionAnchor = null;
  }

  /** Update the selection for a click on a track row. Plain click selects only
      that row; Ctrl toggles it; Shift selects the range from the anchor. */
  selectTrack(index: number, mods: { ctrl?: boolean; shift?: boolean }) {
    if (mods.shift && this.selectionAnchor != null) {
      const lo = Math.min(this.selectionAnchor, index);
      const hi = Math.max(this.selectionAnchor, index);
      const next = new Set<number>();
      for (let i = lo; i <= hi; i++) next.add(i);
      this.selection = next;
    } else if (mods.ctrl) {
      const next = new Set(this.selection);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      this.selection = next;
      this.selectionAnchor = index;
    } else {
      this.selection = new Set([index]);
      this.selectionAnchor = index;
    }
  }

  /** The selected tracks (in row order), loading any pages the selection spans
      that aren't in cache yet — so acting on a Shift-range that reaches past the
      visible window still yields every track. */
  async collectSelectedTracks(): Promise<Track[]> {
    const idx = [...this.selection].sort((a, b) => a - b);
    if (idx.length === 0) return [];
    await this.ensureTrackRangeLoaded(idx[0], idx[idx.length - 1]);
    return idx.map((i) => this.tracks[i]).filter((t): t is Track => t != null);
  }

  /** Fetch the pages covering a flat row range (mapped through any separators). */
  ensureRowRange(firstRow: number, lastRow: number) {
    const [first, last] = this.trackRangeForRows(firstRow, lastRow);
    this.ensureTrackRange(first, last);
  }

  /** Open an album (AlbumDetail) by id — used by track-list separators. */
  async openAlbumById(id: number) {
    const a = await call<Album | null>("album_by_id", { albumId: id });
    if (a) await this.selectAlbum(a);
  }

  /** Ensure every page covering track indices [first, last] is fetched
      (fire-and-forget; used by scrolling). */
  ensureTrackRange(first: number, last: number) {
    void this.ensureTrackRangeLoaded(first, last);
  }

  /** Awaitable variant: resolves once every page covering [first, last] has
      loaded (or its fetch settled). */
  async ensureTrackRangeLoaded(first: number, last: number): Promise<void> {
    if (this.trackCount === 0) return;
    const firstPage = Math.max(0, Math.floor(first / TRACK_PAGE));
    const lastPage = Math.floor(Math.min(last, this.trackCount - 1) / TRACK_PAGE);
    const pending: Promise<void>[] = [];
    for (let p = firstPage; p <= lastPage; p++) pending.push(this.loadTrackPage(p));
    await Promise.all(pending);
  }

  private loadTrackPage(page: number): Promise<void> {
    const inflight = this.pagePromises.get(page);
    if (inflight) return inflight;
    const p = this.fetchTrackPage(page);
    this.pagePromises.set(page, p);
    return p;
  }

  private async fetchTrackPage(page: number): Promise<void> {
    const reqId = this.trackReqId;
    const offset = page * TRACK_PAGE;
    let rows: Track[];
    try {
      rows = await call<Track[]>("tracks_range", {
        query: this.query,
        mode: this.mode,
        value: this.selectedValue,
        collapse: this.groupingCollapse,
        offset,
        limit: TRACK_PAGE,
      });
    } catch (e) {
      this.pagePromises.delete(page); // let a later pass retry
      console.debug("tracks_range failed:", e);
      return;
    }
    if (reqId !== this.trackReqId) return; // filter changed mid-flight; discard
    for (let i = 0; i < rows.length; i++) this.tracks[offset + i] = rows[i];
  }

  onScanProgress(p: ScanProgress) {
    this.scanProgress = p;
    this.scanning = !p.done;
    if (p.done) setTimeout(() => (this.scanProgress = null), 4000);
  }

  async startScan(full: boolean) {
    this.scanError = null;
    this.scanning = true;
    try {
      await call("scan_start", { full });
    } catch (e) {
      this.scanning = false;
      this.scanError = String(e);
    }
  }

  async cancelScan() {
    await call("scan_cancel");
  }
}

export const library = new LibraryStore();
