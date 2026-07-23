<script lang="ts">
  import { queue } from "$lib/stores/queue.svelte";
  import { formatTime, displayTitle, displayArtist } from "$lib/format";
</script>

<aside class="queue">
  <div class="header">
    <span>Queued files</span>
    {#if queue.tracks.length > 0}
      <button class="clear" onclick={() => queue.clear()}>Clear</button>
    {/if}
  </div>
  {#if queue.tracks.length === 0}
    <div class="empty">Queue is empty</div>
  {:else}
    <div class="items">
      {#each queue.tracks as t, i (i)}
        <div class="row">
          <span class="no">{i + 1}</span>
          <div class="text">
            <div class="title">{displayTitle(t)}</div>
            <div class="artist">{displayArtist(t) ?? "?"}</div>
          </div>
          <span class="dur">{formatTime(t.duration_ms)}</span>
          <button class="rm" title="Remove from queue" onclick={() => queue.removeAt([i])}>✕</button>
        </div>
      {/each}
    </div>
  {/if}
</aside>

<style>
  .queue {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border-left: 1px solid var(--sidesline);
    overflow: hidden;
  }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: var(--header-h);
    padding: 0 12px;
    color: var(--txt-faded);
    border-bottom: 1px solid var(--hairline);
    flex: none;
  }
  .clear {
    color: var(--txt-faded);
    font-size: 10px;
  }
  .clear:hover {
    color: var(--txt);
  }
  .empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--txt-superfaded);
  }
  .items {
    flex: 1;
    overflow-y: auto;
    padding-bottom: var(--bottombar-h);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 12px;
  }
  .row:hover {
    background: var(--hover-row);
  }
  .no {
    color: var(--txt-faded);
    font-style: italic;
    width: 18px;
    text-align: right;
    flex: none;
    font-size: 11px;
  }
  .text {
    min-width: 0;
    flex: 1;
  }
  .title {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .artist {
    color: var(--txt-faded);
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dur {
    color: var(--txt-faded);
    font-size: 11px;
    flex: none;
  }
  .rm {
    color: var(--txt-faded);
    opacity: 0;
    flex: none;
    padding: 2px 4px;
  }
  .row:hover .rm {
    opacity: 1;
  }
  .rm:hover {
    color: var(--txt);
  }
</style>
