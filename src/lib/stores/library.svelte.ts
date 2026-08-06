import {
  call,
  type Album,
  type BrowseMode,
  type LibraryStats,
  type ScanProgress,
  type SortKey,
  type Track,
  type TrackGroup,
  type TrackSort,
  type WireBrowseMode,
} from "../api";
import { durationBucketLabel } from "../format";
import { prefs } from "./prefs.svelte";

/** Track-list sort keys with their dropdown labels, in menu order. */
export const SORT_KEYS: [SortKey, string][] = [
  ["albumartist", "Album Artist"],
  ["artist", "Artist"],
  ["album", "Album"],
  ["genre", "Genre"],
  ["grouping", "Grouping"],
  ["year", "Year"],
  ["title", "Title"],
  ["duration", "Duration"],
];

function isSortKey(s: string): s is SortKey {
  return SORT_KEYS.some(([k]) => k === s);
}

/** Parse the `track_sort` pref (`key:secondary:asc|desc:asc|desc`, the last
    part blank while the secondary direction follows the primary). Blank or
    anything unrecognized falls back to null = default LIBRARY_SORT order. */
export function parseTrackSort(raw: string): TrackSort | null {
  const [key, secondary, dir, secDir] = raw.split(":");
  if (!key || !isSortKey(key)) return null;
  return {
    key,
    secondary: secondary && isSortKey(secondary) && secondary !== key ? secondary : null,
    descending: dir === "desc",
    secondary_descending: secDir === "desc" ? true : secDir === "asc" ? false : null,
  };
}

function serializeTrackSort(s: TrackSort): string {
  const secDir =
    s.secondary_descending == null ? "" : s.secondary_descending ? "desc" : "asc";
  return `${s.key}:${s.secondary ?? ""}:${s.descending ? "desc" : "asc"}:${secDir}`;
}

/** Rows fetched per windowed page in the Track-list view. */
const TRACK_PAGE = 200;

/** Parse the raw "custom" filter text into a single tag name, or null when it
    isn't exactly one tag. Accepts one `%tag%` token (matching the app's display
    placeholders) or a single bare word; anything with zero or several `%…%`
    tokens, or extra literal text, is rejected so the value list shows nothing. */
