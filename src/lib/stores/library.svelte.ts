import {
  call,
  type Album,
  type BrowseMode,
  type BrowseValue,
  type BrowseValuesResult,
  type FacetFilter,
  type LibraryStats,
  type ScanProgress,
  type SortKey,
  type Track,
  type TrackGroup,
  type TrackSort,
  type WireBrowseMode,
  staleProjectionRevision,
} from "../api";
import {
  durationBucketLabel,
  lastPlayedBucketLabel,
  playCountLabel,
  LAST_PLAYED_SPANS,
} from "../format";
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
  ["lastplayed", "Last played"],
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

/** How a custom filter tag reads in the tab header and the Set-filter preview.
    Only tags whose stored spelling is not how anyone would say them appear
    here; every other tag shows as it was typed. */
export function customTagLabel(tag: string): string {
  if (tag === "lastplayed" || tag === "last_played") return "last played";
  if (tag === "playcount" || tag === "play_count") return "play count";
  return tag;
}

/** Everything a library scan can be doing, from one value. */
export type ScanState =
  | { phase: "idle" }
  | { phase: "starting" }
  | { phase: "running"; progress: ScanProgress | null }
  | { phase: "cancelling"; progress: ScanProgress | null }
  | { phase: "finished"; progress: ScanProgress }
  | { phase: "failed"; message: string };

/** How long a finished scan's summary stays on screen. */
const SCAN_SUMMARY_MS = 4000;

/** Why the whole library view is being rebuilt. */
type ViewRebuild = "projection" | "recovery" | "history";

/** What a Track-list view queries; fixed for as long as that list is on screen. */
interface TrackViewParams {
  query: string | null;
  filters: FacetFilter[];
  projectionRevision: number;
  sort: TrackSort | null;
}

/** The rows on screen come from one source: its parameters and its pages, which
    hold both in-flight and loaded fetches. A reload that succeeds replaces the
    source along with the rows, so a fetch can only ever write into the list it
    was started for, and a reload that fails leaves that list fully working. */
interface TrackSource {
  /** Null until the first reload, when the current filter is what is meant. */
  params: TrackViewParams | null;
  pages: Map<number, Promise<void>>;
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

/** The left-panel categories, in the order their headers are listed. */
const FACET_MODES: BrowseMode[] = ["genre", "artist", "album", "albumartist", "custom"];

/** One left-panel category: its value list, the text narrowing that list, and
    what is selected in it. Collapsing a category hides its list and keeps its
    selection, so the tracks shown match every category's selection whether or
    not that category is open. */
export class Facet {
  readonly mode: BrowseMode;
  expanded = $state(false);
  /** Projected values and counts, loaded while the category is expanded or
      has a selection. */
  values = $state<BrowseValue[]>([]);
  /** Plain-text narrowing of the value list (not a search query). Matches the
      raw value or its sort/romaji, case-insensitively; in a duration list a
      time like "3:56" matches its minute bucket. */
  valueFilter = $state("");
  /** Empty = "All", several entries = Ctrl-multi-select (tracks matching any
      of them show). */
  selectedValues = $state<string[]>([]);
  /** Base value for Shift-range selects: the last plainly clicked or
      Ctrl-toggled value. Null until something is clicked. */
  valueAnchor = $state<string | null>(null);
  /** Orders this category's value loads so only the newest one commits. */
  reqId = 0;

  constructor(mode: BrowseMode, expanded = false) {
    this.mode = mode;
    this.expanded = expanded;
  }

  /** The custom category's single resolved tag name. Null for the built-in
      categories, and when the custom text isn't exactly one tag. */
  get customTagName(): string | null {
    return this.mode === "custom" ? parseCustomTag(prefs.values.custom_browse_tag) : null;
  }

  /** Whether the custom filter browses the grouping tag. */
  get isGrouping(): boolean {
    return this.customTagName === "grouping";
  }

  /** Whether the custom filter browses per-minute duration buckets
      (`%duration%` / `%length%`). Values are bucket numbers as strings; the UI
      renders them via durationBucketLabel. */
  get isDuration(): boolean {
    return this.customTagName === "duration" || this.customTagName === "length";
  }

