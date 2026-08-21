<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { formatTime, formatTitle } from "$lib/format";
  import CoverThumb from "./CoverThumb.svelte";
  import TransportButtons from "./TransportButtons.svelte";
  import SeekBar from "./SeekBar.svelte";
  import VolumeControl from "./VolumeControl.svelte";
  import SpriteButton from "../common/SpriteButton.svelte";

  const titleLine = $derived(formatTitle(prefs.values.titlebar_format, player.track));
  const titleLine2 = $derived(formatTitle(prefs.values.titlebar_format_2, player.track));
  const useSecondTitle = $derived(!!player.track && prefs.values.titlebar_format_2.trim() !== "");

  // The upper half ends where the seek row begins. At 84px it can hold the
  // padded primary line, the smaller second line, and clearance above seek.
  const showSecondTitle = $derived(useSecondTitle && !!titleLine2 && ui.bottombarH >= 84);

  const repeatIcon = $derived(
    player.repeat === "off" ? "repeat" : player.repeat === "all" ? "repeat_all" : "repeat_track",
  );

  // Keep the default 76px bar at a 17.5px seek area, then scale with the bar
  // while retaining practical bounds at its minimum and maximum heights.
  const seekHeight = $derived(
    Math.min(37.5, Math.max(15, (ui.bottombarH / 76) * 17.5)),
  );

  /* drag the top edge to resize the bar */
  let resizing = false;
  function onGripDown(e: PointerEvent) {
    resizing = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onGripMove(e: PointerEvent) {
    if (!resizing) return;
    const h = window.innerHeight - e.clientY;
    ui.bottombarH = Math.min(160, Math.max(64, h));
  }
  function onGripUp() {
    if (!resizing) return;
    resizing = false;
    prefs.setLayout("bottombarH", ui.bottombarH);
  }
</script>

<footer
  class="bottombar"
  style:height={`${ui.bottombarH}px`}
  style:--transport-seek-height={`${seekHeight}px`}
>
  <div
    class="grip"
    role="separator"
    aria-orientation="horizontal"
    onpointerdown={onGripDown}
    onpointermove={onGripMove}
    onpointerup={onGripUp}
  ></div>
  <CoverThumb />
  <div class="controls-pad">
    <TransportButtons />
  </div>
  <div class="center">
    <div class="titleblock" class:with-second={useSecondTitle}>
      <div class="title" class:placeholder={!player.track} class:error={!!player.error}>
        {player.error ? `Playback unavailable: ${player.error}` : player.track ? titleLine : "Nothing played"}
      </div>
      {#if showSecondTitle}
        <div class="title2">{titleLine2}</div>
      {/if}
    </div>
    <div class="seekrow">
      <button
        class="time"
        title={ui.timeRemaining ? "Show elapsed" : "Show remaining"}
        onclick={() => (ui.timeRemaining = !ui.timeRemaining)}
      >
        {ui.timeRemaining
          ? formatTime(player.positionMs - player.durationMs)
          : formatTime(player.positionMs)}
      </button>
      <SeekBar />
      <span class="time">{formatTime(player.durationMs)}</span>
    </div>
  </div>
  <div class="right">
    <VolumeControl />
    <span class="toggle" class:on={player.repeat !== "off"}>
      <SpriteButton
        icon={repeatIcon}
        size={26}
        title={`Repeat: ${player.repeat}`}
        onclick={() => player.cycleRepeat()}
      />
    </span>
    <!-- shuffle stays full-opacity in both states; the icon shape shows the mode -->
    <span class="toggle on">
      <SpriteButton
        icon={player.shuffle ? "shuffle" : "shuffle_off"}
        size={26}
        title={player.shuffle ? "Shuffle: on" : "Shuffle: off"}
        onclick={() => player.setShuffle(!player.shuffle)}
      />
    </span>
  </div>
</footer>

<style>
  .bottombar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    background: var(--bg-headerbar);
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
    border-top: 1px solid var(--hairline);
    z-index: 50;
  }
  .grip {
    position: absolute;
    top: -3px;
    left: 0;
    right: 0;
    height: 6px;
    cursor: ns-resize;
    z-index: 60;
  }
  .controls-pad {
    padding: 0 10px 0 14px;
  }
  .center {
    position: relative;
    align-self: stretch;
    flex: 1;
    min-width: 0;
  }
  .titleblock {
    position: absolute;
    top: 25%;
    left: 14px;
    right: 14px;
    transform: translateY(-50%);
    text-align: center;
    min-width: 0;
  }
  .titleblock.with-second {
    top: 8px;
    transform: none;
  }
  .title {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 350;
    color: var(--txt);
  }
  .title.placeholder {
    color: var(--txt-faded);
  }
  .title.error {
    color: #d97777;
  }
  .title2 {
    margin-top: 2px;
    color: var(--txt-faded);
    font-size: 10px;
    line-height: 1.1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .seekrow {
    position: absolute;
    top: 50%;
    left: 14px;
    right: 14px;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .time {
    color: var(--txt-faded);
    font-size: 11px;
    min-width: 34px;
    text-align: center;
    flex: none;
  }
  .right {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 16px 0 6px;
    flex: none;
  }
  .toggle {
    display: flex;
    opacity: 0.55;
  }
  .toggle.on {
    opacity: 1;
  }
</style>
