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
  import VisualizerTab from "$lib/components/visualizer/VisualizerTab.svelte";
  import ContextMenu from "$lib/components/common/ContextMenu.svelte";
  import SettingsDialog from "$lib/components/settings/SettingsDialog.svelte";
  import SetFilterDialog from "$lib/components/library/SetFilterDialog.svelte";
  import SetCollectionDialog from "$lib/components/playlists/SetCollectionDialog.svelte";
  import SearchHelp from "$lib/components/SearchHelp.svelte";

  let settingsOpen = $state(false);
  let visualizerMounted = $state(false);

  $effect(() => {
    if (ui.activeTab === "visualizer") visualizerMounted = true;
  });

  onMount(async () => {
    await prefs.load();
    ui.activeTab = prefs.values.default_tab;
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

  function isInteractiveTarget(e: KeyboardEvent): boolean {
    const el = e.target instanceof Element ? e.target : null;
    return !!el?.closest('button, select, summary, a[href], [role="button"], [role="link"]');
  }

  function onKeydown(e: KeyboardEvent) {
    if (settingsOpen || document.querySelector('[aria-modal="true"]')) return;
    if (e.key === "/" && !isTypingTarget(e)) {
      e.preventDefault();
      document.querySelector<HTMLInputElement>(".search input")?.focus();
      return;
    }
    if (isTypingTarget(e)) return;
    // Let focused controls keep their native Space activation instead of
    // turning the keystroke into a global play/pause command.
    if (e.key === " " && isInteractiveTarget(e)) return;
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
    {:else if ui.activeTab === "nowplaying"}
      <NowPlayingTab />
    {/if}
    {#if visualizerMounted}
      <div class="visualizer-host" class:active={ui.activeTab === "visualizer"}>
        <VisualizerTab active={ui.activeTab === "visualizer"} />
      </div>
    {/if}
  </main>
  <BottomBar />
  <ContextMenu />
  <SettingsDialog open={settingsOpen} onclose={() => (settingsOpen = false)} />
  <SetFilterDialog />
  <SetCollectionDialog />
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
    position: relative;
    flex: 1;
    min-height: 0;
    /* content scrolls beneath the translucent bottom bar; panels that must
       not be covered add padding-bottom: var(--bottombar-h) themselves */
  }
  .visualizer-host {
    /* WebKitGTK can keep a visibility-hidden WebGL canvas composited and
       clickable. display:none detaches that surface without unmounting it. */
    display: none;
    position: absolute;
    inset: 0;
  }
  .visualizer-host.active {
    display: block;
  }
</style>
