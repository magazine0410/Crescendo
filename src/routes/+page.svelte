<script lang="ts">
  import { onMount } from "svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { player } from "$lib/stores/player.svelte";
  import TopBar from "$lib/components/TopBar.svelte";
  import BottomBar from "$lib/components/transport/BottomBar.svelte";
  import LibraryTab from "$lib/components/library/LibraryTab.svelte";
  import PlaylistsTab from "$lib/components/playlists/PlaylistsTab.svelte";
  import NowPlayingTab from "$lib/components/nowplaying/NowPlayingTab.svelte";
  import ContextMenu from "$lib/components/common/ContextMenu.svelte";
  import SettingsDialog from "$lib/components/settings/SettingsDialog.svelte";
  import SetFilterDialog from "$lib/components/library/SetFilterDialog.svelte";
  import SearchHelp from "$lib/components/SearchHelp.svelte";

  let settingsOpen = $state(false);

  onMount(async () => {
    await prefs.load();
    const layout = await prefs.getLayout();
    if (layout.bottombarH) ui.bottombarH = layout.bottombarH;
    if (layout.playlistSidebarW) ui.playlistSidebarW = layout.playlistSidebarW;
    if (layout.queuePanelW) ui.queuePanelW = layout.queuePanelW;
    if (layout.libraryValueListW) ui.libraryValueListW = layout.libraryValueListW;
    const { initEvents } = await import("$lib/events");
    await initEvents();
    const { library } = await import("$lib/stores/library.svelte");
    const { playlists } = await import("$lib/stores/playlists.svelte");
    const { queue } = await import("$lib/stores/queue.svelte");
    await Promise.all([library.init(), playlists.init(), queue.init()]);
    // First launch with an empty library: open settings so scanning can start.
    if ((library.stats?.tracks ?? 0) === 0) settingsOpen = true;
  });

  function isTypingTarget(e: KeyboardEvent): boolean {
    const el = e.target as HTMLElement;
    return el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable;
  }

  function onKeydown(e: KeyboardEvent) {
    if (settingsOpen) return;
    if (e.key === "/" && !isTypingTarget(e)) {
      e.preventDefault();
      document.querySelector<HTMLInputElement>(".search input")?.focus();
      return;
    }
    if (isTypingTarget(e)) return;
    switch (e.key) {
      case " ":
        e.preventDefault();
        player.togglePause();
        break;
      case "ArrowLeft":
        if (player.durationMs > 0) player.seek(Math.max(0, player.positionMs - 5000));
        break;
      case "ArrowRight":
        if (player.durationMs > 0)
          player.seek(Math.min(player.durationMs, player.positionMs + 5000));
        break;
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="app">
  <TopBar onOpenSettings={() => (settingsOpen = true)} />
  <main class="content" style:--bottombar-h={`${ui.bottombarH}px`}>
    {#if ui.activeTab === "library"}
      <LibraryTab />
    {:else if ui.activeTab === "playlists"}
      <PlaylistsTab />
    {:else}
      <NowPlayingTab />
    {/if}
  </main>
  <BottomBar />
  <ContextMenu />
  <SettingsDialog open={settingsOpen} onclose={() => (settingsOpen = false)} />
  <SetFilterDialog />
  <SearchHelp open={ui.searchHelpOpen} onclose={() => (ui.searchHelpOpen = false)} />
</div>

<style>
  .app {
    position: relative;
    height: 100vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .content {
    flex: 1;
    min-height: 0;
    /* content scrolls beneath the translucent bottom bar; panels that must
       not be covered add padding-bottom: var(--bottombar-h) themselves */
  }
</style>
