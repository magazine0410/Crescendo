/**
 * Tauri event -> store bridge. Call `initEvents()` once from the main page.
 */
import { listen } from "@tauri-apps/api/event";
import { isTauri, call, type PlayerState, type ScanProgress } from "./api";
import { player } from "./stores/player.svelte";
import { library } from "./stores/library.svelte";
import { playlists } from "./stores/playlists.svelte";
import { queue } from "./stores/queue.svelte";

let inited = false;

export async function initEvents() {
  if (inited || !isTauri) return;
  inited = true;

  await listen<PlayerState>("player:state", (e) => player.applyState(e.payload));
  await listen<{ ms: number; duration_ms: number }>("player:position", (e) => {
    player.onPositionEvent(e.payload.ms, e.payload.duration_ms);
  });
  await listen<string>("player:error", (e) => {
    console.error("playback error:", e.payload);
  });
  await listen<ScanProgress>("scan:progress", (e) => library.onScanProgress(e.payload));
  await listen<ScanProgress>("scan:done", (e) => {
    library.onScanProgress(e.payload);
    library.refresh();
  });
  await listen<string>("scan:error", (e) => {
    library.scanError = e.payload;
    library.scanning = false;
  });
  await listen<{ id: number; plays: number }>("play:recorded", (e) => {
    library.bumpPlayCount(e.payload.id, e.payload.plays);
    playlists.bumpPlayCount(e.payload.id, e.payload.plays);
  });
  await listen("library:changed", () => library.refresh());
  await listen("queue:changed", () => queue.refresh());
  await listen<number>("playlist:changed", () => playlists.refresh());

  // Dropping .m3u/.m3u8 files anywhere on the window imports them.
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  await getCurrentWebview().onDragDropEvent(async (event) => {
    if (event.payload.type !== "drop") return;
    const m3us = event.payload.paths.filter((p) => /\.m3u8?$/i.test(p));
    if (m3us.length === 0) return;
    const { ui } = await import("./stores/ui.svelte");
    ui.activeTab = "playlists";
    for (const path of m3us) {
      try {
        await playlists.importM3u(path);
      } catch (e) {
        console.error(`m3u import failed for ${path}:`, e);
      }
    }
  });

  // Adopt current playback state (e.g. after a UI reload).
  const s = await call<PlayerState>("player_get_state");
  player.applyState(s);
}
