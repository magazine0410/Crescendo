<script lang="ts">
  import { onMount } from "svelte";
  import { call, isTauri, type ProjectMAssetFile, type ProjectMImport, type ProjectMPresetFile } from "$lib/api";
  import { prefs, type VisualizerMode } from "$lib/stores/prefs.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import BarsVisualizer from "./BarsVisualizer.svelte";
  import ProjectMVisualizer from "./ProjectMVisualizer.svelte";

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
    | { source: "import"; preset: ProjectMPresetFile };

  let bundled = $state<BundledPreset[]>([]);
  let imported = $state<ProjectMImport | null>(null);
  let activePreset = $state<ProjectMPresetFile | null>(null);
  let activeAssets = $state<ProjectMAssetFile[]>([]);
  let chooserOpen = $state(false);
  let search = $state("");
  let busy = $state(false);
  let error = $state("");

  const selectorValue = $derived.by(() => {
    switch (prefs.values.visualizer_mode) {
      case "projectm_en_d":
        return "en-d";
      case "projectm_preset":
        return "choose";
      case "projectm_import":
        return "import";
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

  async function activateBundled(preset: BundledPreset, mode: "projectm_en_d" | "projectm_preset") {
    busy = true;
    error = "";
    try {
      activeAssets = [];
      activePreset = await readBundled(preset);
      await prefs.set("visualizer_preset", activePreset.id);
      await setMode(mode);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      busy = false;
    }
  }

  function importedForRenderer(preset: ProjectMPresetFile): ProjectMPresetFile {
    return {
      ...preset,
      id: `import:${prefs.values.visualizer_import_path}:${preset.id}`,
    };
  }

  async function activateImported(preset: ProjectMPresetFile) {
    activeAssets = imported?.assets ?? [];
    activePreset = importedForRenderer(preset);
    await prefs.set("visualizer_preset", preset.id);
    await setMode("projectm_import");
  }

  async function choose(choice: Choice) {
    chooserOpen = false;
    search = "";
    if (choice.source === "en-d") await activateBundled(choice.preset, "projectm_preset");
    else await activateImported(choice.preset);
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
    try {
      imported = await call<ProjectMImport>("projectm_import_folder", { path: selected });
      await prefs.set("visualizer_import_path", selected);
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
      activePreset = null;
      activeAssets = [];
      await setMode("bars");
    } else if (value === "en-d") {
      const saved = prefs.values.visualizer_preset;
      const match = bundled.find((preset) => `en-d:${preset.id}` === saved);
      const next = match ?? bundled[Math.floor(Math.random() * bundled.length)];
      if (next) await activateBundled(next, "projectm_en_d");
    } else if (value === "choose") {
      chooserOpen = true;
      // The choice is not committed until a preset is picked.
      select.value = selectorValue;
    } else if (value === "import") {
      select.value = selectorValue;
      await importFolder();
    }
  }

  async function cycleEnD() {
    if (!bundled.length || busy) return;
    const current = activePreset?.id;
    let candidates = bundled.filter((preset) => `en-d:${preset.id}` !== current);
    if (!candidates.length) candidates = bundled;
    await activateBundled(
      candidates[Math.floor(Math.random() * candidates.length)],
      "projectm_en_d",
    );
  }

  async function restore() {
    busy = true;
    try {
      const response = await fetch("/projectm/presets/en-d/index.json");
      if (!response.ok) throw new Error("The bundled En D preset pack is unavailable");
      const index = (await response.json()) as PresetIndex;
      bundled = index.presets;

      const mode = prefs.values.visualizer_mode;
      if (mode === "projectm_import" && prefs.values.visualizer_import_path && isTauri) {
        imported = await call<ProjectMImport>("projectm_import_folder", {
          path: prefs.values.visualizer_import_path,
        });
        const selected =
          imported.presets.find((preset) => preset.id === prefs.values.visualizer_preset) ??
          imported.presets[0];
        activeAssets = imported.assets;
        activePreset = importedForRenderer(selected);
      } else if (mode !== "bars") {
        const selected =
          bundled.find((preset) => `en-d:${preset.id}` === prefs.values.visualizer_preset) ??
          bundled[0];
        if (selected) {
          activeAssets = [];
          activePreset = await readBundled(selected);
        }
      }
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
      if (prefs.values.visualizer_mode !== "bars") await setMode("bars");
    } finally {
      busy = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && chooserOpen) {
      chooserOpen = false;
      search = "";
    }
  }

  $effect(() => {
    if (prefs.values.visualizer_mode !== "projectm_en_d" || !bundled.length) return;
    const timer = window.setInterval(() => void cycleEnD(), 30_000);
    return () => window.clearInterval(timer);
  });

  onMount(() => void restore());
</script>

<svelte:window onkeydown={onKeydown} />

<div class="vis" style:padding-bottom={`${ui.bottombarH}px`}>
  <div class="surface">
    {#if prefs.values.visualizer_mode === "bars"}
      <BarsVisualizer />
    {:else if activePreset}
      <ProjectMVisualizer preset={activePreset} assets={activeAssets} />
    {:else}
      <div class="empty">{busy ? "Loading visualizer…" : "Select a projectM preset"}</div>
    {/if}

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
        <option value="choose">Choose preset</option>
        <option value="import">Import preset folder...</option>
      </select>
      {#if activePreset && prefs.values.visualizer_mode !== "bars"}
        <span class="preset-name" title={activePreset.name}>{activePreset.name}</span>
      {/if}
    </div>

    {#if error}
      <button class="error" title="Dismiss" onclick={() => (error = "")}>{error}</button>
    {/if}
  </div>
</div>

{#if chooserOpen}
  <div
    class="backdrop"
    role="presentation"
    onclick={(event) => event.target === event.currentTarget && (chooserOpen = false)}
  >
    <div class="dialog" role="dialog" aria-modal="true" aria-label="Choose projectM preset">
      <header>
        <strong>Choose preset</strong>
        <button class="close" title="Close" onclick={() => (chooserOpen = false)}>✕</button>
      </header>
      <input class="search" type="search" placeholder="Filter presets…" bind:value={search} />
      <div class="preset-list">
        <h3>En D <span>{filteredBundled.length}</span></h3>
        {#each filteredBundled as item (item.id)}
          <button class="preset" onclick={() => choose({ source: "en-d", preset: item })}>
            {item.name}
          </button>
        {/each}
        {#if imported}
          <h3>{imported.folder_name} <span>{filteredImported.length}</span></h3>
          {#each filteredImported as item (item.id)}
            <button class="preset" onclick={() => choose({ source: "import", preset: item })}>
              {item.name}
            </button>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .vis {
    height: 100%;
    box-sizing: border-box;
  }
  .surface {
    position: relative;
    width: 100%;
    height: 100%;
    overflow: hidden;
  }
  .empty {
    height: 100%;
    display: grid;
    place-items: center;
    color: var(--txt-faded);
  }
  .toolbar {
    position: absolute;
    top: 12px;
    left: 14px;
    display: flex;
    align-items: center;
    max-width: calc(100% - 80px);
    gap: 9px;
    z-index: 4;
  }
  select {
    height: 28px;
    min-width: 154px;
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
    box-shadow: 0 2px 9px rgba(0, 0, 0, 0.35);
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
    overflow: hidden;
    max-width: min(48vw, 520px);
    color: rgba(255, 255, 255, 0.68);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-shadow: 0 1px 3px #000;
  }
  .error {
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
    min-height: 0;
    padding: 0 8px 10px;
    overflow-y: auto;
  }
  h3 {
    margin: 10px 6px 5px;
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
    padding: 6px 8px;
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
