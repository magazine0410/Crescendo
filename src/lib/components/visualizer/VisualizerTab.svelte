<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { ui } from "$lib/stores/ui.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { isTauri, call } from "$lib/api";
  import { clamp, hexToRgba } from "$lib/color";
  import ColorPicker from "../common/ColorPicker.svelte";

  // --- tuning constants (visual only) ---
  const BLOCKS = 28; // vertical blocks per bar
  const BAR_MIN_PX = 24; // min bar footprint -> derives bar count from width
  const H_GAP = 3; // horizontal gap between bars (css px)
  const V_GAP = 2; // vertical gap between blocks (css px)
  const MIN_DB = -72; // spectrum magnitude mapped to an empty bar
  const MAX_DB = -14; // spectrum magnitude mapped to a full bar
  const ATTACK = 0.55; // rise smoothing (higher = snappier)
  const DECAY = 0.14; // fall smoothing
  const PEAK_GRAVITY = 0.013; // per-frame fall of the peak cap (higher = hugs the bar)
  const SILENCE_MS = 400; // no spectrum message for this long -> decay to zero
  const LABEL_H = 18; // bottom strip reserved for the Hz axis
  const FFT_BANDS = 512; // matches the backend spectrum `bands` (for the axis in preview)
  const MIN_IDX = 1; // skip the DC-ish first band (shared by binning + axis)
  const FREQ_TICKS = [50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];

  let stage: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let ctx: CanvasRenderingContext2D | null = null;
  let pickerOpen = $state(false);

  // Non-reactive hot state (updated ~30/s or per-frame; kept out of runes).
  let rawBands: number[] = [];
  let sampleRate = 44100; // real rate from the pipeline; drives the Hz axis
  let labelColor = "#6e6e6e"; // --txt-faded, read once on mount
  let lastSpectrumAt = 0;
  let levels: Float32Array = new Float32Array(0);
  let peaks: Float32Array = new Float32Array(0);
  let cssW = 0;
  let cssH = 0;
  let raf = 0;
  let unlisten: (() => void) | null = null;

  function resizeCanvas() {
    const w = stage.clientWidth;
    const h = stage.clientHeight;
    if (w === cssW && h === cssH) return;
    cssW = w;
    cssH = h;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(1, Math.round(w * dpr));
    canvas.height = Math.max(1, Math.round(h * dpr));
    if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  function barCount(): number {
    return Math.max(8, Math.floor(cssW / BAR_MIN_PX));
  }

  function ensureArrays(n: number) {
    if (levels.length !== n) {
      levels = new Float32Array(n);
      peaks = new Float32Array(n);
    }
  }

  /** Log-bin the raw dB bands into `n` normalized (0..1) targets, bass -> treble. */
  function realTargets(n: number, out: Float32Array) {
    const N = rawBands.length;
    const silent = performance.now() - lastSpectrumAt > SILENCE_MS;
    if (N === 0 || silent) {
      out.fill(0);
      return;
    }
    for (let j = 0; j < n; j++) {
      let lo = Math.floor(MIN_IDX * Math.pow(N / MIN_IDX, j / n));
      let hi = Math.floor(MIN_IDX * Math.pow(N / MIN_IDX, (j + 1) / n));
      lo = clamp(lo, MIN_IDX, N - 1);
      hi = clamp(Math.max(hi, lo + 1), MIN_IDX + 1, N);
      let sum = 0;
      for (let k = lo; k < hi; k++) sum += rawBands[k];
      const db = sum / (hi - lo);
      out[j] = clamp((db - MIN_DB) / (MAX_DB - MIN_DB), 0, 1);
    }
  }

  /** Lively synthetic spectrum so the visualizer animates in browser preview. */
  function mockTargets(n: number, out: Float32Array) {
    const t = performance.now() / 1000;
    const beat = 0.55 + 0.45 * Math.pow(Math.max(0, Math.sin(t * 2.4)), 3);
    for (let j = 0; j < n; j++) {
      const f = j / n;
      const tilt = 1 - f * 0.55; // bass-heavy
      const wobble = 0.5 + 0.5 * Math.sin(t * (1.5 + j * 0.35) + j * 0.7);
      out[j] = clamp(tilt * (0.35 + 0.65 * wobble) * beat, 0, 1);
    }
  }

  let targets: Float32Array = new Float32Array(0);

  function frame() {
    raf = requestAnimationFrame(frame);
    if (!ctx) return;
    resizeCanvas();
    const n = barCount();
    ensureArrays(n);
    if (targets.length !== n) targets = new Float32Array(n);

    if (isTauri) realTargets(n, targets);
    else mockTargets(n, targets);

    // Smooth levels + peak caps.
    for (let j = 0; j < n; j++) {
      const tg = targets[j];
      const cur = levels[j];
      levels[j] = cur + (tg - cur) * (tg > cur ? ATTACK : DECAY);
      if (levels[j] > peaks[j]) peaks[j] = levels[j];
      else peaks[j] = Math.max(0, peaks[j] - PEAK_GRAVITY);
    }

    draw(n);
  }

  function draw(n: number) {
    if (!ctx) return;
    ctx.clearRect(0, 0, cssW, cssH);

    const color = prefs.values.visualizer_color || "#ffaf32";
    const lit = color;
    const dim = hexToRgba(color, 0.11);
    const cap = hexToRgba(color, 0.42); // faint marker: brighter than unlit, fainter than lit

    const padTop = 8;
    const bottom = cssH - LABEL_H; // bars sit above the Hz-label strip
    const areaH = Math.max(0, bottom - padTop);
    const cellH = areaH / BLOCKS;
    const blockH = Math.max(1, cellH - V_GAP);
    const barW = cssW / n;
    const blockW = Math.max(1, barW - H_GAP);

    for (let j = 0; j < n; j++) {
      const x = j * barW + (barW - blockW) / 2;
      const litBlocks = Math.round(levels[j] * BLOCKS);
      const peakBlock = Math.round(peaks[j] * BLOCKS);
      for (let k = 0; k < BLOCKS; k++) {
        const y = bottom - (k + 1) * cellH + (cellH - blockH);
        if (k < litBlocks) ctx.fillStyle = lit;
        else if (k + 1 === peakBlock && peakBlock > litBlocks) ctx.fillStyle = cap;
        else ctx.fillStyle = dim;
        ctx.fillRect(x, y, blockW, blockH);
      }
    }

    drawFreqAxis(bottom);
  }

  function fmtHz(f: number): string {
    if (f < 1000) return `${f}`;
    const k = f / 1000;
    return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
  }

  /** Hz ticks under the bars, placed at the same log positions used for binning
      so a label sits exactly under the bars showing that frequency. */
  function drawFreqAxis(yTop: number) {
    if (!ctx) return;
    const N = rawBands.length || FFT_BANDS;
    const nyquist = sampleRate / 2;
    const fMin = (MIN_IDX * nyquist) / N;
    const denom = Math.log(N / MIN_IDX);
    ctx.font = "10px system-ui, sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    const labelY = yTop + LABEL_H / 2 + 1;
    let lastX = -Infinity;
    for (const f of FREQ_TICKS) {
      if (f <= fMin || f >= nyquist) continue;
      const idx = (f * N) / nyquist;
      const p = clamp(Math.log(idx / MIN_IDX) / denom, 0, 1);
      const x = p * cssW;
      if (x - lastX < 26) continue; // avoid crowding on narrow widths
      lastX = x;
      ctx.fillStyle = labelColor;
      ctx.globalAlpha = 0.5;
      ctx.fillRect(x - 0.5, yTop, 1, 3); // small tick
      ctx.globalAlpha = 1;
      ctx.fillText(fmtHz(f), clamp(x, 14, cssW - 14), labelY);
    }
  }

  onMount(() => {
    ctx = canvas.getContext("2d");
    labelColor = getComputedStyle(stage).getPropertyValue("--txt-faded").trim() || labelColor;
    resizeCanvas();
    raf = requestAnimationFrame(frame);

    if (isTauri) {
      call("player_set_spectrum_enabled", { on: true }).catch(() => {});
      listen<{ bands: number[]; rate: number }>("player:spectrum", (e) => {
        rawBands = e.payload.bands;
        if (e.payload.rate > 0) sampleRate = e.payload.rate;
        lastSpectrumAt = performance.now();
      }).then((un) => (unlisten = un));
    }
  });

  onDestroy(() => {
    cancelAnimationFrame(raf);
    unlisten?.();
    if (isTauri) call("player_set_spectrum_enabled", { on: false }).catch(() => {});
  });

  function onColor(hex: string) {
    prefs.set("visualizer_color", hex);
  }
</script>

<div class="vis" style:padding-bottom={`${ui.bottombarH}px`}>
  <div class="stage" bind:this={stage}>
    <canvas bind:this={canvas}></canvas>
    <div class="tools" class:revealed={pickerOpen}>
      <ColorPicker
        value={prefs.values.visualizer_color}
        onchange={onColor}
        onopenchange={(o) => (pickerOpen = o)}
        title="Visualizer color"
      />
    </div>
  </div>
</div>

<style>
  .vis {
    height: 100%;
    box-sizing: border-box;
  }
  .stage {
    position: relative;
    width: 100%;
    height: 100%;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }
  /* Hidden until the mouse enters this corner (padding enlarges the hover
     target), or while the picker is open. */
  .tools {
    position: absolute;
    top: 0;
    right: 0;
    padding: 12px 14px;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .tools:hover,
  .tools.revealed {
    opacity: 1;
  }
</style>
