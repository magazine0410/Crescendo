import { call, type M3uImportResult, type PlaylistInfo, type Track } from "../api";

class PlaylistsStore {
  list = $state<PlaylistInfo[]>([]);
  selectedId = $state<number | null>(null);
  tracks = $state<Track[]>([]);
  lastImport = $state<M3uImportResult | null>(null);

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
