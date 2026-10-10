<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { waveform } from "$lib/api";
  import { formatTime } from "$lib/format";

  let trackEl: HTMLDivElement | null = $state(null);
  let dragging = $state(false);
  let dragMs = $state(0);
  let hoverRatio = $state<number | null>(null);
  let tooltipWidth = $state(0);

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
    void prefs.values.color_txt_full; // redraw when Settings > Appearance changes the token
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
    if (r.width <= 0) return 0;
    const x = Math.min(r.right, Math.max(r.left, e.clientX)) - r.left;
    return (x / r.width) * player.durationMs;
  }

  function onPointerDown(e: PointerEvent) {
    if (!enabled) return;
    dragging = true;
    onPointerMove(e);
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    const ms = msFromEvent(e);
    if (dragging) dragMs = ms;
    const r = trackEl?.getBoundingClientRect();
    // Captured drag events can arrive outside the bar; only preview on hover.
    hoverRatio = enabled && r &&
      e.clientX >= r.left && e.clientX <= r.right &&
      e.clientY >= r.top && e.clientY <= r.bottom
      ? ms / player.durationMs
      : null;
  }
  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    player.seek(msFromEvent(e));
  }
  function onPointerCancel() {
    dragging = false;
    hoverRatio = null;
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
  onpointerenter={onPointerMove}
  onpointermove={onPointerMove}
  onpointerleave={() => hoverRatio = null}
  onpointerup={onPointerUp}
  onpointercancel={onPointerCancel}
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
  {#if enabled && hoverRatio !== null}
    <div class="hover-marker" style:left={`${hoverRatio * 100}%`} aria-hidden="true"></div>
    <div
      class="hover-time"
      bind:offsetWidth={tooltipWidth}
      style:left={`clamp(${tooltipWidth / 2}px, ${hoverRatio * 100}%, calc(100% - ${tooltipWidth / 2}px))`}
      aria-hidden="true"
    >{formatTime(hoverRatio * player.durationMs)}</div>
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
  .hover-marker {
    position: absolute;
    top: 50%;
    z-index: 3;
    width: 1px;
    height: 11px;
    background: var(--txt-full);
    transform: translate(-50%, -50%);
    pointer-events: none;
  }
  .hover-time {
    position: absolute;
    bottom: calc(50% + 9px);
    z-index: 3;
    padding: 2px 5px;
    border: 1px solid var(--popup-border);
    border-radius: 3px;
    background: var(--bg-popup);
    color: var(--txt-full);
    font-size: 11px;
    line-height: 1.2;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    transform: translateX(-50%);
    pointer-events: none;
  }
  .seek:not(.enabled) {
    opacity: 0.4;
  }
</style>