  /** Whether the custom filter browses last-played buckets (`%lastplayed%`).
      Values are bucket numbers as strings ("0" = today … "5" = never played);
      the UI renders them via lastPlayedBucketLabel. */
  get isLastPlayed(): boolean {
    return this.customTagName === "lastplayed" || this.customTagName === "last_played";
  }

  /** Whether the custom filter browses play counts (`%playcount%`). Values are
      the counts themselves as strings, listed most-played first; the UI renders
      them via playCountLabel. */
  get isPlayCount(): boolean {
    return (
      this.customTagName === "playcount" ||
      this.customTagName === "play_count" ||
      this.customTagName === "plays"
    );
  }

  /** What this category's values are called, for "All (12 artists)" and the
      filter bar's placeholder. */
  get noun(): string {
    return (
      this.mode === "genre" ? "genres"
      : this.mode === "artist" ? "artists"
      : this.mode === "album" ? "albums"
      : this.mode === "albumartist" ? "album artists"
      : this.isGrouping ? "groups"
      : this.isDuration ? "durations"
      : this.isLastPlayed ? "spans"
      : this.isPlayCount ? "play counts" : "values"
    );
  }

  /** How one of this category's values reads in the list. */
  label(v: string): string {
    if (this.isDuration) return durationBucketLabel(Number(v));
    if (this.isLastPlayed) return lastPlayedBucketLabel(Number(v));
    if (this.isPlayCount) return playCountLabel(Number(v));
    return v === "" ? "?" : v;
  }

  /** The mode marshalled for the backend: `custom` becomes
      `{ custom: "<tag>" }`, every other mode stays a plain string. */
  get wireMode(): WireBrowseMode {
    if (this.mode === "custom") return { custom: this.customTagName ?? "" };
    return this.mode;
  }

  /** `values` narrowed by `valueFilter` (memoized). Empty filter -> all values.
      Duration list: a time like "3:56" narrows to its minute bucket ("3 minutes
      (3:00 - 3:59)"); any other text matches the rendered bucket labels. */
  filteredValues = $derived.by(() => {
    const f = this.valueFilter.trim().toLowerCase();
    if (!f) return this.values;
    if (this.isDuration) {
      const time = /^(\d+):\d{0,2}$/.exec(f);
      if (time) {
        const bucket = String(parseInt(time[1], 10));
        return this.values.filter(({ value }) => value === bucket);
      }
      return this.values.filter(({ value }) =>
        durationBucketLabel(Number(value)).toLowerCase().includes(f),
      );
    }
    if (this.isLastPlayed) {
      // Match the bucket name or the span beside it ("week", "7 days", …).
      return this.values.filter(({ value }) =>
        `${lastPlayedBucketLabel(Number(value))} ${LAST_PLAYED_SPANS[Number(value)] ?? ""}`
          .toLowerCase()
          .includes(f),
      );
    }
    if (this.isPlayCount) {
      // Match the count itself ("3") or its label ("3 plays", "never").
      return this.values.filter(
        ({ value }) =>
          value === f || playCountLabel(Number(value)).toLowerCase().includes(f),
      );
    }
    return this.values.filter(
      ({ value, sort_value }) =>
        value.toLowerCase().includes(f) || sort_value?.toLowerCase().includes(f),
    );
  });
}

class LibraryStore {
  /** The left-panel categories in header order. Artist starts expanded: a
      value load is also what brings the projection revision. */
  facets = FACET_MODES.map((mode) => new Facet(mode, mode === "artist"));
  albums = $state<Album[]>([]);
  selectedAlbum = $state<Album | null>(null);
  albumTracks = $state<Track[]>([]);
  /** Persisted scroll offset (px) of the Album-wall / Track-list viewports, so
      drilling into an album and coming back restores where you were. Reset to 0
      only on a genuine filter change (mode / value / query) — not when the view
      merely remounts after an album, which is what makes restoration work. */
  wallScroll = $state(0);
  trackScroll = $state(0);
  /** Changes only for a genuine filter reset; virtual lists use it to
      distinguish "scroll changed while browsing" from "scroll to top now". */
  scrollResetKey = $state(0);
  stats = $state<LibraryStats | null>(null);
  query = $state<string | null>(null);

