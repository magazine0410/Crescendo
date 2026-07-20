<script lang="ts">
  import { ui } from "$lib/stores/ui.svelte";
  import PlaylistSidebar from "./PlaylistSidebar.svelte";
  import PlaylistView from "./PlaylistView.svelte";
  import QueuePanel from "./QueuePanel.svelte";
  import Splitter from "../common/Splitter.svelte";
</script>

<div class="tab" style:--bottombar-h={`${ui.bottombarH}px`}>
  <div class="left" style:width={`${ui.playlistSidebarW}px`}>
    <PlaylistSidebar />
  </div>
  <Splitter
    size={ui.playlistSidebarW}
    min={160}
    max={420}
    persistKey="playlistSidebarW"
    onresize={(px) => (ui.playlistSidebarW = px)}
  />
  <PlaylistView />
  <Splitter
    size={ui.queuePanelW}
    min={180}
    max={440}
    invert
    persistKey="queuePanelW"
    onresize={(px) => (ui.queuePanelW = px)}
  />
  <div class="right" style:width={`${ui.queuePanelW}px`}>
    <QueuePanel />
  </div>
</div>

<style>
  .tab {
    height: 100%;
    display: flex;
    min-height: 0;
  }
  .left,
  .right {
    flex: none;
    min-height: 0;
  }
</style>
