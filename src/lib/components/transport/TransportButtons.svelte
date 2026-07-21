<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import SpriteButton from "../common/SpriteButton.svelte";
  import { openContextMenu } from "../common/ContextMenu.svelte";

  function stopMenu(e: MouseEvent) {
    openContextMenu(e, [
      {
        label: "Stop after this song",
        checked: player.stopAfterCurrent,
        action: () => player.toggleStopAfterCurrent(),
      },
    ]);
  }
</script>

<div class="transport">
  <SpriteButton icon="prev" size={30} title="Previous" onclick={() => player.prev()} />
  <SpriteButton
    icon={player.status === "playing" ? "pause" : "play"}
    size={34}
    title={player.status === "playing" ? "Pause" : "Play"}
    onclick={() => player.togglePause()}
  />
  <SpriteButton icon="next" size={30} title="Next" onclick={() => player.next()} />
  <SpriteButton
    icon="stop"
    size={26}
    title={player.stopAfterCurrent ? "Stop (stopping after this song)" : "Stop"}
    overlay={player.stopAfterCurrent ? "circle-pause" : undefined}
    onclick={() => player.stop()}
    oncontextmenu={stopMenu}
  />
</div>

<style>
  .transport {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
</style>
