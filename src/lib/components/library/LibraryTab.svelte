<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import ModeSelector from "./ModeSelector.svelte";
  import ValueFilter from "./ValueFilter.svelte";
  import ValueList from "./ValueList.svelte";
  import AlbumWall from "./AlbumWall.svelte";
  import AlbumDetail from "./AlbumDetail.svelte";
  import TrackList from "./TrackList.svelte";
  import CollapsiblePanel from "../common/CollapsiblePanel.svelte";
  import ScanProgress from "../common/ScanProgress.svelte";
  import QueuePanel from "../playlists/QueuePanel.svelte";

  $effect(() => {
    const q = ui.searchQuery;
    if (library.interactionBlocked) return;
    const t = setTimeout(() => {
      // Clearing an active search while a track plays in the Track-list view:
      // scroll to the now-playing song's spot in the full library instead of
      // snapping to the top. TrackList consumes this after it reloads.
      const clearing = !q.trim() && library.query != null;
      if (
        clearing &&
        prefs.values.library_view === "track_list" &&
        !library.selectedAlbum &&
        player.track
      ) {
        library.scrollToTrackId = player.track.id;
      }
      library.setQuery(q);
    }, 200);
    return () => clearTimeout(t);
  });

  // Switching the library view drops any drilled-in album so the new view shows.
  $effect(() => {
    prefs.values.library_view;
    library.selectAlbum(null);
  });

  // "View album" asked for from outside this tab (the transport bar, the
  // playlist view). It is held in the store because this tab is unmounted
  // while another one is on screen, so the album opens on mount as well.
  $effect(() => {
    const id = library.pendingAlbumId;
    if (id == null) return;
    library.pendingAlbumId = null;
    void library.openAlbumById(id);
  });

  // "Jump to now playing song", the part TrackList cannot decide. The Track
  // list scrolls itself and only needs a drilled-in album taken off screen;
  // the Album wall has no track rows at all, so the song's album is opened
  // instead. Both change what the tab shows, so only the menu command may make
  // them — a shuffle follow-along scrolls lists and leaves the rest alone.
  $effect(() => {
    const kind = library.pendingJump;
    if (!kind) return;
    const trackList = prefs.values.library_view === "track_list";
    if (trackList && !library.selectedAlbum) return; // TrackList consumes it
    if (kind === "explicit" && trackList) {
      void library.selectAlbum(null); // TrackList mounts and takes it from here
      return;
    }
    library.pendingJump = null;
    const albumId = player.track?.album_id;
    if (kind === "explicit" && albumId != null) {
      // This album is reached from inside the Library tab, so "Back" belongs to
      // the Album wall. Drop any return tab left by an earlier cross-tab visit
      // that the user left without pressing Back.
      ui.albumReturnTab = null;
      void library.openAlbumById(albumId);
    }
  });
</script>

<div class="tab">
  <ScanProgress />
  {#if library.interactionBlocked || library.notice}
    <button
      class="projection-status"
      class:error={!!library.notice}
      disabled={library.interactionBlocked}
      title={library.notice ? "Dismiss" : undefined}
      onclick={() => (library.notice = null)}
    >
      {library.notice ?? "Updating library view…"}
    </button>
  {/if}
  <div class="cols">
    <CollapsiblePanel
      side="left"
      size={ui.libraryValueListW}
      min={150}
      max={480}
      persistKey="libraryValueListW"
      collapsePersistKey="librarySidebarCollapsed"
      collapsed={ui.librarySidebarCollapsed}
      panelId="library-filter-panel"
      label="Library filters"
      transitionsReady={ui.layoutReady}
      onresize={(px) => (ui.libraryValueListW = px)}
      oncollapsedchange={(collapsed) => (ui.librarySidebarCollapsed = collapsed)}
    >
      <div class="left">
        <ModeSelector />
        <ValueFilter />
        <ValueList />
      </div>
    </CollapsiblePanel>
    {#if library.selectedAlbum}
      <AlbumDetail />
    {:else if prefs.values.library_view === "track_list"}
      <TrackList />
    {:else}
      <AlbumWall />
    {/if}
    <CollapsiblePanel
      side="right"
      size={ui.queuePanelW}
      min={180}
      max={440}
      persistKey="queuePanelW"
      collapsePersistKey="queuePanelCollapsed"
      collapsed={ui.queuePanelCollapsed}
      panelId="library-queue-panel"
      label="Queued files"
      transitionsReady={ui.layoutReady}
      onresize={(px) => (ui.queuePanelW = px)}
      oncollapsedchange={(collapsed) => (ui.queuePanelCollapsed = collapsed)}
    >
      <div class="right"><QueuePanel /></div>
    </CollapsiblePanel>
  </div>
</div>

<style>
  .tab {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .cols {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .projection-status {
    flex: none;
    width: 100%;
    min-height: 24px;
    padding: 3px 12px;
    text-align: left;
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-panel));
    color: var(--txt);
    border-bottom: 1px solid var(--hairline);
  }
  .projection-status.error {
    color: var(--warning, #ffba66);
  }
  .left {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: none;
    background: var(--bg-panel);
  }
  .right {
    width: 100%;
    height: 100%;
    min-height: 0;
  }
</style>
