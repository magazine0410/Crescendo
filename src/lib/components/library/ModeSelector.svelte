<script lang="ts">
  import { customTagLabel, library } from "$lib/stores/library.svelte";
  import { plugins } from "$lib/stores/plugins.svelte";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { openSetFilter } from "./SetFilterDialog.svelte";
  import type { BrowseMode, PluginMenuControl } from "$lib/api";

  const modes: { id: BrowseMode; label: string }[] = [
    { id: "genre", label: "GENRE" },
    { id: "artist", label: "ARTIST" },
    { id: "album", label: "ALBUM" },
    { id: "albumartist", label: "ALBUM ARTIST" },
    { id: "custom", label: "CUSTOM" },
  ];

  /** The custom tab shows its resolved tag name (e.g. YEAR); until one is set it
      reads CUSTOM. Every other tab uses its fixed label. */
  function labelFor(m: { id: BrowseMode; label: string }): string {
    if (m.id !== "custom") return m.label;
    return customTagLabel(library.customTagName ?? "custom").toUpperCase();
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

  /** The CUSTOM tab's own menu: host-rendered controls contributed for the
      active tag by the immutable loaded registry, then the filter actions. */
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

  function tabMenu(e: MouseEvent, id: BrowseMode) {
    if (library.interactionBlocked) return;
    if (id === "custom") openContextMenu(e, customMenuItems());
  }

  /** The chevron's menu: the overflowed tabs, so a hidden one (e.g. CUSTOM)
      stays reachable. The active tab is check-marked.

      A hidden CUSTOM tab would otherwise take its whole menu with it. Choosing
      it here does not bring the tab back into the strip, so its plugin
      controls and filter actions have no other surface at this width; they are
      appended below the tab list, under the CUSTOM entry they belong to. */
  function moreMenu(e: MouseEvent) {
    if (library.interactionBlocked) return;
    const items: MenuItem[] = hiddenModes.map((m) => ({
      label: labelFor(m),
      checked: library.mode === m.id,
      action: () => library.setMode(m.id),
    }));
    if (hiddenModes.some((m) => m.id === "custom")) {
      items.push({ separator: true, label: "" }, ...customMenuItems());
    }
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
      disabled={library.interactionBlocked}
      onclick={() => library.setMode(m.id)}
      oncontextmenu={(e) => tabMenu(e, m.id)}
    >
      {labelFor(m)}
    </button>
  {/each}

  {#if hiddenModes.length}
    <button
      class="more"
      disabled={library.interactionBlocked}
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
