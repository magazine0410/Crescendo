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

/** Playlist under a drag position, read off the `data-drop-playlist` markers
    the sidebar rows and the open playlist view carry. Drag positions are
    physical pixels; elementFromPoint wants CSS pixels. */
function playlistAt(position: { x: number; y: number }): number | null {
  const dpr = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(position.x / dpr, position.y / dpr);
  const id = el?.closest<HTMLElement>("[data-drop-playlist]")?.dataset.dropPlaylist;
  return id ? Number(id) : null;
}

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

  // File-manager drops: .m3u/.m3u8 import as new playlists wherever they
  // land; anything else (files or whole folders) is added to the playlist
  // under the pointer.
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  // "over" carries no paths, so remember from "enter" whether this drag holds
  // anything a playlist can take — an m3u-only drag makes its own playlist and
  // must not highlight one.
  let dragAddsTracks = false;
  await getCurrentWebview().onDragDropEvent(async (event) => {
    const payload = event.payload;
    if (payload.type === "leave") {
      playlists.dropTargetId = null;
      return;
    }
    if (payload.type === "enter") {
      dragAddsTracks = payload.paths.some((p) => !/\.m3u8?$/i.test(p));
    }
    if (payload.type === "enter" || payload.type === "over") {
      playlists.dropTargetId = dragAddsTracks ? playlistAt(payload.position) : null;
      return;
    }
    if (payload.type !== "drop") return;

    // The pointer has already left by the time "drop" arrives on some
    // compositors, so keep the last hovered target as the fallback.
    const target = playlistAt(payload.position) ?? playlists.dropTargetId;
    playlists.dropTargetId = null;

    const m3us = payload.paths.filter((p) => /\.m3u8?$/i.test(p));
    const rest = payload.paths.filter((p) => !/\.m3u8?$/i.test(p));
    const { ui } = await import("./stores/ui.svelte");

    if (m3us.length > 0) {
      ui.activeTab = "playlists";
      for (const path of m3us) {
        try {
          await playlists.importM3u(path);
        } catch (e) {
          console.error(`m3u import failed for ${path}:`, e);
        }
      }
    }
    if (rest.length === 0) return;

    if (target == null) {
      // Show where the drop should have gone instead of failing silently.
      ui.activeTab = "playlists";
      playlists.dropReport =
        playlists.list.length === 0
          ? "Create a playlist first, then drop the files onto it"
          : "Drop files onto a playlist to add them";
      return;
    }
    try {
      await playlists.addPaths(target, rest);
    } catch (e) {
      console.error("adding dropped files failed:", e);
    }
  });

  // Adopt current playback state (e.g. after a UI reload).
  const s = await call<PlayerState>("player_get_state");
  player.applyState(s);
}
