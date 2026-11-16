<script lang="ts">
  import { ui, type Tab } from "$lib/stores/ui.svelte";
  import SearchBar from "./SearchBar.svelte";
  import Icon from "./common/Icon.svelte";

  type TabDef = { id: Tab; label: string; icon: string };
  /** Two groups with a separator between them: what plays from this computer,
      then what plays from the internet. */
  const localTabs: TabDef[] = [
    { id: "library", label: "Library", icon: "library_icon" },
    { id: "playlists", label: "Playlists", icon: "playlist_icon" },
    { id: "nowplaying", label: "Now playing", icon: "nowplaying_icon" },
    { id: "visualizer", label: "Visualizer", icon: "visualizer_icon" },
  ];
  const onlineTabs: TabDef[] = [{ id: "radio", label: "Radio", icon: "radio" }];

  interface Props {
    onOpenSettings?: () => void;
  }
  let { onOpenSettings }: Props = $props();
</script>

<header class="topbar">
  {#snippet tabButton(tab: TabDef)}
    <button
      class="tab"
      class:active={ui.activeTab === tab.id}
      onclick={() => (ui.activeTab = tab.id)}
    >
      <Icon name={tab.icon} size={15} />
      <span>{tab.label}</span>
    </button>
  {/snippet}
  <nav class="tabs">
    {#each localTabs as tab (tab.id)}
      {@render tabButton(tab)}
    {/each}
    <span class="group-separator" role="separator" aria-orientation="vertical"></span>
    {#each onlineTabs as tab (tab.id)}
      {@render tabButton(tab)}
    {/each}
  </nav>
  <div class="right">
    <SearchBar />
    <button class="settings" title="Settings" onclick={() => onOpenSettings?.()}>
      <Icon name="global_settings" size={16} />
    </button>
  </div>
</header>

<style>
  .topbar {
    height: var(--topbar-h);
    display: flex;
    align-items: stretch;
    justify-content: space-between;
    background: var(--bg-main);
    border-bottom: 1px solid var(--hairline);
    padding: 0 10px 0 4px;
    flex: none;
  }
  .tabs {
    display: flex;
    align-items: stretch;
  }
  .group-separator {
    align-self: center;
    width: 1px;
    height: 16px;
    margin: 0 6px;
    background: var(--txt-superfaded);
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 0 14px;
    color: var(--txt-faded);
    position: relative;
  }
  /* Icons inherit currentColor, so the tab's per-state color drives them too. */
  .tab:hover {
    color: var(--txt);
  }
  .tab.active {
    color: var(--txt-full);
  }
  .tab.active::after {
    content: "";
    position: absolute;
    left: 10px;
    right: 10px;
    bottom: 0;
    height: 2px;
    background: var(--txt-full);
  }
  .right {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .settings {
    display: flex;
    align-items: center;
    color: var(--txt);
    opacity: 0.6;
  }
  .settings:hover {
    opacity: 1;
  }
</style>
