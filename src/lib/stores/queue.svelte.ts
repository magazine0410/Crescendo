import { call, type PlayContext, type Track } from "../api";

class QueueStore {
  tracks = $state<Track[]>([]);
  /** Bumped per refresh; a slower, older answer must not overwrite a newer one. */
  private reqId = 0;

  async init() {
    await this.refresh();
  }

  async refresh() {
    const reqId = ++this.reqId;
    const tracks = await call<Track[]>("queue_list");
    if (reqId === this.reqId) this.tracks = tracks;
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

  /** Remove rows as shown: each position goes with its track id, so the
      player can tell when the head of the queue was taken since this view
      was drawn and remove the right entry. */
  async removeAt(positions: number[]) {
    const items = positions.flatMap((p) => {
      const t = this.tracks[p];
      return t ? [[p, t.id] as [number, number]] : [];
    });
    await call("queue_remove", { items });
    await this.refresh();
  }

  async clear() {
    await call("queue_clear");
    await this.refresh();
  }
}

export const queue = new QueueStore();
