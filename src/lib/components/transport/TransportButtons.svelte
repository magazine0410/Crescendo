<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import SpriteButton from "../common/SpriteButton.svelte";
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
  <SpriteButton icon="stop" size={26} title="Stop" onclick={() => player.stop()} />
</div>

<style>
  .transport {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
</style>
