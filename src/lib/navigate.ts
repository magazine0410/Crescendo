/**
 * Cross-tab navigation: showing a track's album, and revealing the song that
 * plays right now in whichever list view can show it.
 *
 * Nothing here scrolls anything itself. The Library and Playlists tabs are
 * unmounted while another tab is on screen, so a request is left in their
 * stores as a one-shot flag and each view acts on it as soon as it has rows.
 * That is what lets a jump asked for from the Now playing or Visualizer tab
 * land without taking the user off that tab.
 */
import { library } from "./stores/library.svelte";
import { player } from "./stores/player.svelte";
import { playlists } from "./stores/playlists.svelte";
import { prefs } from "./stores/prefs.svelte";
import { ui } from "./stores/ui.svelte";

/** Open an album in the Library tab, from anywhere in the app. */
export function viewAlbum(albumId: number) {
  ui.albumReturnTab = ui.activeTab;
  library.pendingAlbumId = albumId;
  ui.activeTab = "library";
}

/**
 * Ask both list views to scroll to the song that plays now.
 *
 * `switchTab` (the explicit menu command) also brings a view forward, but only
 * from a tab that has a list of its own. The library holds every track and can
 * always show it; the playlist view only can while the open playlists hold it.
 */
export function jumpToNowPlaying(opts: { switchTab?: boolean } = {}) {
  const track = player.track;
  if (!track) return;
  playlists.pendingJump = true;
  if (!opts.switchTab) {
    library.pendingJump = "auto";
    return;
  }
  const inOpenPlaylist = playlists.tracks.some((t) => t.id === track.id);
  if (ui.activeTab === "playlists" && !inOpenPlaylist) ui.activeTab = "library";
  // Opening an album is a bigger move than scrolling a list (see
  // LibraryStore.pendingJump), so the Album-wall view only makes it when the
  // library is where this command actually lands.
  library.pendingJump =
    ui.activeTab === "library" || !inOpenPlaylist ? "explicit" : "auto";
}

/** Follow a new song: only while shuffling, only with the preference on, and
    never by changing tab. Called when the player reports a different track. */
export function autoJumpOnTrackChange() {
  if (player.shuffle && prefs.values.auto_jump_now_playing) jumpToNowPlaying();
}
