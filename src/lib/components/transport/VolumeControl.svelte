<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import SpriteButton from "../common/SpriteButton.svelte";

  let open = $state(false);
  let popEl: HTMLDivElement | null = $state(null);
  let sliderEl: HTMLDivElement | null = $state(null);
  let dragging = false;

  const icon = $derived.by(() => {
    if (player.muted || player.volume === 0) return "mute";
    if (player.volume < 0.25) return "volume1";
    if (player.volume < 0.5) return "volume2";
    if (player.volume < 0.75) return "volume3";
    return "volume4";
  });

  // Value shown above the icon while scrolling; hidden ~1 s after the last wheel step.
  let wheelTip = $state(false);
  let wheelTipTimer: ReturnType<typeof setTimeout> | undefined;

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const step = e.deltaY < 0 ? 0.05 : -0.05;
    player.setVolume(player.volume + step);
    // The open popover already shows the number.
    if (open) return;
    wheelTip = true;
    clearTimeout(wheelTipTimer);
    wheelTipTimer = setTimeout(() => (wheelTip = false), 1000);
  }

  $effect(() => () => clearTimeout(wheelTipTimer));

  function toggleMute(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    player.toggleMute();
  }

  function volFromEvent(e: PointerEvent): number {
    if (!sliderEl) return player.volume;
    const r = sliderEl.getBoundingClientRect();
    const y = Math.min(r.bottom, Math.max(r.top, e.clientY));
    return 1 - (y - r.top) / r.height;
  }

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    player.setVolume(volFromEvent(e));
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    if (dragging) player.setVolume(volFromEvent(e));
  }
  function onPointerUp() {
    dragging = false;
  }

  function onWindowPointerDown(e: PointerEvent) {
    if (open && popEl && !popEl.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="volume" bind:this={popEl}>
  <SpriteButton
    {icon}
    size={26}
    title={`Volume (right-click to ${player.muted ? "unmute" : "mute"})`}
    onclick={() => (open = !open)}
    onwheel={onWheel}
    oncontextmenu={toggleMute}
  />
  {#if open}
    <div class="popover" onwheel={onWheel}>
      <div
        class="slider"
        role="slider"
        aria-label="Volume"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(player.volume * 100)}
        tabindex="-1"
        bind:this={sliderEl}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
      >
        <div class="track">
          <div class="fill" style:height={`${(player.muted ? 0 : player.volume) * 100}%`}></div>
        </div>
        <div
          class="handle"
          style:bottom={`${(player.muted ? 0 : player.volume) * 100}%`}
        ></div>
      </div>
      <div class="pct">{Math.round((player.muted ? 0 : player.volume) * 100)}</div>
    </div>
  {/if}
  {#if wheelTip && !open}
    <div class="wheel-tip">
      {Math.round((player.muted ? 0 : player.volume) * 100)}
    </div>
  {/if}
</div>

<style>
  .volume {
    position: relative;
    display: flex;
  }
  .popover {
    position: absolute;
    bottom: 34px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    padding: 10px 0 6px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    z-index: 100;
  }
  .slider {
    position: relative;
    height: 110px;
    width: 28px;
    display: flex;
    justify-content: center;
    cursor: pointer;
  }
  .track {
    width: 3px;
    height: 100%;
    background: rgba(255, 255, 255, 0.22);
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
  }
  .fill {
    width: 100%;
    background: var(--txt-full);
  }
  .handle {
    position: absolute;
    left: 50%;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--txt-full);
    transform: translate(-50%, 50%);
    pointer-events: none;
  }
  .pct {
    color: var(--txt-faded);
    font-size: 10px;
  }
  .wheel-tip {
    position: absolute;
    bottom: calc(100% + 4px);
    left: 50%;
    transform: translateX(-50%);
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    color: var(--txt-full);
    font-size: 11px;
    padding: 2px 6px;
    pointer-events: none;
    white-space: nowrap;
    z-index: 100;
  }
</style>
