<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { waveform } from "$lib/api";

  let trackEl: HTMLDivElement | null = $state(null);
  let dragging = $state(false);
  let dragMs = $state(0);

  const shownMs = $derived(dragging ? dragMs : player.positionMs);
  const ratio = $derived(
    player.durationMs > 0 ? Math.min(1, Math.max(0, shownMs / player.durationMs)) : 0,
  );
  const enabled = $derived(player.track !== null && player.durationMs > 0);

  // --- Loudness waveform (faint backdrop behind the seek line) ---
  let canvasEl: HTMLCanvasElement | null = $state(null);
  let peaks = $state<number[]>([]);
  let cssW = $state(0);
  let cssH = $state(0);

  // Fetch peaks whenever the track changes or the feature is toggled on.
  $effect(() => {
    const id = player.track?.id;
    if (!prefs.values.show_waveform || id == null) {
      peaks = [];
      return;
    }
    let cancelled = false;
    waveform(id)
      .then((p) => {
        if (!cancelled) peaks = p;
      })
      .catch(() => {
        if (!cancelled) peaks = [];
      });
    return () => {
      cancelled = true;
    };
  });

  // Track the seek bar's rendered size so the canvas stays crisp on resize.
  $effect(() => {
    if (!canvasEl) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0].contentRect;
      cssW = r.width;
      cssH = r.height;
    });
    ro.observe(canvasEl);
    return () => ro.disconnect();
  });

  // Draw the waveform: played portion bright, unplayed faint. Cheap enough to
  // redraw fully on each position tick.
  $effect(() => {
    const el = canvasEl;
    const data = peaks;
    const r = ratio;
    const w = cssW;
    const h = cssH;
    if (!el || w === 0 || h === 0) return;
    const dpr = window.devicePixelRatio || 1;
    el.width = Math.round(w * dpr);
    el.height = Math.round(h * dpr);
    const ctx = el.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, el.width, el.height);
    if (data.length === 0) return;
    ctx.scale(dpr, dpr);
    const full = getComputedStyle(el).getPropertyValue("--txt-full").trim() || "#ffffff";
    ctx.fillStyle = full;
    const mid = h / 2;
    const n = data.length;
    const barW = w / n;
    for (let i = 0; i < n; i++) {
      const barH = Math.max(1, (data[i] / 255) * h);
      const played = (i + 0.5) / n <= r;
      ctx.globalAlpha = played ? 0.5 : 0.15;
      ctx.fillRect(i * barW, mid - barH / 2, Math.max(1, barW), barH);
    }
    ctx.globalAlpha = 1;
  });

  function msFromEvent(e: PointerEvent): number {
    if (!trackEl) return 0;
    const r = trackEl.getBoundingClientRect();
    const x = Math.min(r.right, Math.max(r.left, e.clientX)) - r.left;
    return (x / r.width) * player.durationMs;
  }

  function onPointerDown(e: PointerEvent) {
    if (!enabled) return;
    dragging = true;
    dragMs = msFromEvent(e);
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    if (dragging) dragMs = msFromEvent(e);
  }
  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    player.seek(msFromEvent(e));
  }
</script>

<div
  class="seek"
  class:enabled
  role="slider"
  aria-label="Seek"
  aria-valuemin={0}
  aria-valuemax={player.durationMs}
  aria-valuenow={Math.round(shownMs)}
  tabindex="-1"
  bind:this={trackEl}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
>
  {#if prefs.values.show_waveform && enabled}
    <canvas class="wave" bind:this={canvasEl}></canvas>
  {/if}
  <div class="line">
    <div class="fill" style:width={`${ratio * 100}%`}></div>
  </div>
  {#if enabled}
    <div class="handle" style:left={`${ratio * 100}%`}></div>
  {/if}
</div>

<style>
  .seek {
    position: relative;
    height: var(--transport-seek-height, 14px);
    display: flex;
    align-items: center;
    flex: 1;
    min-width: 60px;
  }
  .wave {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    z-index: 0;
    pointer-events: none;
  }
  .line {
    position: relative;
    z-index: 1;
    width: 100%;
    height: 2px;
    background: rgba(255, 255, 255, 0.22);
  }
  .fill {
    height: 100%;
    background: var(--txt-full);
  }
  .handle {
    position: absolute;
    top: 50%;
    z-index: 2;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--txt-full);
    transform: translate(-50%, -50%);
    pointer-events: none;
  }
  .seek.enabled:hover .line {
    height: 3px;
  }
  .seek:not(.enabled) {
    opacity: 0.4;
  }
</style>
