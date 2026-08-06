<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import Icon from "../common/Icon.svelte";

  const noun = $derived(
    library.mode === "genre" ? "genres"
    : library.mode === "artist" ? "artists"
    : library.mode === "album" ? "albums"
    : library.mode === "albumartist" ? "album artists"
    : library.isGroupingMode ? "groups"
    : library.isDurationMode ? "durations" : "values",
  );
</script>

<div class="vfilter" class:active={library.valueFilter.length > 0}>
  <span class="funnel"><Icon name="value_filter" size={12} /></span>
  <input
    bind:value={library.valueFilter}
    type="text"
    placeholder="Filter {noun}…"
    spellcheck="false"
    autocomplete="off"
  />
  {#if library.valueFilter}
    <button class="clear" onclick={() => (library.valueFilter = "")} title="Clear filter">✕</button>
  {/if}
</div>

<style>
  .vfilter {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
    height: 26px;
    padding: 0 8px;
    border-bottom: 1px solid var(--hairline);
    border-top: 1px solid transparent;
  }
  .vfilter:focus-within,
  .vfilter.active {
    background: rgba(0, 0, 0, 0.25);
  }
  .funnel {
    display: flex;
    flex: none;
    color: var(--txt-faded);
  }
  .vfilter.active .funnel {
    color: var(--txt);
  }
  input {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    color: var(--txt);
  }
  input::placeholder {
    color: var(--txt-faded);
  }
  .clear {
    flex: none;
    color: var(--txt-faded);
    font-size: 10px;
    padding: 2px 4px;
  }
  .clear:hover {
    color: var(--txt);
  }
</style>
