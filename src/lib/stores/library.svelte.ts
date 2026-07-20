import { call, type Album, type BrowseMode, type LibraryStats, type ScanProgress, type Track } from "../api";

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

  scanning = $state(false);
  scanProgress = $state<ScanProgress | null>(null);
  scanError = $state<string | null>(null);

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
      });
    } catch (e) {
      console.debug("browse_albums failed:", e);
    }
  }

  async selectAlbum(a: Album | null) {
    this.selectedAlbum = a;
    this.albumTracks = a ? await call<Track[]>("album_tracks", { albumId: a.id }) : [];
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
