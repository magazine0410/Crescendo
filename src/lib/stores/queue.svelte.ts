import { call, type Track } from "../api";

class QueueStore {
  tracks = $state<Track[]>([]);

  async init() {
    await this.refresh();
  }

  async refresh() {
    this.tracks = await call<Track[]>("queue_list");
  }

  async add(trackIds: number[]) {
    await call("queue_add", { trackIds });
    await this.refresh();
  }

  async removeAt(positions: number[]) {
    await call("queue_remove", { positions });
    await this.refresh();
  }

  async clear() {
    await call("queue_clear");
    await this.refresh();
  }
}

export const queue = new QueueStore();
