import { call, type PlayContext, type Track } from "../api";

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

  /** Append everything a context names. The backend resolves it under the
      projection guard with the same SQL the view used, so the queue gets
      exactly the tracks the user is looking at — and no track list crosses
      IPC on the way. */
  async addContext(ctx: PlayContext) {
    await call<number>("queue_add_context", { ctx });
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
