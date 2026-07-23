<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { openSetFilter } from "./SetFilterDialog.svelte";
  import type { BrowseMode } from "$lib/api";

  const modes: { id: BrowseMode; label: string }[] = [
    { id: "genre", label: "GENRE" },
    { id: "artist", label: "ARTIST" },
    { id: "album", label: "ALBUM" },
    { id: "grouping", label: "GROUPING" },
    { id: "custom", label: "CUSTOM" },
  ];

  /** The custom tab shows its resolved tag name (e.g. YEAR); until one is set it
      reads CUSTOM. Every other tab uses its fixed label. */
  function labelFor(m: { id: BrowseMode; label: string }): string {
    if (m.id !== "custom") return m.label;
    return (library.customTagName ?? "custom").toUpperCase();
  }

  // --- Overflow handling --------------------------------------------------
  // When the panel is too narrow for all five tabs, hide the right-most ones
  // (they mustn't spill into the middle panel) and expose them via a chevron.
  const CHEVRON_W = 22; // px reserved for the "›" button when overflowing

  let stripEl = $state<HTMLDivElement>();
  let measureEl = $state<HTMLDivElement>();
  let availW = $state(0);
  let widths = $state<number[]>([]);

  function measure() {
    if (measureEl) {
      widths = [...measureEl.children].map((c) => (c as HTMLElement).getBoundingClientRect().width);
    }
    if (stripEl) availW = stripEl.clientWidth - 8; // clientWidth includes the 0 4px padding
  }

  // Re-measure when a label changes (the CUSTOM tab relabels to its tag).
  $effect(() => {
    modes.map(labelFor);
    measure();
  });

  // Re-measure when the panel is resized.
  $effect(() => {
    if (!stripEl) return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(stripEl);
    return () => ro.disconnect();
  });

  /** How many tabs fit left-to-right, leaving room for the chevron when any
      overflow. Falls back to all tabs until the first measurement lands. */
  const visibleCount = $derived.by(() => {
    const n = modes.length;
    if (widths.length !== n || availW <= 0) return n;
    if (widths.reduce((a, b) => a + b, 0) <= availW) return n; // all fit, no chevron
    let sum = 0;
    for (let k = 0; k < n; k++) {
      if (sum + widths[k] + CHEVRON_W > availW) return Math.max(1, k);
      sum += widths[k];
    }
    return n;
  });

  const visibleModes = $derived(modes.slice(0, visibleCount));
  const hiddenModes = $derived(modes.slice(visibleCount));
  const hiddenActive = $derived(hiddenModes.some((m) => m.id === library.mode));

  /** Right-click menus: GROUPING toggles multi-theme collapse; CUSTOM sets/clears
      the browsed tag. Other tabs have no menu. */
  function tabMenu(e: MouseEvent, id: BrowseMode) {
    if (id === "grouping") {
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
    } else if (id === "custom") {
      openContextMenu(e, [
        { label: "Set filter…", action: () => openSetFilter() },
        {
          label: "Clear filter",
          disabled: library.customTag.trim() === "",
          action: () => library.setCustomTag(""),
        },
      ]);
    }
  }

  /** The chevron's menu: the overflowed tabs, so a hidden one (e.g. CUSTOM)
      stays reachable. The active tab is check-marked. */
  function moreMenu(e: MouseEvent) {
    const items: MenuItem[] = hiddenModes.map((m) => ({
      label: labelFor(m),
      checked: library.mode === m.id,
      action: () => library.setMode(m.id),
    }));
    openContextMenu(e, items);
  }
</script>

<div class="modes" bind:this={stripEl}>
  <!-- Off-screen copy of every tab at natural width, used only to measure. -->
  <div class="measure" bind:this={measureEl} aria-hidden="true">
    {#each modes as m (m.id)}
      <span class="mode">{labelFor(m)}</span>
    {/each}
  </div>

  {#each visibleModes as m (m.id)}
    <button
      class="mode"
      class:active={library.mode === m.id}
      onclick={() => library.setMode(m.id)}
      oncontextmenu={(e) => tabMenu(e, m.id)}
    >
      {labelFor(m)}
    </button>
  {/each}

  {#if hiddenModes.length}
    <button
      class="more"
      class:active={hiddenActive}
      title="More filters"
      aria-label="More filters"
      onclick={moreMenu}
      oncontextmenu={moreMenu}
    >
      ›
    </button>
  {/if}
</div>

<style>
  .modes {
    position: relative;
    display: flex;
    align-items: center;
    height: var(--header-h);
    border-bottom: 1px solid var(--hairline);
    flex: none;
    padding: 0 4px;
    overflow: hidden;
  }
  .mode {
    padding: 0 9px;
    height: 100%;
    flex: none;
    white-space: nowrap;
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
  /* Hidden measuring row: laid out but never shown and never interactive. */
  .measure {
    position: absolute;
    top: 0;
    left: 0;
    display: flex;
    visibility: hidden;
    pointer-events: none;
  }
  .more {
    flex: none;
    margin-left: auto; /* pin to the right edge */
    width: 18px;
    height: 100%;
    font-size: 15px;
    line-height: 1;
    color: var(--txt-faded);
  }
  .more:hover,
  .more.active {
    color: var(--txt-full);
  }
</style>
