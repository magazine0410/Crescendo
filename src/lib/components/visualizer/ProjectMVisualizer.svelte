<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { call, isTauri, type ProjectMAssetFile, type ProjectMPresetFile } from "$lib/api";

  interface Props {
    preset: ProjectMPresetFile | null;
    assets?: ProjectMAssetFile[];
    onready?: () => void;
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
    _crescendo_projectm_init(width: number, height: number): number;
    _crescendo_projectm_destroy(): void;
    _crescendo_projectm_resize(width: number, height: number): void;
    _crescendo_projectm_render(): void;
    _crescendo_projectm_add_pcm(samples: number, frames: number): void;
    _crescendo_projectm_load_preset(data: number, smooth: number): void;
    _crescendo_projectm_refresh_textures(): void;
  }

  let { preset, assets = [], onready }: Props = $props();

  let host: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let module = $state<ProjectMModule | null>(null);
  let status = $state<"loading" | "ready" | "error">("loading");
  let error = $state("");

  let disposed = false;
  let raf = 0;
  let resizeObserver: ResizeObserver | null = null;
  let unlisten: (() => void) | null = null;
  let pcmPointer = 0;
  let pcmCapacity = 0;
  let mockPhase = 0;
  let loadedPresetId = "";
  let installedAssetKey = "";

  function runtimeFile(path: string): string {
    if (path.endsWith(".wasm")) return "/projectm/projectm.wasm";
    if (path.endsWith(".data")) return "/projectm/projectm.data";
    return `/projectm/${path.split("/").pop()}`;
  }

  function dimensions(): [number, number] {
    const dpr = window.devicePixelRatio || 1;
    return [
      Math.max(1, Math.round(host.clientWidth * dpr)),
      Math.max(1, Math.round(host.clientHeight * dpr)),
    ];
  }

  function resize() {
    const active = module;
    if (!active || status !== "ready") return;
    const [width, height] = dimensions();
    if (canvas.width === width && canvas.height === height) return;
    canvas.width = width;
    canvas.height = height;
    active._crescendo_projectm_resize(width, height);
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

  function addPcm(samples: number[] | Int16Array) {
    const active = module;
    if (!active || status !== "ready") return;
    // Keep each copy bounded. GStreamer normally sends far fewer than this,
    // but imported/slow webview frames must not grow one giant WASM allocation.
    const maxSamples = 8192;
    for (let offset = 0; offset + 1 < samples.length; offset += maxSamples) {
      const count = Math.min(maxSamples, samples.length - offset) & ~1;
      const pointer = ensurePcmCapacity(count);
      active.HEAP16.set(
        samples instanceof Int16Array
          ? samples.subarray(offset, offset + count)
          : samples.slice(offset, offset + count),
        pointer >> 1,
      );
      active._crescendo_projectm_add_pcm(pointer, count / 2);
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
    const active = module;
    if (!active || status !== "ready") return;
    if (!isTauri) feedPreviewAudio();
    active._crescendo_projectm_render();
  }

  function decodeBase64(value: string): Uint8Array {
    const binary = atob(value);
    const out = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
    return out;
  }

  function safeAssetName(name: string): string {
    return name.replaceAll("\\", "_").replaceAll("/", "_").replaceAll("..", "_");
  }

  function installAssets(active: ProjectMModule, next: ProjectMAssetFile[]) {
    const key = next.map((asset) => `${asset.name}:${asset.data_base64.length}`).join("|");
    if (key === installedAssetKey) return;
    active.FS.mkdirTree("/imports");
    for (const old of active.FS.readdir("/imports")) {
      if (old !== "." && old !== "..") active.FS.unlink(`/imports/${old}`);
    }
    for (const asset of next) {
      active.FS.writeFile(`/imports/${safeAssetName(asset.name)}`, decodeBase64(asset.data_base64));
    }
    active._crescendo_projectm_refresh_textures();
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

  async function initialise() {
    try {
      // Keep the prebuilt Emscripten module in `static/`; a variable prevents
      // Vite/TypeScript from trying to bundle or resolve it as application TS.
      const moduleUrl = "/projectm/projectm.js";
      const imported = await import(/* @vite-ignore */ moduleUrl);
      if (disposed) return;
      const [width, height] = dimensions();
      canvas.width = width;
      canvas.height = height;
      const active = (await imported.default({
        canvas,
        locateFile: runtimeFile,
        printErr: (message: unknown) => console.error("projectM:", message),
      })) as ProjectMModule;
      if (disposed) {
        active._crescendo_projectm_destroy();
        return;
      }
      module = active;
      active.FS.mkdirTree("/imports");
      if (!active._crescendo_projectm_init(width, height)) {
        throw new Error(
          active.UTF8ToString(active._crescendo_projectm_last_error()) ||
            "projectM could not initialize",
        );
      }
      status = "ready";
      installAssets(active, assets);
      if (preset) loadPreset(active, preset);
      resizeObserver = new ResizeObserver(resize);
      resizeObserver.observe(host);
      onready?.();
    } catch (cause) {
      console.error(cause);
      error = cause instanceof Error ? cause.message : String(cause);
      status = "error";
    }
  }

  $effect(() => {
    const active = module;
    const nextPreset = preset;
    const nextAssets = assets;
    if (active && status === "ready") {
      // Texture lookup happens while a preset is parsed, so imported assets
      // must enter the virtual filesystem before that preset is loaded.
      installAssets(active, nextAssets);
      if (nextPreset) loadPreset(active, nextPreset);
    }
  });

  onMount(() => {
    if (isTauri) {
      call("player_set_projectm_enabled", { on: true }).catch(() => {});
      listen<{ samples: number[] }>("player:projectm-pcm", (event) => addPcm(event.payload.samples))
        .then((stop) => {
          if (disposed) stop();
          else unlisten = stop;
        })
        .catch(() => {});
    }
    raf = requestAnimationFrame(render);
    void initialise();
  });

  onDestroy(() => {
    disposed = true;
    cancelAnimationFrame(raf);
    resizeObserver?.disconnect();
    unlisten?.();
    const active = module;
    if (active) {
      if (pcmPointer) active._free(pcmPointer);
      active._crescendo_projectm_destroy();
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