export function parseCustomTag(raw: string): string | null {
  const s = raw.trim();
  if (!s) return null;
  const tokens = s.match(/%[^%]+%/g);
  if (tokens) {
    // Exactly one %…% token and no surrounding literal text.
    if (tokens.length === 1 && s === tokens[0]) {
      return s.slice(1, -1).trim().toLowerCase() || null;
    }
    return null;
  }
  // No %…% at all: accept a single bare word (e.g. `year`).
  return /^\w+$/.test(s) ? s.toLowerCase() : null;
}

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
  /** Plain-text narrowing of the left-panel value list (not a search query).
      Matches the raw value or its sort/romaji, case-insensitively; in duration
      mode a time like "3:56" matches its minute bucket. */
  valueFilter = $state("");
  /** Left-panel selection: empty = "All", several entries = Ctrl-multi-select
      (tracks matching any of them show). */
  selectedValues = $state<string[]>([]);
  /** Base value for Shift-range selects in the left panel: the last plainly
      clicked or Ctrl-toggled value. Null until something is clicked. */
  valueAnchor = $state<string | null>(null);
  albums = $state<Album[]>([]);
  selectedAlbum = $state<Album | null>(null);
  albumTracks = $state<Track[]>([]);
  /** Persisted scroll offset (px) of the Album-wall / Track-list viewports, so
      drilling into an album and coming back restores where you were. Reset to 0
      only on a genuine filter change (mode / value / query) — not when the view
      merely remounts after an album, which is what makes restoration work. */
  wallScroll = $state(0);
  trackScroll = $state(0);
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

  /** One-shot intent: after the next Track-list reload, scroll the view to this
      track id (its position in the freshly-loaded view). Set when clearing the
      search bar while a track plays, so the now-playing song is revealed within
      the full library instead of snapping to the top. Consumed once. */
  scrollToTrackId = $state<number | null>(null);

  scanning = $state(false);
  scanProgress = $state<ScanProgress | null>(null);
  scanError = $state<string | null>(null);

  /** Whether the active fixed/custom filter browses the grouping tag. */
  get isGroupingMode(): boolean {
    return (
      this.mode === "grouping" ||
      (this.mode === "custom" && this.customTagName === "grouping")
    );
  }

  /** Whether the GROUPING custom view is collapsed to primary themes right now.
      Other modes always match exactly. */
  get groupingCollapse(): boolean {
    return this.isGroupingMode && !prefs.values.grouping_split_multi;
  }

  /** Whether the custom filter browses per-minute duration buckets
      (`%duration%` / `%length%`). Values are bucket numbers as strings; the UI
      renders them via durationBucketLabel. */
  get isDurationMode(): boolean {
    return (
      this.mode === "custom" &&
      (this.customTagName === "duration" || this.customTagName === "length")
    );
  }

  /** Raw text of the custom filter (persisted centrally in prefs). */
  get customTag(): string {
    return prefs.values.custom_browse_tag;
  }

  /** The custom filter's single resolved tag name, or null when the text isn't
      exactly one tag. Drives the tab label and whether the value list loads. */
  customTagName = $derived.by(() => parseCustomTag(prefs.values.custom_browse_tag));

  /** The active mode marshalled for the backend: `custom` becomes
      `{ custom: "<tag>" }`, every other mode stays a plain string. Passed to all
      value/album/track queries so the custom tab filters like the built-in ones. */
  get wireMode(): WireBrowseMode {
    if (this.mode === "custom") return { custom: this.customTagName ?? "" };
    return this.mode;
  }

  /** Set (and persist) the custom filter text, switch to the custom tab, and
      reload it — configuring the filter implies you want to use it. */
  async setCustomTag(raw: string) {
    await prefs.set("custom_browse_tag", raw);
    this.mode = "custom";
    this.selectedValues = [];
    this.valueAnchor = null;
    this.selectedAlbum = null;
    this.albumTracks = [];
    this.resetScroll();
    await this.loadValues();
    await this.loadAlbums();
  }

  /** Explicit Track-list sort (persisted in prefs), or null = default order. */
  sort = $derived.by(() => parseTrackSort(prefs.values.track_sort));

  /** Change the primary sort key (null = back to the default order, which also
      drops the secondary key and directions). Keeps the rest of the selection. */
  async setSortKey(key: SortKey | null) {
    const cur = this.sort;
    await prefs.set(
      "track_sort",
      key === null
        ? ""
        : serializeTrackSort({
            key,
            secondary: cur?.secondary === key ? null : (cur?.secondary ?? null),
            descending: cur?.descending ?? false,
            secondary_descending: cur?.secondary_descending ?? null,
          }),
    );
  }

  /** Change the secondary sort key. Turning it off (null) also puts its
      direction back to following the primary. */
  async setSortSecondary(secondary: SortKey | null) {
    const cur = this.sort;
    if (!cur) return;
    await prefs.set(
      "track_sort",
      serializeTrackSort({
        ...cur,
        secondary,
        secondary_descending: secondary === null ? null : cur.secondary_descending,
      }),
    );
  }

  async toggleSortDirection() {
    const cur = this.sort;
    if (!cur) return;
    await prefs.set(
      "track_sort",
      serializeTrackSort({ ...cur, descending: !cur.descending }),
    );
  }

  /** Effective secondary direction: its own when set, else the primary's. */
  get sortSecondaryDescending(): boolean {
    const s = this.sort;
    return s ? (s.secondary_descending ?? s.descending) : false;
  }

  /** Flip the secondary direction. The first click detaches it from the
      primary (it flips from whatever it currently shows and stays explicit). */
  async toggleSortSecondaryDirection() {
    const cur = this.sort;
    if (!cur || !cur.secondary) return;
    await prefs.set(
      "track_sort",
      serializeTrackSort({
        ...cur,
        secondary_descending: !this.sortSecondaryDescending,
      }),
    );
  }

  /** `values` narrowed by `valueFilter` (memoized). Empty filter -> all values.
      Duration mode: a time like "3:56" narrows to its minute bucket ("3 minutes
      (3:00 - 3:59)"); any other text matches the rendered bucket labels. */
  filteredValues = $derived.by(() => {
    const f = this.valueFilter.trim().toLowerCase();
    if (!f) return this.values;
    if (this.isDurationMode) {
      const time = /^(\d+):\d{0,2}$/.exec(f);
      if (time) {
        const bucket = String(parseInt(time[1], 10));
        return this.values.filter(([v]) => v === bucket);
      }
      return this.values.filter(([v]) =>
        durationBucketLabel(Number(v)).toLowerCase().includes(f),
      );
    }
    return this.values.filter(
      ([v, , sort]) => v.toLowerCase().includes(f) || sort.toLowerCase().includes(f),
    );
  });

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
    this.selectedValues = [];
    this.valueAnchor = null;
    this.selectedAlbum = null;
    this.albumTracks = [];
    this.valueFilter = "";
    this.resetScroll();
    await this.loadValues();
    await this.loadAlbums();
  }

  async setQuery(q: string | null) {
    const norm = q && q.trim() ? q : null;
    if (norm === this.query) return;
    this.query = norm;
    await this.loadValues();
    // Drop the parts of the selection the new search filtered away.
    if (this.selectedValues.length > 0) {
      const still = this.selectedValues.filter((v) =>
        this.values.some(([x]) => x === v),
      );
      if (still.length !== this.selectedValues.length) {
        this.selectedValues = still;
        if (still.length === 0) {
          this.selectedAlbum = null;
          this.albumTracks = [];
        }
      }
    }
    this.resetScroll();
    await this.loadAlbums();
  }

  /** Forget the saved viewport offsets (see `wallScroll` / `trackScroll`). */
  resetScroll() {
    this.wallScroll = 0;
    this.trackScroll = 0;
  }

  async loadValues() {
    // Custom tab with no single valid tag set → empty value list ("show nothing").
    if (this.mode === "custom" && this.customTagName === null) {
      this.values = [];
      return;
    }
    try {
      this.values = await call<[string, number, string][]>("browse_values", {
        mode: this.wireMode,
        query: this.query,
        collapse: this.groupingCollapse,
      });
    } catch (e) {
      // invalid search query mid-typing — keep the previous list
      console.debug("browse_values failed:", e);
    }
  }

  /** Plain click: select exactly this value (or "All" for null). */
  async selectValue(v: string | null) {
    this.valueAnchor = v;
    await this.applyValueSelection(v === null ? [] : [v]);
  }

  /** Ctrl+click: toggle this value in/out of the multi-selection. */
  async toggleValue(v: string) {
    this.valueAnchor = v;
    await this.applyValueSelection(
      this.selectedValues.includes(v)
        ? this.selectedValues.filter((x) => x !== v)
        : [...this.selectedValues, v],
    );
  }

  /** Shift+click: select the visible range between the anchor (the last
      plainly clicked or toggled value) and this one, replacing the selection.
      Without a usable anchor — none yet, or it was filtered out of the visible
      list — acts like a plain click. The anchor stays, so a second Shift+click
      re-ranges from the same base (matching the Track-list behavior). */
  async selectValueRange(v: string) {
    const visible = this.filteredValues.map(([x]) => x);
    const a = this.valueAnchor === null ? -1 : visible.indexOf(this.valueAnchor);
    const b = visible.indexOf(v);
    if (a < 0 || b < 0) return this.selectValue(v);
    const [lo, hi] = a <= b ? [a, b] : [b, a];
    await this.applyValueSelection(visible.slice(lo, hi + 1));
  }

  private async applyValueSelection(values: string[]) {
    this.selectedValues = values;
    this.selectedAlbum = null;
    this.albumTracks = [];
    this.resetScroll();
    await this.loadAlbums();
  }

  async loadAlbums() {
    try {
      this.albums = await call<Album[]>("browse_albums", {
        mode: this.wireMode,
        values: this.selectedValues,
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

  /** Patch a track's play counter in the loaded views so the "Plays: N" line
      updates live when the player credits a play. */
  bumpPlayCount(id: number, plays: number) {
    for (const t of this.albumTracks) if (t.id === id) t.play_count = plays;
    for (const t of this.tracks) if (t && t.id === id) t.play_count = plays;
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

  /** Inverse of `rowAt` for a track: the flat row index a given (flat) track
      index maps to, honoring album separators. */
  rowForTrackIndex(trackIndex: number): number {
    const m = this.trackModel;
    if (!m) return trackIndex;
    const ts = m.trackStart;
    let lo = 0;
    let hi = ts.length - 2; // last valid group index
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (ts[mid] <= trackIndex) lo = mid;
      else hi = mid - 1;
    }
    return m.rowStart[lo] + (m.hasSep[lo] ? 1 : 0) + (trackIndex - m.trackStart[lo]);
  }

  /** 0-based index of a track within the current Track-list view (same filter +
      order as the loaded rows), or null if it isn't in the view. */
  async indexOfTrack(id: number): Promise<number | null> {
    try {
      return await call<number | null>("track_index", {
        query: this.query,
        mode: this.wireMode,
        values: this.selectedValues,
        collapse: this.groupingCollapse,
        sort: this.sort,
        trackId: id,
      });
    } catch (e) {
      console.debug("track_index failed:", e);
      return null;
    }
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
        mode: this.wireMode,
        values: this.selectedValues,
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
          mode: this.wireMode,
          values: this.selectedValues,
          collapse: this.groupingCollapse,
          sort: this.sort,
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
        mode: this.wireMode,
        values: this.selectedValues,
        collapse: this.groupingCollapse,
        sort: this.sort,
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
