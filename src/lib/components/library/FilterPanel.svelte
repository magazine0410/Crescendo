<script lang="ts">
  import { customTagLabel, library, type Facet } from "$lib/stores/library.svelte";
  import { plugins } from "$lib/stores/plugins.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { LIBRARY_PANEL_MAX_W, ui } from "$lib/stores/ui.svelte";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { openSetFilter } from "./SetFilterDialog.svelte";
  import Icon from "../common/Icon.svelte";
  import ValueFilter from "./ValueFilter.svelte";
  import ValueList from "./ValueList.svelte";
  import type { BrowseMode, PluginMenuControl } from "$lib/api";

  const labels: Record<BrowseMode, string> = {
    genre: "GENRE",
    artist: "ARTIST",
    album: "ALBUM",
    albumartist: "ALBUM ARTIST",
    custom: "CUSTOM",
  };

  /** The custom category shows its resolved tag name (e.g. YEAR); until one is
      set it reads CUSTOM. Every other category uses its fixed label. */
  function labelFor(facet: Facet): string {
    if (facet.mode !== "custom") return labels[facet.mode];
    return customTagLabel(library.customTagName ?? "custom").toUpperCase();
  }

  /** What a category has selected, shown in its header (or its tab's tooltip)
      so a filter stays visible while its list is not. */
  function selectionSummary(facet: Facet): string {
    const selected = facet.selectedValues;
    return selected.length === 1 ? facet.label(selected[0]) : `${selected.length} selected`;
  }

  const tabs = $derived(prefs.values.filter_panel_layout === "tabs");
  const filtering = $derived(library.activeFilters.length > 0);

  // The vertical list can have every category collapsed; the tabs always show
  // one. Settle that when the layout is switched to tabs.
  $effect(() => {
    if (tabs) void library.showOneFacet();
  });

  /** The category whose list the tabs layout shows. */
  const shown = $derived(library.facets.find((facet) => facet.expanded) ?? library.facet("artist"));

  // --- Tab overflow -------------------------------------------------------
  // When the panel is too narrow for all five tabs, hide the right-most ones
  // (they mustn't spill into the middle panel) and expose them via a chevron.
  const CHEVRON_W = 22; // px reserved for the "›" button when overflowing
  const CLEAR_W = 26; // px the "Clear filters" button always takes

  let stripEl = $state<HTMLDivElement>();
  let measureEl = $state<HTMLDivElement>();
  let availW = $state(0);
  let widths = $state<number[]>([]);

  function measure() {
    if (measureEl) {
      widths = [...measureEl.children].map((c) => (c as HTMLElement).getBoundingClientRect().width);
    }
    // clientWidth includes the 0 4px padding.
    if (stripEl) availW = stripEl.clientWidth - 8 - CLEAR_W;
  }

  // Re-measure when a label changes (the custom tab relabels to its tag).
  $effect(() => {
    library.facets.map(labelFor);
    measure();
  });

  // Re-measure when the panel is resized.
  $effect(() => {
    if (!stripEl) return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(stripEl);
    return () => ro.disconnect();
  });

  // With no saved width, widen the default so every tab shows beside the Clear
  // filters button. A width the user dragged to is saved and kept as it is
  // (+page.svelte marks it settled). The fitted width itself is not saved. It
  // waits for the font and the custom tab's label to be in place.
  $effect(() => {
    if (ui.libraryPanelWidthSettled || !tabs || !ui.layoutReady || !prefs.loaded || !measureEl)
      return;
    const tabsW = [...measureEl.children].reduce(
      (sum, c) => sum + (c as HTMLElement).getBoundingClientRect().width,
      0,
    );
    if (tabsW <= 0) return;
    ui.libraryPanelWidthSettled = true;
    const needed = Math.min(LIBRARY_PANEL_MAX_W, Math.ceil(tabsW) + 8 + CLEAR_W);
    if (ui.libraryValueListW < needed) ui.libraryValueListW = needed;
  });

  /** How many tabs fit left-to-right, leaving room for the chevron when any
      overflow. Falls back to all tabs until the first measurement lands. */
  const visibleCount = $derived.by(() => {
    const n = library.facets.length;
    if (widths.length !== n || availW <= 0) return n;
    if (widths.reduce((a, b) => a + b, 0) <= availW) return n; // all fit, no chevron
    let sum = 0;
    for (let k = 0; k < n; k++) {
      if (sum + widths[k] + CHEVRON_W > availW) return Math.max(1, k);
      sum += widths[k];
    }
    return n;
  });

  const visibleFacets = $derived(library.facets.slice(0, visibleCount));
  const hiddenFacets = $derived(library.facets.slice(visibleCount));

  /** Flip the boolean setting a menu control is bound to, through the guarded
      projection transition. */
  async function toggleSettingControl(control: PluginMenuControl) {
    if (control.setting_key == null) return;
    await library.changeProjection(async () => {
      const result = await plugins.setSetting(
        control.plugin_id,
        control.setting_key as string,
        !control.checked,
      );
      return result.snapshot.projection_revision;
    });
  }

  /** Dispatch one declared Wasm command. This changes no projection, so it
      runs outside the transition; a failing plugin only reports through the
      library's notice line and never disturbs the view or playback. */
  async function runCommandControl(control: PluginMenuControl) {
    if (control.command == null) return;
    try {
      await plugins.invoke(control.plugin_id, control.command, {});
    } catch (reason) {
      library.notice = `${control.label} failed: ${reason instanceof Error ? reason.message : reason}`;
    }
  }

  /** The custom category's own menu: host-rendered controls contributed for
      the active tag by the immutable loaded registry, then the filter actions. */
  function customMenuItems(): MenuItem[] {
    const controls =
      plugins.snapshot?.menu_controls.filter(
        (control) =>
          control.slot === "library.filter" && control.target_tag === library.customTagName,
      ) ?? [];
    const items: MenuItem[] = controls.map((control) => ({
      label: control.label,
      // A toggle shows its setting as a tick; a command action has no state
      // to show, so it renders as a plain item.
      checked: control.checked ?? undefined,
      disabled: library.interactionBlocked,
      action: () =>
        control.action === "run_command"
          ? runCommandControl(control)
          : toggleSettingControl(control),
    }));
    if (controls.length > 0) items.push({ separator: true, label: "" });
    items.push(
      { label: "Set filter…", action: () => openSetFilter() },
      {
        label: "Clear filter",
        disabled: library.customTag.trim() === "",
        action: () => library.setCustomTag(""),
      },
    );
    return items;
  }

  function headerMenu(e: MouseEvent, facet: Facet) {
    if (library.interactionBlocked) return;
    if (facet.mode === "custom") openContextMenu(e, customMenuItems());
  }

  /** The chevron's menu: the overflowed tabs, so a hidden one (e.g. the
      custom tab) stays reachable. The shown tab is check-marked, and a tab
      with a selection names it.

      A hidden custom tab would otherwise take its whole menu with it. Choosing
      it here does not bring the tab back into the strip, so its plugin
      controls and filter actions have no other surface at this width; they are
      appended below the tab list, under the entry they belong to. */
  function moreMenu(e: MouseEvent) {
    if (library.interactionBlocked) return;
    const items: MenuItem[] = hiddenFacets.map((facet) => ({
      label:
        facet.selectedValues.length > 0
          ? `${labelFor(facet)} (${selectionSummary(facet)})`
          : labelFor(facet),
      checked: facet.expanded,
      action: () => library.openFacet(facet),
    }));
    if (hiddenFacets.some((facet) => facet.mode === "custom")) {
      items.push({ separator: true, label: "" }, ...customMenuItems());
    }
    openContextMenu(e, items);
  }
