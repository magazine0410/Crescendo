<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { call, isTauri, type ProjectMAssetEntry, type ProjectMPresetFile } from "$lib/api";

  interface Props {
    preset: ProjectMPresetFile | null;
    assets?: ProjectMAssetEntry[];
    assetRoot?: string;
    active?: boolean;
    /** The renderer itself cannot run; the caller is expected to fall back. */
    onerror?: (message: string) => void;
    /** A preset or its textures failed after startup. The previously loaded
        visual keeps rendering, so this only needs reporting. */
    onpresetfail?: (message: string) => void;
  }

  interface ProjectMModule {
    FS: {
      mkdirTree(path: string): void;
      readdir(path: string): string[];
      unlink(path: string): void;
      writeFile(path: string, data: Uint8Array): void;
    };
    HEAP16: Int16Array;
    HEAPU8: Uint8Array;
    UTF8ToString(pointer: number): string;
    _malloc(bytes: number): number;
    _free(pointer: number): void;
    _crescendo_projectm_last_error(): number;
    _crescendo_projectm_init(
      width: number,
      height: number,
      drawableWidth: number,
      drawableHeight: number,
    ): number;
    _crescendo_projectm_destroy(): void;
    _crescendo_projectm_resize(
      width: number,
      height: number,
      drawableWidth: number,
      drawableHeight: number,
    ): void;
    _crescendo_projectm_render(): void;
    _crescendo_projectm_add_pcm(samples: number, frames: number): void;
    _crescendo_projectm_load_preset(data: number, smooth: number): void;
    _crescendo_projectm_refresh_textures(): void;
  }

  let {
    preset,
    assets = [],
    assetRoot = "",
    active = true,
    onerror,
    onpresetfail,
  }: Props = $props();

  let host: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let module = $state<ProjectMModule | null>(null);
  let status = $state<"loading" | "ready" | "error">("loading");
  let error = $state("");

  let disposed = false;
  let raf = 0;
  let resizeObserver: ResizeObserver | null = null;
  let pcmPointer = 0;
  let pcmCapacity = 0;
  let mockPhase = 0;
  let loadedPresetId = "";
  let installedAssetKey = "";
  let selectionGeneration = 0;
  let pcmPulling = false;
  let cssWidth = 0;
  let cssHeight = 0;

  /** Emscripten asks for `projectm.wasm` and `projectm.data` by bare name. */
  function runtimeFile(path: string): string {
    return `/projectm/${path.split("/").pop()}`;
  }

  function dimensions(): [number, number] {
    return [
      Math.max(1, Math.round(host.clientWidth)),
      Math.max(1, Math.round(host.clientHeight)),
    ];
  }

  function resize() {
    const active = module;
    if (!active || status !== "ready") return;
    const [width, height] = dimensions();
    if (cssWidth === width && cssHeight === height) return;
    cssWidth = width;
    cssHeight = height;
    sizeCanvasBackingStore(width, height);
    active._crescendo_projectm_resize(width, height, canvas.width, canvas.height);
  }

  function sizeCanvasBackingStore(width: number, height: number) {
    const scale = window.devicePixelRatio || 1;
    canvas.width = Math.max(1, Math.round(width * scale));
    canvas.height = Math.max(1, Math.round(height * scale));
  }

  function ensurePcmCapacity(sampleCount: number): number {
    const active = module;
    if (!active) return 0;
    if (sampleCount <= pcmCapacity) return pcmPointer;
    if (pcmPointer) active._free(pcmPointer);
    pcmCapacity = Math.max(sampleCount, 4096);
    pcmPointer = active._malloc(pcmCapacity * Int16Array.BYTES_PER_ELEMENT);
    return pcmPointer;
  }

  function addPcm(samples: Int16Array) {
    const runtime = module;
    if (!runtime || status !== "ready" || !active) return;
    // Keep each copy bounded: one pull can carry up to MAX_PROJECTM_PCM_BYTES
    // (128 KiB, so 65,536 samples), which must not grow one giant allocation.
    // `enqueue_projectm_pcm` only ever hands back whole stereo frames, so both
    // the total and every chunk of it are even and frame-aligned.
    const maxSamples = 8192;
    for (let offset = 0; offset < samples.length; offset += maxSamples) {
      const count = Math.min(maxSamples, samples.length - offset);
      const pointer = ensurePcmCapacity(count);
      runtime.HEAP16.set(samples.subarray(offset, offset + count), pointer >> 1);
      runtime._crescendo_projectm_add_pcm(pointer, count / 2);
    }
  }

  function feedPreviewAudio() {
    // Browser preview has no GStreamer stream. A two-tone signal makes the
    // real projectM audio analyser visibly respond while developing the UI.
    const frames = 735; // 44,100 / 60
    const samples = new Int16Array(frames * 2);
    for (let i = 0; i < frames; i++) {
      const beat = 0.55 + 0.45 * Math.sin(mockPhase * 0.125) ** 2;
      const value = Math.round(
        9000 * beat * Math.sin(mockPhase) + 3500 * Math.sin(mockPhase * 2.013),
      );
      samples[i * 2] = value;
      samples[i * 2 + 1] = value;
      mockPhase += (2 * Math.PI * 110) / 44100;
    }
    addPcm(samples);
  }

  function render() {
    if (disposed) return;
    raf = requestAnimationFrame(render);
    const runtime = module;
    if (!runtime || status !== "ready" || !active) return;
    if (!isTauri) feedPreviewAudio();
    else void pullPcm();
    runtime._crescendo_projectm_render();
  }

  /** Rust sends bare file names, so this only guards the WASM filesystem
      against separators that are legal in a Linux file name. */
  function safeAssetName(name: string): string {
    return name.replaceAll("\\", "_").replaceAll("/", "_");
  }

  async function installAssets(
    runtime: ProjectMModule,
    next: ProjectMAssetEntry[],
    root: string,
    generation: number,
  ) {
    const key = `${root}|${next.map((asset) => `${asset.id}:${asset.bytes}`).join("|")}`;
    if (key === installedAssetKey) return;
    // Every bail-out below leaves /imports wiped or half-written, so stop
    // claiming any particular set is installed until the last file lands.
    installedAssetKey = "";
    runtime.FS.mkdirTree("/imports");
    for (const old of runtime.FS.readdir("/imports")) {
      if (old !== "." && old !== "..") runtime.FS.unlink(`/imports/${old}`);
    }
    for (const asset of next) {
      if (!root || disposed || generation !== selectionGeneration) return;
      const response = await call<ArrayBuffer>("projectm_load_asset", {
        path: root,
        id: asset.id,
      });
      if (disposed || generation !== selectionGeneration) return;
      runtime.FS.writeFile(`/imports/${safeAssetName(asset.name)}`, new Uint8Array(response));
    }
    runtime._crescendo_projectm_refresh_textures();
    installedAssetKey = key;
  }

  function loadPreset(active: ProjectMModule, next: ProjectMPresetFile) {
    if (next.id === loadedPresetId) return;
    const encoded = new TextEncoder().encode(`${next.data}\0`);
    const pointer = active._malloc(encoded.length);
    active.HEAPU8.set(encoded, pointer);
    active._crescendo_projectm_load_preset(pointer, loadedPresetId ? 1 : 0);
    active._free(pointer);
    loadedPresetId = next.id;
  }

  /** `fatal` is set only for the startup selection, which has no earlier
      preset to fall back on: a failure there means the renderer never runs.
      Later failures leave the previous preset on screen and are only
      reported, so one unreadable file cannot take the visualizer down. */
  async function applySelection(
    runtime: ProjectMModule,
    nextPreset: ProjectMPresetFile | null,
    nextAssets: ProjectMAssetEntry[],
    root: string,
    fatal = false,
  ) {
    const generation = ++selectionGeneration;
    try {
      // Texture lookup happens while a preset is parsed, so imported assets
      // must enter the virtual filesystem before that preset is loaded.
      await installAssets(runtime, nextAssets, root, generation);
      if (!disposed && generation === selectionGeneration && nextPreset) {
        loadPreset(runtime, nextPreset);
      }
    } catch (cause) {
      if (fatal) {
        fail(cause);
      } else {
        console.error(cause);
        onpresetfail?.(cause instanceof Error ? cause.message : String(cause));
      }
    }
  }

  async function pullPcm() {
    if (pcmPulling || !active || status !== "ready") return;
    pcmPulling = true;
    try {
      const response = await call<ArrayBuffer>("player_take_projectm_pcm");
      if (active && response.byteLength >= 4) addPcm(new Int16Array(response));
    } catch {
      // A transient IPC failure should not tear down a working renderer.
    } finally {
      pcmPulling = false;
    }
  }

  function fail(cause: unknown) {
    console.error(cause);
    error = cause instanceof Error ? cause.message : String(cause);
    status = "error";
    onerror?.(error);
  }

  async function initialise() {
    try {
      // Keep the prebuilt Emscripten module in `static/`; a variable prevents
      // Vite/TypeScript from trying to bundle or resolve it as application TS.
      const moduleUrl = "/projectm/projectm.js";
      const imported = await import(/* @vite-ignore */ moduleUrl);
      if (disposed) return;
      const [width, height] = dimensions();
      cssWidth = width;
      cssHeight = height;
      // SDL's Emscripten backend treats an existing canvas as externally
      // sized and does not initialize its default 300×150 backing store.
      // Size it physically here while still passing CSS pixels to SDL below,
      // avoiding both an undersized canvas and double DPR multiplication.
      sizeCanvasBackingStore(width, height);
      const runtime = (await imported.default({
        canvas,
        locateFile: runtimeFile,
        printErr: (message: unknown) => console.error("projectM:", message),
      })) as ProjectMModule;
      if (disposed) {
        runtime._crescendo_projectm_destroy();
        return;
      }
      module = runtime;
      runtime.FS.mkdirTree("/imports");
      if (!runtime._crescendo_projectm_init(width, height, canvas.width, canvas.height)) {
        throw new Error(
          runtime.UTF8ToString(runtime._crescendo_projectm_last_error()) ||
            "projectM could not initialize",
        );
      }
      await applySelection(runtime, preset, assets, assetRoot, true);
      if (status === "error") return;
      status = "ready";
      resizeObserver = new ResizeObserver(resize);
      resizeObserver.observe(host);
    } catch (cause) {
      fail(cause);
    }
  }

  $effect(() => {
    const runtime = module;
    const nextPreset = preset;
    const nextAssets = assets;
    const nextRoot = assetRoot;
    if (runtime && status === "ready") {
      void applySelection(runtime, nextPreset, nextAssets, nextRoot);
    }
  });

  $effect(() => {
    const enabled = isTauri && active && status === "ready";
    if (isTauri) call("player_set_projectm_enabled", { on: enabled }).catch(() => {});
    return () => {
      if (enabled) call("player_set_projectm_enabled", { on: false }).catch(() => {});
    };
  });

  onMount(() => {
    raf = requestAnimationFrame(render);
    void initialise();
  });

  onDestroy(() => {
    disposed = true;
    cancelAnimationFrame(raf);
    resizeObserver?.disconnect();
    const runtime = module;
    if (runtime) {
      if (pcmPointer) runtime._free(pcmPointer);
      runtime._crescendo_projectm_destroy();
    }
    if (isTauri) call("player_set_projectm_enabled", { on: false }).catch(() => {});
  });
</script>

<div class="projectm" bind:this={host}>
  <canvas bind:this={canvas} aria-label="projectM MilkDrop visualizer"></canvas>
  {#if status === "loading"}
    <div class="message">Starting projectM…</div>
  {:else if status === "error"}
    <div class="message error">
      <strong>projectM could not start</strong>
      <span>{error}</span>
    </div>
  {/if}
</div>

<style>
  .projectm {
    position: relative;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: #000;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }
  .message {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 7px;
    color: var(--txt-faded);
    background: var(--bg-main);
  }
  .message.error strong {
    color: #d98282;
  }
  .message.error span {
    max-width: 520px;
    padding: 0 24px;
    text-align: center;
  }
</style>
