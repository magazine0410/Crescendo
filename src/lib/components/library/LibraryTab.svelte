<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import ModeSelector from "./ModeSelector.svelte";
  import ValueList from "./ValueList.svelte";
  import AlbumWall from "./AlbumWall.svelte";
  import AlbumDetail from "./AlbumDetail.svelte";
  import TrackList from "./TrackList.svelte";
  import Splitter from "../common/Splitter.svelte";
  import ScanProgress from "../common/ScanProgress.svelte";
  import QueuePanel from "../playlists/QueuePanel.svelte";

  $effect(() => {
    const q = ui.searchQuery;
    const t = setTimeout(() => library.setQuery(q), 200);
    return () => clearTimeout(t);
  });

  // Switching the library view drops any drilled-in album so the new view shows.
  $effect(() => {
    prefs.values.library_view;
    library.selectAlbum(null);
  });
</script>

<div class="tab">
  <ScanProgress />
  <div class="cols">
    <div class="left" style:width={`${ui.libraryValueListW}px`}>
      <ModeSelector />
      <ValueList />
    </div>
    <Splitter
      size={ui.libraryValueListW}
      min={150}
      max={480}
      persistKey="libraryValueListW"
      onresize={(px) => (ui.libraryValueListW = px)}
    />
    {#if library.selectedAlbum}
      <AlbumDetail />
    {:else if prefs.values.library_view === "track_list"}
      <TrackList />
    {:else}
      <AlbumWall />
    {/if}
    <Splitter
      size={ui.queuePanelW}
      min={180}
      max={440}
      invert
      persistKey="queuePanelW"
      onresize={(px) => (ui.queuePanelW = px)}
    />
    <div class="right" style:width={`${ui.queuePanelW}px`} style:--bottombar-h={`${ui.bottombarH}px`}>
      <QueuePanel />
    </div>
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
  .left {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: none;
    background: var(--bg-panel);
  }
  .right {
    flex: none;
    min-height: 0;
  }
</style>
