<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    call,
    isTauri,
    type ProjectMAssetEntry,
    type ProjectMImport,
    type ProjectMPresetEntry,
    type ProjectMPresetFile,
  } from "$lib/api";
  import { prefs, type VisualizerMode } from "$lib/stores/prefs.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import VirtualList from "$lib/virtual/VirtualList.svelte";
  import BarsVisualizer from "./BarsVisualizer.svelte";
  import ProjectMVisualizer from "./ProjectMVisualizer.svelte";

  interface Props {
    active?: boolean;
  }
  let { active = true }: Props = $props();

  interface BundledPreset {
    id: string;
    file: string;
    name: string;
    source: string;
    bytes: number;
  }

  interface PresetIndex {
    pack: string;
    presets: BundledPreset[];
  }

  type Choice =
    | { source: "en-d"; preset: BundledPreset }
    | { source: "import"; preset: ProjectMPresetEntry };

  type ChooserRow =
    | { type: "header"; label: string; count: number }
    | { type: "preset"; choice: Choice };

  let bundled = $state<BundledPreset[]>([]);
  let imported = $state<ProjectMImport | null>(null);
  let activePreset = $state<ProjectMPresetFile | null>(null);
  let activeAssets = $state<ProjectMAssetEntry[]>([]);
  let activeAssetRoot = $state("");
  let projectmMounted = $state(false);
  let chooserOpen = $state(false);
  let search = $state("");
  let busy = $state(false);
  let error = $state("");
  let notice = $state("");
  let dialog = $state<HTMLDivElement | null>(null);
  let searchInput = $state<HTMLInputElement | null>(null);

  const selectorValue = $derived.by(() => {
    switch (prefs.values.visualizer_mode) {
      case "projectm_en_d":
        return "en-d";
      case "projectm_preset":
        return "current-preset";
      case "projectm_import":
        return "current-import";
      default:
        return "bars";
    }
  });

  const filteredBundled = $derived.by(() => {
    const query = search.trim().toLocaleLowerCase();
    return query
      ? bundled.filter((preset) => preset.name.toLocaleLowerCase().includes(query))
      : bundled;
  });

  const filteredImported = $derived.by(() => {
    const presets = imported?.presets ?? [];
    const query = search.trim().toLocaleLowerCase();
    return query
      ? presets.filter((preset) => preset.name.toLocaleLowerCase().includes(query))
      : presets;
  });

  const chooserRows = $derived.by(() => {
    const rows: ChooserRow[] = [
      { type: "header", label: "En D", count: filteredBundled.length },
      ...filteredBundled.map(
        (preset): ChooserRow => ({ type: "preset", choice: { source: "en-d", preset } }),
      ),
    ];
    if (imported) {
      rows.push(
        { type: "header", label: imported.folder_name, count: filteredImported.length },
        ...filteredImported.map(
          (preset): ChooserRow => ({ type: "preset", choice: { source: "import", preset } }),
        ),
      );
    }
    return rows;
  });

  async function readBundled(preset: BundledPreset): Promise<ProjectMPresetFile> {
    const response = await fetch(`/projectm/presets/en-d/${preset.file}`);
    if (!response.ok) throw new Error(`Could not load En D preset: ${preset.name}`);
    return {
      id: `en-d:${preset.id}`,
      name: preset.name,
      data: await response.text(),
    };
  }

  async function setMode(mode: VisualizerMode) {
    await prefs.set("visualizer_mode", mode);
  }

  async function activateBundled(
    preset: BundledPreset,
    mode: "projectm_en_d" | "projectm_preset",
    foreground = true,
  ) {
    if (foreground) busy = true;
    error = "";
    notice = "";
    try {
      activeAssets = [];
      activeAssetRoot = "";
      activePreset = await readBundled(preset);
      projectmMounted = true;
      await prefs.set("visualizer_preset", activePreset.id);
      if (prefs.values.visualizer_mode !== mode) await setMode(mode);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      if (foreground) busy = false;
    }
  }

  async function readImported(preset: ProjectMPresetEntry): Promise<ProjectMPresetFile> {
    const path = prefs.values.visualizer_import_path;
    const loaded = await call<ProjectMPresetFile>("projectm_load_preset", {
      path,
      id: preset.id,
    });
    return {
      ...loaded,
      id: `import:${path}:${preset.id}`,
    };
  }

  async function activateImported(preset: ProjectMPresetEntry) {
    busy = true;
    error = "";
    try {
      activeAssets = imported?.assets ?? [];
      activeAssetRoot = prefs.values.visualizer_import_path;
      activePreset = await readImported(preset);
      projectmMounted = true;
      await prefs.set("visualizer_preset", preset.id);
      if (prefs.values.visualizer_mode !== "projectm_import") {
        await setMode("projectm_import");
      }
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }

  async function choose(choice: Choice) {
    closeChooser();
    if (choice.source === "en-d") await activateBundled(choice.preset, "projectm_preset");
    else await activateImported(choice.preset);
  }

  async function openChooser() {
    chooserOpen = true;
    await tick();
    searchInput?.focus();
  }

  function closeChooser() {
    chooserOpen = false;
    search = "";
  }

  async function importFolder() {
    if (!isTauri) {
      error = "Preset-folder import is available in the Crescendo desktop app.";
      return;
    }
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selected = await open({ directory: true, title: "Import projectM preset folder" });
    if (typeof selected !== "string") return;

    busy = true;
    error = "";
    notice = "";
    try {
      imported = await call<ProjectMImport>("projectm_import_folder", { path: selected });
      await prefs.set("visualizer_import_path", selected);
      notice = imported.warnings.join(" · ");
      await activateImported(imported.presets[0]);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }

  async function selectVisualizer(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const value = select.value;
    if (value === "bars") {
      error = "";
      notice = "";
      activePreset = null;
      activeAssets = [];
      activeAssetRoot = "";
      await setMode("bars");
    } else if (value === "en-d") {
      const saved = prefs.values.visualizer_preset;
      const match = bundled.find((preset) => `en-d:${preset.id}` === saved);
      const next = match ?? bundled[Math.floor(Math.random() * bundled.length)];
      if (next) await activateBundled(next, "projectm_en_d");
    } else if (value === "choose") {
      await openChooser();
      // The choice is not committed until a preset is picked.
      select.value = selectorValue;
    } else if (value === "import") {
      select.value = selectorValue;
      await importFolder();
    }
  }

  async function cycleEnD() {
    if (!bundled.length || busy || player.status !== "playing" || !active) return;
    const current = activePreset?.id;
    let candidates = bundled.filter((preset) => `en-d:${preset.id}` !== current);
    if (!candidates.length) candidates = bundled;
    const next = candidates[Math.floor(Math.random() * candidates.length)];
    if (!next) return;
    try {
      activeAssets = [];
      activeAssetRoot = "";
      activePreset = await readBundled(next);
      await prefs.set("visualizer_preset", activePreset.id);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    }
  }

  async function restore() {
    busy = true;
    try {
      const response = await fetch("/projectm/presets/en-d/index.json");
      if (!response.ok) throw new Error("The bundled En D preset pack is unavailable");
      const index = (await response.json()) as PresetIndex;
      bundled = index.presets;

      const mode = prefs.values.visualizer_mode;
      if (mode === "projectm_import") {
        if (!isTauri) {
          throw new Error("Imported projectM presets are available only in the desktop app");
        }
        if (!prefs.values.visualizer_import_path) {
          throw new Error("The imported projectM preset folder is no longer configured");
        }
        imported = await call<ProjectMImport>("projectm_import_folder", {
          path: prefs.values.visualizer_import_path,
        });
        notice = imported.warnings.join(" · ");
        const selected =
          imported.presets.find((preset) => preset.id === prefs.values.visualizer_preset) ??
          imported.presets[0];
        activeAssets = imported.assets;
        activeAssetRoot = prefs.values.visualizer_import_path;
        activePreset = await readImported(selected);
        projectmMounted = true;
      } else if (mode !== "bars") {
        const selected =
          bundled.find((preset) => `en-d:${preset.id}` === prefs.values.visualizer_preset) ??
          bundled[0];
        if (selected) {
          activeAssets = [];
          activeAssetRoot = "";
          activePreset = await readBundled(selected);
          projectmMounted = true;
        }
      }
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
      if (prefs.values.visualizer_mode !== "bars") await setMode("bars");
    } finally {
      busy = false;
    }
  }

  async function onProjectMError(message: string) {
    projectmMounted = false;
    activePreset = null;
    activeAssets = [];
    activeAssetRoot = "";
    await setMode("bars");
    error = `projectM could not start; switched to Bars (Eole). ${message}`;
  }

  function onDialogKeydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      closeChooser();
      return;
    }
    if (event.key === "Tab") {
      const focusable = Array.from(
        dialog?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ) ?? [],
      ).filter((element) => element.offsetParent !== null);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  }

  $effect(() => {
    if (
      prefs.values.visualizer_mode !== "projectm_en_d" ||
      !bundled.length ||
      player.status !== "playing" ||
      !active
    )
      return;
    const timer = window.setInterval(() => void cycleEnD(), 30_000);
    return () => window.clearInterval(timer);
  });

  onMount(() => void restore());