  /** Windowed full-library track list (Track-list view). `tracks[i]` is
      undefined until its page loads. `trackEpoch` bumps on every reset so the
      view can re-fetch its visible range; `trackSource` is what those rows come
      from, and `trackReqId` orders reloads so only the newest one commits. */
  trackCount = $state(0);
  tracks = $state<(Track | undefined)[]>([]);
  trackEpoch = $state(0);
  /** Non-null when album separators are on: the flat row layout over `tracks`. */
  trackModel = $state<RowModel | null>(null);
  private trackSource: TrackSource = { params: null, pages: new Map() };
  private pendingTrackRange: [number, number] | null = null;
  private loadingVisibleTracks = false;
  private trackReqId = 0;
  private albumsReqId = 0;
  private albumTracksReqId = 0;
  /** Rebuilds run one at a time, in order. The gate closes when the first is
      queued and opens when the last has finished. */
  private rebuildQueue: Promise<void> = Promise.resolve();
  private rebuildsWaiting = 0;
  private queuedRecovery: Promise<void> | null = null;
  private recoveryRevision = 0;
  private queuedHistory: Promise<void> | null = null;
  /** A revision newer than the rebuild in progress was reading. */
  private staleDuringRebuild: number | null = null;
  /** The separators setting the view last asked for while a rebuild ran. */
  private requestedDuringRebuild: boolean | null = null;
  /** Identifies what the rows on screen were last loaded for. */
  private loadedViewKey: string | null = null;

  /** One revision backs every projection-dependent object currently rendered. */
  projectionRevision = $state(0);
  rebuild = $state<ViewRebuild | null>(null);
  get interactionBlocked(): boolean {
    return this.rebuild !== null;
  }
  /** One dismissible banner line for the library tab: a stale-projection
      retry that could not be revalidated, or a plugin menu action that
      failed. Cleared when a projection change starts. */
  notice = $state<string | null>(null);

  /** Track-list selection, by (flat) track index. `selectionAnchor` is the
      base row for Shift-range selects. Reset whenever the view reloads. */
  selection = $state<Set<number>>(new Set());
  selectionAnchor = $state<number | null>(null);

  /** One-shot intent: after the next Track-list reload, scroll the view to this
      track id (its position in the freshly-loaded view). Set when clearing the
      search bar while a track plays, so the now-playing song is revealed within
      the full library instead of snapping to the top. Consumed once. */
  scrollToTrackId = $state<number | null>(null);

  /** One-shot intent: open this album (AlbumDetail) as soon as the Library tab
      can. Set from outside the tab — the transport bar and the playlist view —
      because the tab is unmounted while another one is on screen. */
  pendingAlbumId = $state<number | null>(null);

  /** One-shot intent: reveal whatever is playing *now*. It carries no track id
      on purpose: the song is read when the view consumes the flag, so a jump
      requested from another tab shows the song playing when you come back
      rather than the one that was playing when you asked.

      `explicit` (the menu command) may also change what the tab shows — leave
      an album, or open one in the Album-wall view, which has no track rows.
      `auto` (the shuffle follow-along) only ever scrolls a list: drilling into
      an album under a browsing user on every song would be far too much. */
  pendingJump = $state<"explicit" | "auto" | null>(null);

  scan = $state<ScanState>({ phase: "idle" });
  private scanSummaryTimer: ReturnType<typeof setTimeout> | null = null;

  get scanning(): boolean {
    const phase = this.scan.phase;
    return phase === "starting" || phase === "running" || phase === "cancelling";
  }

  /** The latest numbers from the scan in progress, if it has reported any. */
  get scanProgressNow(): ScanProgress | null {
    return this.scan.phase === "running" || this.scan.phase === "cancelling"
      ? this.scan.progress
      : null;
  }

  facet(mode: BrowseMode): Facet {
    return this.facets.find((facet) => facet.mode === mode)!;
  }

  /** The categories whose value lists are in use: shown, or needed to tell
      whether a selection still exists. */
  private liveFacets(): Facet[] {
    return this.facets.filter((facet) => facet.expanded || facet.selectedValues.length > 0);
  }

