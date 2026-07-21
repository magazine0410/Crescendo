<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import type { BrowseMode } from "$lib/api";

  const modes: { id: BrowseMode; label: string }[] = [
    { id: "genre", label: "GENRE" },
    { id: "artist", label: "ARTIST" },
    { id: "album", label: "ALBUM" },
    { id: "grouping", label: "GROUPING" },
  ];

  /** Right-click the GROUPING tab: toggle collapsing multi-theme grouping tags
      under their primary theme. Other tabs have no menu. */
  function tabMenu(e: MouseEvent, id: BrowseMode) {
    if (id !== "grouping") return;
    openContextMenu(e, [
      {
        label: "Show multi-tagged titles separately",
        checked: prefs.values.grouping_split_multi,
        action: async () => {
          await prefs.set("grouping_split_multi", !prefs.values.grouping_split_multi);
          // Umbrella values change meaning between modes; drop any selection
          // (this re-runs the track view) and rebuild the value list.
          library.selectValue(null);
          await library.loadValues();
        },
      },
    ]);
  }
</script>

<div class="modes">
  {#each modes as m (m.id)}
    <button
      class="mode"
      class:active={library.mode === m.id}
      onclick={() => library.setMode(m.id)}
      oncontextmenu={(e) => tabMenu(e, m.id)}
    >
      {m.label}
    </button>
  {/each}
</div>

<style>
  .modes {
    display: flex;
    align-items: center;
    height: 30px;
    border-bottom: 1px solid var(--hairline);
    flex: none;
    padding: 0 4px;
  }
  .mode {
    padding: 0 9px;
    height: 100%;
    font-size: 10px;
    letter-spacing: 0.06em;
    color: var(--txt-faded);
  }
  .mode:hover {
    color: var(--txt);
  }
  .mode.active {
    color: var(--txt-full);
    box-shadow: inset 0 -2px 0 var(--txt-full);
  }
</style>
