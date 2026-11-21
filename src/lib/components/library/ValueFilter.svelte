<script lang="ts">
  import { library, type Facet } from "$lib/stores/library.svelte";
  import Icon from "../common/Icon.svelte";

  let { facet }: { facet: Facet } = $props();
</script>

<div class="vfilter" class:active={facet.valueFilter.length > 0}>
  <span class="funnel"><Icon name="value_filter" size={12} /></span>
  <input
    bind:value={facet.valueFilter}
    type="text"
    placeholder="Filter {facet.noun}…"
    spellcheck="false"
    autocomplete="off"
    disabled={library.interactionBlocked}
  />
  {#if facet.valueFilter}
    <button
      class="clear"
      disabled={library.interactionBlocked}
      onclick={() => (facet.valueFilter = "")}
      title="Clear filter"
    >✕</button>
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