</script>

<div class="facets" class:list={!tabs}>
  {#if tabs}
    <div class="tabs" bind:this={stripEl}>
      <!-- Off-screen copy of every tab at natural width, used only to measure. -->
      <div class="measure" bind:this={measureEl} aria-hidden="true">
        {#each library.facets as facet (facet.mode)}
          <span class="tab">{labelFor(facet)}</span>
        {/each}
      </div>

      {#each visibleFacets as facet (facet.mode)}
        <button
          class="tab"
          class:active={facet.expanded}
          class:filtering={facet.selectedValues.length > 0}
          title={facet.selectedValues.length > 0 ? selectionSummary(facet) : undefined}
          disabled={library.interactionBlocked}
          onclick={() => library.openFacet(facet)}
          oncontextmenu={(e) => headerMenu(e, facet)}
        >
          {labelFor(facet)}
        </button>
      {/each}

      {#if hiddenFacets.length}
        <button
          class="more"
          class:active={hiddenFacets.some((facet) => facet.expanded)}
          class:filtering={hiddenFacets.some((facet) => facet.selectedValues.length > 0)}
          disabled={library.interactionBlocked}
          title="More filters"
          aria-label="More filters"
          onclick={moreMenu}
          oncontextmenu={moreMenu}
        >
          ›
        </button>
      {/if}

      <button
        class="clear-icon"
        disabled={!filtering || library.interactionBlocked}
        title="Clear filters"
        aria-label="Clear filters"
        onclick={() => library.clearFilters()}
      >
        <Icon name="trash" size={13} />
      </button>
    </div>
    <!-- A new list per tab, so each starts at its top. -->
    {#key shown.mode}
      <div class="body">
        <ValueFilter facet={shown} />
        <ValueList facet={shown} />
      </div>
    {/key}
  {:else}
    <!-- Every header is always on screen. The open category's list takes the
         height the headers leave, so the headers below it sit at the bottom,
         right above the Clear filters button. -->
    {#each library.facets as facet (facet.mode)}
      <button
        class="head"
        class:expanded={facet.expanded}
        aria-expanded={facet.expanded}
        disabled={library.interactionBlocked}
        onclick={() => library.toggleFacet(facet)}
        oncontextmenu={(e) => headerMenu(e, facet)}
      >
        <span class="chevron"><Icon name="chevron-right" size={12} /></span>
        <span class="label">{labelFor(facet)}</span>
        {#if facet.selectedValues.length > 0}
          <span class="picked">{selectionSummary(facet)}</span>
        {/if}
      </button>
      {#if facet.expanded}
        <div class="body">
          <ValueFilter {facet} />
          <ValueList {facet} />
        </div>
      {/if}
    {/each}
    <button
      class="clear-all"
      disabled={!filtering || library.interactionBlocked}
      onclick={() => library.clearFilters()}
    >
      Clear filters
    </button>
  {/if}
</div>

<style>
  .facets {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    /* Stop at the top edge of the bottom bar (as the track list and the album
       wall do), so neither a list's scroll thumb nor a header goes under it. */
    padding-bottom: var(--bottombar-h);
  }

  /* --- Horizontal tabs --- */
  .tabs {
    position: relative;
    display: flex;
    align-items: center;
    height: var(--header-h);
    border-bottom: 1px solid var(--hairline);
    flex: none;
    padding: 0 4px;
    overflow: hidden;
  }
  .tab {
    position: relative;
    padding: 0 9px;
    height: 100%;
    flex: none;
    white-space: nowrap;
    font-size: 10px;
    letter-spacing: 0.06em;
    color: var(--txt-faded);
  }
  .tab:hover {
    color: var(--txt);
  }
  .tab.active {
    color: var(--txt-full);
    box-shadow: inset 0 -2px 0 var(--txt-full);
  }
  /* A category with a selection gets a dot. It sits in the tab's padding, so
     the tab keeps its width and selecting a value cannot push a tab out. */
  .tab.filtering::after,
  .more.filtering::after {
    content: "";
    position: absolute;
    top: 9px;
    right: 3px;
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--txt-full);
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
    position: relative;
    flex: none;
    margin-left: auto; /* pin to the right edge, before Clear filters */
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
  .clear-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 26px;
    height: 100%;
    color: var(--txt-faded);
  }
  .clear-icon:hover:not(:disabled) {
    color: var(--txt-full);
  }

  /* --- Vertical list --- */
  /* In the tabs layout the value list draws the panel's right edge. Here the
     lists can all be collapsed, so the panel draws one edge for its whole
     height, down to the bottom bar, and the lists leave theirs out. */
  .facets.list {
    position: relative;
    padding-right: 1px;
  }
  .facets.list::after {
    content: "";
    position: absolute;
    top: 0;
    right: 0;
    bottom: var(--bottombar-h);
    width: 1px;
    background: var(--sidesline);
    pointer-events: none;
  }
  .facets.list :global(.valuelist) {
    border-right: none;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
    width: 100%;
    /* The shared header height, so the first category's bottom line meets the
       ones under the track list's and the queue's headers. */
    height: var(--header-h);
    padding: 0 12px 0 8px;
    border-bottom: 1px solid var(--hairline);
    text-align: left;
    color: var(--txt-faded);
  }
  .head:hover {
    color: var(--txt);
  }
  .head.expanded {
    color: var(--txt-full);
  }
  .chevron {
    display: flex;
    flex: none;
    transition: transform 120ms;
  }
  .head.expanded .chevron {
    transform: rotate(90deg);
  }
  .label {
    flex: none;
    white-space: nowrap;
    font-size: 10px;
    letter-spacing: 0.06em;
  }
  .picked {
    margin-left: auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11px;
    color: var(--txt);
  }
  /* The open list takes what the headers leave. */
  .body {
    display: flex;
    flex-direction: column;
    flex: 1 1 0;
    min-height: 0;
    border-bottom: 1px solid var(--hairline);
  }
  .tabs + .body {
    border-bottom: none;
  }
  /* Stays at the bottom of the panel when every category is collapsed. */
  .clear-all {
    flex: none;
    width: 100%;
    height: 28px;
    margin-top: auto;
    font-size: 11px;
    color: var(--txt);
  }
  .clear-all:hover:not(:disabled) {
    color: var(--txt-full);
    background: var(--hover-row);
  }
  .clear-icon:disabled,
  .clear-all:disabled {
    opacity: 0.35;
  }
</style>