</script>

<div class="vis" style:padding-bottom={`${ui.bottombarH}px`}>
  <div class="surface">
    <div class="toolbar">
      <select
        aria-label="Visualizer"
        title="Visualizer"
        value={selectorValue}
        disabled={busy}
        onchange={selectVisualizer}
      >
        <option value="bars">Bars (Eole)</option>
        <option value="en-d">MilkDrop: En D</option>
        {#if prefs.values.visualizer_mode === "projectm_preset"}
          <option value="current-preset">MilkDrop: Selected preset</option>
        {/if}
        <option value="choose">Choose preset</option>
        {#if prefs.values.visualizer_mode === "projectm_import"}
          <option value="current-import">MilkDrop: Imported folder</option>
        {/if}
        <option value="import">Import preset folder...</option>
      </select>
      {#if activePreset && prefs.values.visualizer_mode !== "bars"}
        <span class="preset-name" title={activePreset.name}>{activePreset.name}</span>
      {/if}
    </div>

    <div class="viewport">
      {#if prefs.values.visualizer_mode === "bars"}
        <BarsVisualizer {active} />
      {:else if !projectmMounted}
        <div class="empty">{busy ? "Loading visualizer…" : "Select a projectM preset"}</div>
      {/if}
      {#if projectmMounted}
        <div class="projectm-layer" class:shown={prefs.values.visualizer_mode !== "bars"}>
          <ProjectMVisualizer
            preset={activePreset}
            assets={activeAssets}
            assetRoot={activeAssetRoot}
            active={active && prefs.values.visualizer_mode !== "bars"}
            onerror={onProjectMError}
          />
        </div>
      {/if}

      {#if error}
        <button class="error" title="Dismiss" onclick={() => (error = "")}>{error}</button>
      {:else if notice}
        <button class="notice" title="Dismiss" onclick={() => (notice = "")}>{notice}</button>
      {/if}
    </div>
  </div>
</div>

{#if chooserOpen}
  <div
    class="backdrop"
    role="presentation"
    onclick={(event) => event.target === event.currentTarget && closeChooser()}
  >
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-label="Choose projectM preset"
      tabindex="-1"
      bind:this={dialog}
      onkeydown={onDialogKeydown}
    >
      <header>
        <strong>Choose preset</strong>
        <button class="close" title="Close" onclick={closeChooser}>✕</button>
      </header>
      <input
        class="search"
        type="search"
        placeholder="Filter presets…"
        bind:this={searchInput}
        bind:value={search}
      />
      <div class="preset-list">
        <VirtualList count={chooserRows.length} itemHeight={30} row={chooserRow} overscan={12} />
      </div>
    </div>
  </div>
{/if}

{#snippet chooserRow(index: number)}
  {@const row = chooserRows[index]}
  {#if row.type === "header"}
    <h3>{row.label} <span>{row.count}</span></h3>
  {:else}
    <button class="preset" onclick={() => choose(row.choice)}>
      {row.choice.preset.name}
    </button>
  {/if}
{/snippet}

<style>
  .vis {
    height: 100%;
    box-sizing: border-box;
  }
  .surface {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    overflow: hidden;
  }
  .viewport {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
    overflow: hidden;
  }
  .empty {
    height: 100%;
    display: grid;
    place-items: center;
    color: var(--txt-faded);
  }
  .projectm-layer {
    display: none;
    position: absolute;
    inset: 0;
  }
  .projectm-layer.shown {
    display: block;
  }
  .toolbar {
    display: flex;
    flex: 0 0 52px;
    align-items: center;
    gap: 14px;
    min-width: 0;
    padding: 0 9px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--hairline);
    box-sizing: border-box;
  }
  select {
    flex: 0 0 171px;
    height: 32px;
    padding: 0 25px 0 8px;
    color: var(--txt);
    appearance: none;
    -webkit-appearance: none;
    background-color: var(--bg-panel);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10' viewBox='0 0 24 24' fill='none' stroke='%236e6e6e' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 7px center;
    border: 1px solid var(--border);
    border-radius: 2px;
    cursor: pointer;
  }
  select:disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  select option {
    color: var(--txt);
    background: var(--bg-main);
  }
  .preset-name {
    min-width: 0;
    overflow: hidden;
    max-width: min(52vw, 620px);
    color: var(--txt-semifaded);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .error,
  .notice {
    position: absolute;
    left: 14px;
    bottom: 12px;
    z-index: 5;
    max-width: min(620px, calc(100% - 28px));
    padding: 7px 10px;
    color: #efb0b0;
    text-align: left;
    background: rgba(65, 24, 24, 0.92);
    border: 1px solid rgba(219, 106, 106, 0.55);
  }
  .notice {
    color: #e8d39a;
    background: rgba(57, 46, 20, 0.94);
    border-color: rgba(205, 166, 65, 0.55);
  }
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 300;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.58);
  }
  .dialog {
    display: flex;
    flex-direction: column;
    width: min(680px, calc(100vw - 48px));
    height: min(650px, calc(100vh - 72px));
    background: var(--bg-main);
    border: 1px solid var(--popup-border);
    box-shadow: 0 8px 40px rgba(0, 0, 0, 0.72);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 11px 14px;
    border-bottom: 1px solid var(--hairline);
  }
  .close {
    color: var(--txt-faded);
  }
  .search {
    margin: 12px 14px 8px;
    padding: 7px 9px;
    color: var(--txt);
    background: var(--bg-input, #202020);
    border: 1px solid var(--hairline);
  }
  .preset-list {
    flex: 1;
    min-height: 0;
    margin: 0 8px 10px;
    overflow: hidden;
  }
  h3 {
    height: 100%;
    margin: 0;
    padding: 9px 6px 0;
    box-sizing: border-box;
    color: var(--txt-faded);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
  }
  h3 span {
    margin-left: 5px;
    font-weight: 400;
  }
  .preset {
    display: block;
    width: 100%;
    height: 100%;
    padding: 0 8px;
    overflow: hidden;
    color: var(--txt);
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .preset:hover,
  .preset:focus-visible {
    background: var(--hover-row);
  }
</style>
