import {
  call,
  type DirectoryStation,
  type RadioCountry,
  type RadioSearch,
  type RadioTag,
  type Station,
} from "$lib/api";

const PAGE_SIZE = 100;

function message(error: unknown): string {
  return typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
}

/** Saved stations and the Radio Browser directory search behind the Radio tab. */
class RadioStore {
  stations = $state<Station[]>([]);
  loaded = $state(false);

  query = $state("");
  tag = $state("");
  countrycode = $state("");
  results = $state<DirectoryStation[]>([]);
  searching = $state(false);
  searchError = $state<string | null>(null);
  /** The last page came back full, so another may follow. */
  hasMore = $state(false);
  countries = $state<RadioCountry[]>([]);
  tags = $state<RadioTag[]>([]);
  /** Edits to the saved list that failed, shown until dismissed. */
  notice = $state<string | null>(null);

  /** Bumped per search, so a slow answer never replaces a newer one. */
  private searchSeq = 0;

  /** Directory ids already saved, for the save toggle in the results. */
  get savedUuids(): Set<string> {
    return new Set(this.stations.flatMap((s) => (s.uuid ? [s.uuid] : [])));
  }

  async load() {
    try {
      this.stations = await call<Station[]>("radio_stations");
    } catch (error) {
      this.notice = message(error);
    }
    this.loaded = true;
  }

  /** The genre and country lists; the directory is asked once per session. */
  async loadFilters() {
    if (this.countries.length && this.tags.length) return;
    try {
      const [countries, tags] = await Promise.all([
        call<RadioCountry[]>("radio_countries"),
        call<RadioTag[]>("radio_tags"),
      ]);
      this.countries = countries;
      this.tags = tags;
    } catch (error) {
      this.searchError = message(error);
    }
  }

  /** Search from the top, or fetch the next page with `more`. */
  async search(more = false) {
    const seq = ++this.searchSeq;
    const search: RadioSearch = {
      name: this.query,
      tag: this.tag,
      countrycode: this.countrycode,
      offset: more ? this.results.length : 0,
    };
    this.searching = true;
    this.searchError = null;
    try {
      const page = await call<DirectoryStation[]>("radio_search", { search });
      if (seq !== this.searchSeq) return;
      this.results = more ? [...this.results, ...page] : page;
      this.hasMore = page.length >= PAGE_SIZE;
    } catch (error) {
      if (seq !== this.searchSeq) return;
      if (!more) this.results = [];
      this.hasMore = false;
      this.searchError = message(error);
    } finally {
      if (seq === this.searchSeq) this.searching = false;
    }
  }

  async save(uuid: string) {
    try {
      const station = await call<Station>("radio_save", { uuid });
      if (!this.stations.some((s) => s.id === station.id)) {
        this.stations = [...this.stations, station];
      }
    } catch (error) {
      this.notice = message(error);
    }
  }

  /** Save a typed address. Throws so the form can keep what was typed. */
  async addUrl(name: string, url: string) {
    const station = await call<Station>("radio_add_url", { name, url });
    this.stations = [...this.stations, station];
  }

  async rename(id: number, name: string) {
    const trimmed = name.trim();
    const station = this.stations.find((s) => s.id === id);
    if (!station || !trimmed || trimmed === station.name) return;
    try {
      await call("radio_rename", { id, name: trimmed });
      this.stations = this.stations.map((s) => (s.id === id ? { ...s, name: trimmed } : s));
    } catch (error) {
      this.notice = message(error);
    }
  }

  async remove(id: number) {
    try {
      await call("radio_remove", { id });
      this.stations = this.stations.filter((s) => s.id !== id);
    } catch (error) {
      this.notice = message(error);
    }
  }
}

export const radio = new RadioStore();
