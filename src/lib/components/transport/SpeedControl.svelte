<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { RATE_MAX, RATE_MIN } from "$lib/api";
  import SpriteButton from "../common/SpriteButton.svelte";

  let open = $state(false);
  let popEl: HTMLDivElement | null = $state(null);

  // The badge over the gauge says how far from normal speed the song plays:
  // one chevron up to a quarter either way, two beyond that.
  const overlay = $derived.by(() => {
    const r = player.rate;
    if (r < 0.75) return "chevrons-down";
    if (r < 1) return "chevron-down";
    if (r === 1) return undefined;
    if (r <= 1.5) return "chevron-up";
    return "chevrons-up";
  });

  const label = $derived(`${player.rate.toFixed(2)}×`);

  function setFromInput(e: Event) {
    const value = parseFloat((e.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(value)) void player.setRate(value);
  }

  function onWindowPointerDown(e: PointerEvent) {
    if (open && popEl && !popEl.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="speed" bind:this={popEl}>
  <SpriteButton
    icon="gauge"
    size={26}
    title={`Speed: ${label}`}
    {overlay}
    onclick={() => (open = !open)}
  />
  {#if open}
    <div class="popover">
      <input
        class="slider"
        type="range"
        min={RATE_MIN}
        max={RATE_MAX}
        step="0.05"
        value={player.rate}
        aria-label="Playback speed"
        oninput={setFromInput}
      />
      <div class="row">
        <input
          class="number"
          type="number"
          min={RATE_MIN}
          max={RATE_MAX}
          step="0.05"
          value={player.rate.toFixed(2)}
          aria-label="Playback speed"
          onchange={setFromInput}
        />
        <span class="unit">×</span>
        <button class="reset" disabled={player.rate === 1} onclick={() => player.setRate(1)}>
          Normal
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .speed {
    position: relative;
    display: flex;
  }
  .popover {
    position: absolute;
    bottom: 34px;
    left: 50%;
    transform: translateX(-50%);
    width: 170px;
    padding: 10px 12px 8px;
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 100;
  }
  .slider {
    width: 100%;
    margin: 0;
    accent-color: var(--txt-full);
    cursor: pointer;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .number {
    width: 54px;
    padding: 2px 4px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    color: var(--txt);
    font-size: 11px;
    text-align: right;
  }
  .unit {
    color: var(--txt-faded);
    font-size: 11px;
  }
  .reset {
    margin-left: auto;
    padding: 2px 8px;
    border: 1px solid var(--border);
    color: var(--txt);
    font-size: 11px;
  }
  .reset:hover:not(:disabled) {
    color: var(--txt-full);
    background: var(--hover-row);
  }
  .reset:disabled {
    color: var(--txt-faded);
    cursor: default;
  }
</style>
