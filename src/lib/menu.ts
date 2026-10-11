import { playlists } from "./stores/playlists.svelte";
import { call, isTauri, type PlayContext, type Track } from "./api";
import { library } from "./stores/library.svelte";
import type { MenuItem } from "./components/common/ContextMenu.svelte";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

/** Launch the Forte GUI with the complete current selection. */
export function tagEditItem(tracks: Track[]): MenuItem {
  const ids = tracks.filter(track => !track.missing).map(track => track.id);
  return {
    label: "Tag edit",
    disabled: ids.length === 0,
    action: async () => {
      try { await call("tag_edit_open", { ids }); }
      catch (error) { library.notice = String(error); }
    },
  };
}

/** Reveal a library file in the system file manager. Browser preview is a no-op. */
export async function openInFileManager(path: string): Promise<void> {
  if (!isTauri) return;
  try {
    await revealItemInDir(path);
  } catch (e) {
    console.error("Could not open file manager:", e);
  }
}

/** Shared context-menu entry for a track. Missing playlist files cannot be revealed. */
export function openTrackInFileManagerItem(track: Track): MenuItem {
  return {
    label: "Open in file manager",
    disabled: track.missing,
    action: track.missing ? undefined : () => openInFileManager(track.path),
  };
}

/**
 * Build the "Add to playlist" context-menu entry: a single item that reveals
 * the playlist list in a submenu (so a large library never floods the menu).
 * Falls back to a disabled item when there are no playlists yet. `add` is
 * called with the chosen playlist id.
 *
 * `memberOf` (used for single-track menus) marks playlists the track already
 * belongs to — those entries are disabled and annotated, so you can't add a
 * duplicate. For multi-track menus leave it unset; the backend dedupes on add.
 */
export function addToPlaylistItem(
  add: (playlistId: number) => void | Promise<void>,
  opts: { memberOf?: Set<number> } = {},
): MenuItem {
  const list = playlists.list;
  if (list.length === 0) {
    return { label: "Add to playlist", disabled: true };
  }
  return {
    label: "Add to playlist",
    submenu: list.map((p) =>
      opts.memberOf?.has(p.id)
        ? { label: `${p.name} (already in the playlist)`, disabled: true }
        : { label: p.name, action: () => add(p.id) },
    ),
  };
}

/**
 * "Add to playlist" for everything a context names — an album, or a
 * filter-panel value with the current search applied. The backend resolves
 * the context, so a view that changed underneath (stale projection) is
 * recovered the same way it is for playback and the queue.
 */
export function addContextToPlaylistItem(ctx: PlayContext): MenuItem {
  return addToPlaylistItem(async (pid) => {
    try {
      await playlists.addContext(pid, ctx);
    } catch (reason) {
      if (await library.recoverStaleProjection(reason)) {
        library.notice = "Library view changed; try again";
        return;
      }
      throw reason;
    }
  });
}
