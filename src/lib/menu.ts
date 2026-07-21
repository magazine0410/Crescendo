import { playlists } from "./stores/playlists.svelte";
import type { MenuItem } from "./components/common/ContextMenu.svelte";

/**
 * Build the "Add to playlist" context-menu entry: a single item that reveals
 * the playlist list in a submenu (so a large library never floods the menu).
 * Falls back to a disabled item when there are no playlists yet. `add` is
 * called with the chosen playlist id.
 */
export function addToPlaylistItem(
  add: (playlistId: number) => void | Promise<void>,
): MenuItem {
  const list = playlists.list;
  if (list.length === 0) {
    return { label: "Add to playlist", disabled: true };
  }
  return {
    label: "Add to playlist",
    submenu: list.map((p) => ({
      label: p.name,
      action: () => add(p.id),
    })),
  };
}
