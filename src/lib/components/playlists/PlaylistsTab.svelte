<script lang="ts">
  import { ui } from "$lib/stores/ui.svelte";
  import PlaylistSidebar from "./PlaylistSidebar.svelte";
  import PlaylistView from "./PlaylistView.svelte";
  import QueuePanel from "./QueuePanel.svelte";
  import CollapsiblePanel from "../common/CollapsiblePanel.svelte";
</script>

<div class="tab" style:--bottombar-h={`${ui.bottombarH}px`}>
  <CollapsiblePanel
    side="left"
    size={ui.playlistSidebarW}
    min={160}
    max={420}
    persistKey="playlistSidebarW"
    collapsePersistKey="playlistSidebarCollapsed"
    collapsed={ui.playlistSidebarCollapsed}
    panelId="playlist-sidebar-panel"
    label="Playlists"
    transitionsReady={ui.layoutReady}
    onresize={(px) => (ui.playlistSidebarW = px)}
    oncollapsedchange={(collapsed) => (ui.playlistSidebarCollapsed = collapsed)}
  >
    <PlaylistSidebar />
  </CollapsiblePanel>
  <PlaylistView />
  <CollapsiblePanel
    side="right"
    size={ui.queuePanelW}
    min={180}
    max={440}
    persistKey="queuePanelW"
    collapsePersistKey="queuePanelCollapsed"
    collapsed={ui.queuePanelCollapsed}
    panelId="playlist-queue-panel"
    label="Queued files"
    transitionsReady={ui.layoutReady}
    onresize={(px) => (ui.queuePanelW = px)}
    oncollapsedchange={(collapsed) => (ui.queuePanelCollapsed = collapsed)}
  >
    <QueuePanel />
  </CollapsiblePanel>
</div>

<style>
  .tab {
    height: 100%;
    display: flex;
    min-height: 0;
  }
</style>
