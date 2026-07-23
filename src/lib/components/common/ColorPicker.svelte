<script lang="ts">
  import { clamp, hexToHsv, hsvToHex, type Hsv } from "$lib/color";

  interface Props {
    /** Current color as `#rrggbb`. */
    value: string;
    /** Called with a new `#rrggbb` on every change. */
    onchange: (hex: string) => void;
    /** Hex the reset button restores to. */
    resetTo?: string;
    title?: string;
    /** Notified whenever the popover opens or closes. */
    onopenchange?: (open: boolean) => void;
  }
  let {
    value,
    onchange,
    resetTo = "#ffaf32",
    title = "Pick color",
    onopenchange,
  }: Props = $props();

  let open = $state(false);
  function setOpen(v: boolean) {
    if (open === v) return;
    open = v;
    onopenchange?.(v);
  }
  let popEl: HTMLDivElement | null = $state(null);
  let svEl: HTMLDivElement | null = $state(null);
  let hueEl: HTMLDivElement | null = $state(null);

  // HSV is the source of truth while the picker is open, so hue/saturation stay
  // stable even as value passes through greys or black (where hex→HSV is
  // ambiguous). Seeded from `value` each time the popover opens.
  let h = $state(0);
  let s = $state(1);
  let v = $state(1);
  let hexText = $state(""); // seeded from `value` whenever the popover opens

  function seedFrom(hex: string) {
    const hsv = hexToHsv(hex);
    if (hsv) {
      h = hsv.h;
      s = hsv.s;
      v = hsv.v;
    }
    hexText = hex;
  }

  function toggle() {
    if (!open) seedFrom(value);
    setOpen(!open);
  }

  function emit() {
    const hex = hsvToHex({ h, s, v } as Hsv);
    hexText = hex;
    onchange(hex);
  }

  // --- SV square drag (x = saturation, y = value) ---
  let svDrag = false;
  function svFromEvent(e: PointerEvent) {
    if (!svEl) return;
    const r = svEl.getBoundingClientRect();
    s = clamp((e.clientX - r.left) / r.width, 0, 1);
    v = 1 - clamp((e.clientY - r.top) / r.height, 0, 1);
    emit();
  }
  function svDown(e: PointerEvent) {
    svDrag = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    svFromEvent(e);
  }
  function svMove(e: PointerEvent) {
    if (svDrag) svFromEvent(e);
  }
  function svUp() {
    svDrag = false;
  }

  // --- Hue bar drag ---
  let hueDrag = false;
  function hueFromEvent(e: PointerEvent) {
    if (!hueEl) return;
    const r = hueEl.getBoundingClientRect();
    h = clamp((e.clientX - r.left) / r.width, 0, 1) * 360;
    emit();
  }
  function hueDown(e: PointerEvent) {
    hueDrag = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    hueFromEvent(e);
  }
  function hueMove(e: PointerEvent) {
    if (hueDrag) hueFromEvent(e);
  }
  function hueUp() {
    hueDrag = false;
  }

  function onHexInput(e: Event) {
    const raw = (e.currentTarget as HTMLInputElement).value;
    hexText = raw;
    const hsv = hexToHsv(raw);
    if (hsv) {
      h = hsv.h;
      s = hsv.s;
      v = hsv.v;
      onchange(hsvToHex(hsv));
    }
  }

  function reset() {
    seedFrom(resetTo);
    onchange(resetTo);
  }

  function onWindowPointerDown(e: PointerEvent) {
    if (open && popEl && !popEl.contains(e.target as Node)) setOpen(false);
  }

  const hueColor = $derived(`hsl(${h} 100% 50%)`);
</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="picker" bind:this={popEl}>
  <button
    class="swatch"
    {title}
    style:background={value}
    onclick={toggle}
    aria-label={title}
  ></button>

  {#if open}
    <div class="popover">
      <div
        class="sv"
        role="slider"
        aria-label="Saturation and brightness"
        aria-valuenow={Math.round(s * 100)}
        tabindex="-1"
        bind:this={svEl}
        style:background={`linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, transparent), ${hueColor}`}
        onpointerdown={svDown}
        onpointermove={svMove}
        onpointerup={svUp}
      >
        <div class="sv-cursor" style:left={`${s * 100}%`} style:top={`${(1 - v) * 100}%`}></div>
      </div>

      <div
        class="hue"
        role="slider"
        aria-label="Hue"
        aria-valuemin={0}
        aria-valuemax={360}
        aria-valuenow={Math.round(h)}
        tabindex="-1"
        bind:this={hueEl}
        onpointerdown={hueDown}
        onpointermove={hueMove}
        onpointerup={hueUp}
      >
        <div class="hue-cursor" style:left={`${(h / 360) * 100}%`}></div>
      </div>

      <div class="foot">
        <span class="preview" style:background={value}></span>
        <input
          class="hex"
          type="text"
          spellcheck="false"
          value={hexText}
          oninput={onHexInput}
          aria-label="Hex color"
        />
        <button class="reset" title="Reset to default" onclick={reset}>Reset</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    display: inline-flex;
  }
  .swatch {
    width: 22px;
    height: 22px;
    border-radius: 3px;
    border: 1px solid var(--popup-border);
    cursor: pointer;
  }
  .swatch:hover {
    border-color: var(--txt-full);
  }
  .popover {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    width: 200px;
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    border-radius: 4px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    z-index: 200;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.5);
  }
  .sv {
    position: relative;
    width: 100%;
    height: 130px;
    border-radius: 3px;
    cursor: crosshair;
    touch-action: none;
  }
  .sv-cursor {
    position: absolute;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid #fff;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.5);
    transform: translate(-50%, -50%);
    pointer-events: none;
  }
  .hue {
    position: relative;
    width: 100%;
    height: 12px;
    border-radius: 3px;
    cursor: pointer;
    touch-action: none;
    background: linear-gradient(
      to right,
      #f00 0%,
      #ff0 17%,
      #0f0 33%,
      #0ff 50%,
      #00f 67%,
      #f0f 83%,
      #f00 100%
    );
  }
  .hue-cursor {
    position: absolute;
    top: 50%;
    width: 6px;
    height: 18px;
    border-radius: 2px;
    background: #fff;
    border: 1px solid rgba(0, 0, 0, 0.5);
    transform: translate(-50%, -50%);
    pointer-events: none;
  }
  .foot {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .preview {
    flex: none;
    width: 22px;
    height: 22px;
    border-radius: 3px;
    border: 1px solid var(--popup-border);
  }
  .hex {
    flex: 1;
    min-width: 0;
    background: var(--bg-main);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 4px 6px;
    color: var(--txt);
    font-size: 11px;
    font-family: inherit;
  }
  .hex:focus {
    outline: none;
    border-color: var(--txt-faded);
  }
  .reset {
    flex: none;
    background: var(--bg-main);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 4px 8px;
    color: var(--txt-faded);
    font-size: 11px;
    cursor: pointer;
  }
  .reset:hover {
    color: var(--txt);
    border-color: var(--txt-faded);
  }
</style>