  /** Every category's selection as the backend takes it. A track shows when
      it matches each entry; no entries means the whole library. */
  activeFilters = $derived.by((): FacetFilter[] =>
    this.facets
      .filter((facet) => facet.selectedValues.length > 0)
      .map((facet) => ({ mode: facet.wireMode, values: facet.selectedValues })),
  );

  /** The selections a row of `facet` stands for: `values` in that category,
      and whatever every other category has selected. */
  filtersWith(facet: Facet, values: string[]): FacetFilter[] {
    return this.facets
      .map((other) => ({
        mode: other.wireMode,
        values: other === facet ? values : other.selectedValues,
      }))
      .filter((filter) => filter.values.length > 0);
  }

  /** Raw text of the custom filter (persisted centrally in prefs). */
  get customTag(): string {
    return prefs.values.custom_browse_tag;
  }

  /** The custom filter's single resolved tag name, or null when the text isn't
      exactly one tag. Drives the header label and whether the value list loads. */
  customTagName = $derived.by(() => parseCustomTag(prefs.values.custom_browse_tag));

  /** Set (and persist) the custom filter text, expand the custom category, and
      reload it — configuring the filter implies you want to use it. */
  async setCustomTag(raw: string) {
    await this.rebuildQueue;
    await prefs.set("custom_browse_tag", raw);
    const facet = this.facet("custom");
    if (prefs.values.filter_panel_layout === "tabs") this.closeOthers(facet);
    facet.expanded = true;
    facet.selectedValues = [];
    facet.valueAnchor = null;
    this.selectedAlbum = null;
    this.albumTracks = [];
    this.resetScroll();
    await this.loadValues(facet);
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

  async init() {
    await this.refresh();
    // A webview reload during a scan starts with no idea one is running.
    try {
      if ((await call<boolean>("scan_running")) && this.scan.phase === "idle") {
        this.scan = { phase: "running", progress: null };
      }
    } catch (e) {
      console.debug("scan_running failed:", e);
    }
  }

  async refresh() {
    this.stats = await call<LibraryStats>("library_stats");
    await this.loadAllValues();
    await this.loadAlbums();
    if (this.selectedAlbum) await this.selectAlbum(this.selectedAlbum);
  }

  /** Expand or collapse a category. Its selection stays either way; only the
      text narrowing its list goes with the list. */
  async toggleFacet(facet: Facet) {
    if (!facet.expanded) return this.openFacet(facet);
    facet.expanded = false;
    facet.valueFilter = "";
  }

  /** Show a category's value list. The tabs layout shows one list at a time,
      so there the others close. */
  async openFacet(facet: Facet) {
    if (prefs.values.filter_panel_layout === "tabs") this.closeOthers(facet);
    if (facet.expanded) return;
    facet.expanded = true;
    await this.loadValues(facet);
  }

  /** Close every category but this one. Their selections stay. */
  private closeOthers(facet: Facet) {
    for (const other of this.facets) {
      if (other === facet || !other.expanded) continue;
      other.expanded = false;
      other.valueFilter = "";
    }
  }

  /** The category the tabs layout shows: the first open one, or Artist when
      the vertical list was left with none open. */
  async showOneFacet() {
    await this.openFacet(this.facets.find((facet) => facet.expanded) ?? this.facet("artist"));
  }

  /** Drop every category's selection, so the whole library (or search result)
      shows again. The text narrowing each list and the search stay. */
  async clearFilters() {
    if (this.activeFilters.length === 0) return;
    for (const facet of this.facets) {
      facet.selectedValues = [];
      facet.valueAnchor = null;
    }
    this.selectedAlbum = null;
    this.albumTracks = [];
    this.resetScroll();
    await this.loadAlbums();
  }

  async setQuery(q: string | null) {
    const norm = q && q.trim() ? q : null;
    if (norm === this.query) return;
    this.query = norm;
    // Before any await: the query change starts the track list reload, which
    // may reveal the playing track. Resetting after loadValues() (a GROUP BY
    // over the library) used to land later and throw that reveal away.
    this.resetScroll();
    await Promise.all(
      this.liveFacets().map(async (facet) => {
        const current = await this.loadValues(facet);
        // Drop the parts of the selection the new search filtered away. Only
        // `values` from this search can say that; a failed or superseded load
        // cannot.
        if (!current || facet.selectedValues.length === 0) return;
        const still = facet.selectedValues.filter((v) =>
          facet.values.some(({ value }) => value === v),
        );
        if (still.length === facet.selectedValues.length) return;
        facet.selectedValues = still;
        if (still.length === 0) {
          this.selectedAlbum = null;
          this.albumTracks = [];
        }
      }),
    );
    await this.loadAlbums();
  }

  /** Forget the saved viewport offsets (see `wallScroll` / `trackScroll`). */
  resetScroll() {
    this.wallScroll = 0;
    this.trackScroll = 0;
    this.scrollResetKey += 1;
  }

  /** Whether the category's `values` now reflects this request. */
  async loadValues(facet: Facet): Promise<boolean> {
    const request = ++facet.reqId;
    // Custom category with no single valid tag set → empty value list ("show nothing").
    if (facet.mode === "custom" && facet.customTagName === null) {
      facet.values = [];
      return true;
    }
    try {
      const result = await call<BrowseValuesResult>("browse_values", {
        mode: facet.wireMode,
        query: this.query,
      });
      if (request !== facet.reqId) return false;
      this.projectionRevision = result.projection_revision;
      facet.values = result.values;
      return true;
    } catch (e) {
      // invalid search query mid-typing — keep the previous list
      console.debug("browse_values failed:", e);
      return false;
    }
  }

  private async loadAllValues() {
    await Promise.all(this.liveFacets().map((facet) => this.loadValues(facet)));
  }

  private async reloadView() {
    await this.loadAllValues();
    await Promise.all([
      this.loadAlbums(),
      this.reloadTrackView(prefs.values.track_separators),
    ]);
  }

  /** Run `body` after any rebuild ahead of it, with the gate shut throughout. */
  private queueRebuild(kind: ViewRebuild, body: () => Promise<void>): Promise<void> {
    if (this.rebuildsWaiting === 0) this.invalidateViewRequests();
    this.rebuildsWaiting++;
    this.rebuild ??= kind;
    const run = this.rebuildQueue
      .then(() => {
        this.rebuild = kind;
        this.invalidateViewRequests();
        return body();
      })
      .finally(() => {
        this.rebuildsWaiting--;
        if (this.rebuildsWaiting === 0) this.afterRebuilds();
      });
    this.rebuildQueue = run.catch(() => {});
    return run;
  }

  private afterRebuilds() {
    const stale = this.staleDuringRebuild;
    this.staleDuringRebuild = null;
    if (stale !== null && stale !== this.projectionRevision) {
      void this.queueRecovery(stale);
      return;
    }
    this.rebuild = null;
    const requested = this.requestedDuringRebuild;
    this.requestedDuringRebuild = null;
    if (requested !== null && this.trackViewKey(requested) !== this.loadedViewKey) {
      void this.reloadTrackView(requested);
    }
  }

  /** Serialize a setting write with complete invalidation and reconstruction
      of every object whose identity depends on the projection. A write that
      fails still reloads, since queuing it invalidated what was loading. */
  changeProjection(write: () => Promise<number>): Promise<void> {
    this.notice = null;
    return this.queueRebuild("projection", async () => {
      let written = false;
      try {
        this.projectionRevision = await write();
        written = true;
        this.clearProjectionState();
      } finally {
        if (!written) await this.reloadView();
      }
      await this.reloadView();
    });
  }

  /** Refresh once after a backend stale-revision rejection. */
  async recoverStaleProjection(reason: unknown): Promise<boolean> {
    const revision = staleProjectionRevision(reason);
    if (revision == null) return false;
    await this.queueRecovery(revision);
    return true;
  }

  private queueRecovery(revision: number): Promise<void> {
    this.recoveryRevision = revision;
    this.queuedRecovery ??= this.queueRebuild("recovery", async () => {
      this.queuedRecovery = null;
      this.projectionRevision = this.recoveryRevision;
      this.clearProjectionState();
      await this.reloadView();
    });
    return this.queuedRecovery;
  }

  /** Handle a stale-revision error from a load. True if that is what it was. A
      rebuild is waiting on this very load, so it notes the revision for afterwards
      rather than queuing behind itself. */
  private async recoverLoad(reason: unknown): Promise<boolean> {
    const revision = staleProjectionRevision(reason);
    if (revision == null) return false;
    if (this.rebuild !== null) {
      this.staleDuringRebuild = revision;
      return true;
    }
    await this.queueRecovery(revision);
    return true;
  }

  private invalidateViewRequests() {
    for (const facet of this.facets) facet.reqId++;
    this.albumsReqId++;
    this.albumTracksReqId++;
    this.trackReqId++;
    this.pendingTrackRange = null;
    this.trackSource = { params: this.trackSource.params, pages: new Map() };
  }

  private clearProjectionState() {
    for (const facet of this.facets) {
      facet.values = [];
      facet.selectedValues = [];
      facet.valueAnchor = null;
    }
    this.albums = [];
    this.selectedAlbum = null;
    this.albumTracks = [];
    this.trackModel = null;
    this.trackCount = 0;
    this.tracks = [];
    this.clearSelection();
    this.resetScroll();
  }

  /** Select exactly this value in its category (or "All" for null). */
  async selectValue(facet: Facet, v: string | null) {
    facet.valueAnchor = v;
    await this.applyValueSelection(facet, v === null ? [] : [v]);
  }

  /** Toggle this value in/out of its category's multi-selection. */
  async toggleValue(facet: Facet, v: string) {
    facet.valueAnchor = v;
    await this.applyValueSelection(
      facet,
      facet.selectedValues.includes(v)
        ? facet.selectedValues.filter((x) => x !== v)
        : [...facet.selectedValues, v],
    );
  }

  /** Shift+click: select the visible range between the anchor (the last
      plainly clicked or toggled value) and this one, replacing the selection.
      Without a usable anchor — none yet, or it was filtered out of the visible
      list — acts like a plain click. The anchor stays, so a second Shift+click
      re-ranges from the same base (matching the Track-list behavior). */
  async selectValueRange(facet: Facet, v: string) {
    const visible = facet.filteredValues.map(({ value }) => value);
    const a = facet.valueAnchor === null ? -1 : visible.indexOf(facet.valueAnchor);
    const b = visible.indexOf(v);
    if (a < 0 || b < 0) return this.selectValue(facet, v);
    const [lo, hi] = a <= b ? [a, b] : [b, a];
    await this.applyValueSelection(facet, visible.slice(lo, hi + 1));
  }

  private async applyValueSelection(facet: Facet, values: string[]) {
    facet.selectedValues = values;
    this.selectedAlbum = null;
    this.albumTracks = [];
    this.resetScroll();
    await this.loadAlbums();
  }

  async loadAlbums() {
    const request = ++this.albumsReqId;
    try {
      const albums = await call<Album[]>("browse_albums", {
        filters: this.activeFilters,
        query: this.query,
        projectionRevision: this.projectionRevision,
      });
      if (request === this.albumsReqId) this.albums = albums;
    } catch (e) {
      if (await this.recoverLoad(e)) return;
      console.debug("browse_albums failed:", e);
    }
  }

  async selectAlbum(a: Album | null) {
    const request = ++this.albumTracksReqId;
    this.selectedAlbum = a;
    this.albumTracks = [];
    if (!a) return;
    try {
      const tracks = await call<Track[]>("album_tracks", { albumId: a.id });
      if (request === this.albumTracksReqId && this.selectedAlbum?.id === a.id) {
        this.albumTracks = tracks;
      }
    } catch (e) {
      if (request === this.albumTracksReqId) console.debug("album_tracks failed:", e);
    }
  }

  /** Patch the play counter, then rebuild views whose membership or order
      can change when the player credits a play. */
  async bumpPlayCount(id: number, plays: number) {
    for (const t of this.albumTracks) if (t.id === id) t.play_count = plays;
    for (const t of this.tracks) if (t && t.id === id) t.play_count = plays;
    // Any search may depend on play-count syntax (including aliases). Re-run
    // it rather than maintaining a second search parser in the frontend.
    const custom = this.facet("custom");
    const customByPlays =
      (custom.expanded || custom.selectedValues.length > 0) &&
      (custom.isPlayCount || custom.isLastPlayed);
    if (
      !this.query && !customByPlays &&
      this.sort?.key !== "lastplayed" && this.sort?.secondary !== "lastplayed"
    ) return;
    this.clearSelection();
    this.queuedHistory ??= this.queueRebuild("history", async () => {
      // Plays credited from here on need another rebuild; this one has started.
      this.queuedHistory = null;
      await this.loadAllValues();
      await Promise.all([
        this.loadAlbums(),
        this.reloadTrackView(prefs.values.track_separators),
        this.selectedAlbum ? this.selectAlbum(this.selectedAlbum) : Promise.resolve(),
      ]);
    });
    return this.queuedHistory;
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
    const query = () => call<number | null>("track_index", {
      ...this.trackParams(),
      trackId: id,
    });
    try {
      return await query();
    } catch (e) {
      if (this.rebuild === null && (await this.recoverStaleProjection(e))) {
        try {
          return await query();
        } catch {
          return null;
        }
      }
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

  /** Only the tracks in the visible rows, even within a very large album run. */
  private trackRangeForRows(firstRow: number, lastRow: number): [number, number] {
    const m = this.trackModel;
    if (!m) return [firstRow, lastRow];
    if (m.totalRows === 0 || lastRow < firstRow) return [0, -1];
    firstRow = Math.max(0, firstRow);
    lastRow = Math.min(lastRow, m.totalRows - 1);
    const g1 = this.groupIndexAtRow(firstRow);
    const g2 = this.groupIndexAtRow(lastRow);
    const tFirst = m.trackStart[g1] + Math.max(
      0, firstRow - m.rowStart[g1] - (m.hasSep[g1] ? 1 : 0),
    );
    const tLast = m.trackStart[g2] + lastRow - m.rowStart[g2] - (m.hasSep[g2] ? 1 : 0);
    return [tFirst, tLast];
  }

  /** What a list loaded right now would query. */
  private liveTrackParams(): TrackViewParams {
    return {
      query: this.query,
      filters: this.activeFilters,
      projectionRevision: this.projectionRevision,
      sort: this.sort,
    };
  }

  /** What the rows on screen come from. */
  private trackParams(): TrackViewParams {
    return this.trackSource.params ?? this.liveTrackParams();
  }

  private trackViewKey(withGroups: boolean): string {
    return JSON.stringify([this.liveTrackParams(), withGroups, this.stats]);
  }

  /** The view's inputs changed. A rebuild in progress reconstructs the view
      itself, so wait for it and reload only if it read something older. */
  requestTrackView(withGroups: boolean) {
    if (this.rebuild !== null) {
      this.requestedDuringRebuild = withGroups;
      return;
    }
    return this.reloadTrackView(withGroups);
  }

  /** Recount + reset the Track-list view for the current (selections, query).
      When `withGroups`, also loads album runs for separators. Bumps trackEpoch so
      the mounted view re-fetches its visible range. Until it succeeds, the rows
      on screen and their source stay exactly as they were. */
  async reloadTrackView(withGroups: boolean) {
    const reqId = ++this.trackReqId;
    const key = this.trackViewKey(withGroups);
    const params = this.liveTrackParams();
    const { query, filters, projectionRevision, sort } = params;
    let count: number;
    try {
      count = await call<number>("track_count", { query, filters, projectionRevision });
    } catch (e) {
      if (await this.recoverLoad(e)) return;
      console.debug("track_count failed:", e);
      return;
    }
    if (reqId !== this.trackReqId) return; // superseded by a newer reset

    let model: RowModel | null = null;
    if (withGroups) {
      try {
        const groups = await call<TrackGroup[]>("track_groups", {
          query, filters, projectionRevision, sort,
        });
        if (reqId !== this.trackReqId) return;
        model = buildRowModel(groups);
      } catch (e) {
        if (await this.recoverLoad(e)) return;
        console.debug("track_groups failed:", e);
        // fall back to a flat list rather than leaving the view empty
      }
    }
    this.trackSource = { params, pages: new Map() };
    this.loadedViewKey = key;
    this.pendingTrackRange = null;
    this.trackModel = model;
    this.trackCount = count;
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
  async collectSelectedTracks(limit = Number.POSITIVE_INFINITY): Promise<Track[]> {
    if (this.interactionBlocked) return [];
    const source = this.trackSource;
    const idx = [...this.selection].sort((a, b) => a - b).slice(0, limit);
    if (idx.length === 0) return [];
    const pages = [...new Set(idx.map((i) => Math.floor(i / TRACK_PAGE)))];
    // Sparse selections need only their own pages, including context menus.
    // Bound concurrent IPC for large Shift selections as well.
    for (let i = 0; i < pages.length; i += 4) {
      if (source !== this.trackSource || this.interactionBlocked) return [];
      await Promise.all(pages.slice(i, i + 4).map((page) => this.loadTrackPage(page)));
    }
    if (source !== this.trackSource || this.interactionBlocked) return [];
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
    this.pendingTrackRange = [first, last];
    if (!this.loadingVisibleTracks) void this.loadVisibleTracks();
  }

  /** Keep only the newest viewport while a page is in flight. Scrollbar drags
      must not queue database work for every position the pointer passed. */
  private async loadVisibleTracks(): Promise<void> {
    this.loadingVisibleTracks = true;
    try {
      while (this.pendingTrackRange) {
        const [first, last] = this.pendingTrackRange;
        this.pendingTrackRange = null;
        const source = this.trackSource;
        if (this.trackCount === 0 || last < first) continue;
        const firstPage = Math.max(0, Math.floor(first / TRACK_PAGE));
        const lastPage = Math.floor(Math.min(last, this.trackCount - 1) / TRACK_PAGE);
        for (let page = firstPage; page <= lastPage; page++) {
          await this.loadTrackPage(page);
          if (this.pendingTrackRange || source !== this.trackSource) break;
        }
      }
    } finally {
      this.loadingVisibleTracks = false;
    }
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
    const source = this.trackSource;
    const inflight = source.pages.get(page);
    if (inflight) return inflight;
    const p = this.fetchTrackPage(source, page);
    source.pages.set(page, p);
    return p;
  }

  private async fetchTrackPage(source: TrackSource, page: number): Promise<void> {
    const offset = page * TRACK_PAGE;
    let rows: Track[];
    try {
      rows = await call<Track[]>("tracks_range", {
        ...(source.params ?? this.liveTrackParams()),
        offset,
        limit: TRACK_PAGE,
      });
    } catch (e) {
      source.pages.delete(page); // let a later pass retry
      if (source !== this.trackSource) return;
      if (await this.recoverLoad(e)) return;
      console.debug("tracks_range failed:", e);
      return;
    }
    if (source !== this.trackSource) return; // that list is gone; discard
    for (let i = 0; i < rows.length; i++) this.tracks[offset + i] = rows[i];
  }

  // --- Scan -------------------------------------------------------------

  private clearScanSummaryTimer() {
    if (this.scanSummaryTimer !== null) clearTimeout(this.scanSummaryTimer);
    this.scanSummaryTimer = null;
  }

  onScanProgress(progress: ScanProgress) {
    if (progress.done) {
      this.clearScanSummaryTimer();
      this.scan = { phase: "finished", progress };
      this.scanSummaryTimer = setTimeout(() => {
        this.scanSummaryTimer = null;
        this.scan = { phase: "idle" };
      }, SCAN_SUMMARY_MS);
      return;
    }
    this.scan = { phase: this.scan.phase === "cancelling" ? "cancelling" : "running", progress };
  }

  onScanFailed(message: string) {
    this.clearScanSummaryTimer();
    this.scan = { phase: "failed", message };
  }

  dismissScanFailure() {
    if (this.scan.phase === "failed") this.scan = { phase: "idle" };
  }

  async startScan(full: boolean) {
    this.clearScanSummaryTimer();
    this.scan = { phase: "starting" };
    try {
      await call("scan_start", { full });
    } catch (e) {
      this.onScanFailed(String(e));
    }
  }

  async cancelScan() {
    if (this.scan.phase !== "running") return;
    this.scan = { phase: "cancelling", progress: this.scan.progress };
    await call("scan_cancel");
  }
}

export const library = new LibraryStore();
