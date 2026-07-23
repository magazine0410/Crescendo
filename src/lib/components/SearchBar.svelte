<script lang="ts">
  import { ui } from "$lib/stores/ui.svelte";
  import { call } from "$lib/api";
  import Icon from "./common/Icon.svelte";

  let inputEl: HTMLInputElement | null = $state(null);

  export function focus() {
    inputEl?.focus();
  }

  // Classify the query (debounced) to tint the input: plain free text stays
  // white, a valid structured query turns green, an unparseable one turns red
  // like Eole's keyboard-search does on a miss.
  $effect(() => {
    const q = ui.searchQuery;
    if (!q.trim()) {
      ui.searchKind = "plain";
      return;
    }
    const t = setTimeout(async () => {
      try {
        ui.searchKind = await call<"plain" | "syntax" | "invalid">("search_classify", { query: q });
      } catch {
        ui.searchKind = "plain";
      }
    }, 200);
    return () => clearTimeout(t);
  });
</script>

<div class="search" class:invalid={ui.searchKind === "invalid"} class:syntax={ui.searchKind === "syntax"}>
  <span class="glass"><Icon name="search_icon_white" size={13} /></span>
  <input
    bind:this={inputEl}
    bind:value={ui.searchQuery}
    type="text"
    placeholder="Search..."
    spellcheck="false"
    autocomplete="off"
  />
  {#if ui.searchQuery}
    <button class="clear" onclick={() => (ui.searchQuery = "")} title="Clear search">✕</button>
  {/if}
  <button class="help" title="Search syntax help" onclick={() => (ui.searchHelpOpen = true)}>?</button>
</div>

<style>
  .search {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 250px;
    height: 26px;
    padding: 0 8px;
    border: 1px solid transparent;
    border-radius: 2px;
  }
  .search:focus-within {
    border-color: var(--border);
    background: rgba(0, 0, 0, 0.25);
  }
  .glass {
    display: flex;
    flex: none;
    color: var(--txt-faded);
  }
  input {
    flex: 1;
    min-width: 0;
    color: var(--txt);
  }
  input::placeholder {
    color: var(--txt-faded);
  }
  .invalid input {
    color: var(--search-invalid);
  }
  .syntax input {
    color: var(--search-valid);
  }
  .clear {
    color: var(--txt-faded);
    font-size: 10px;
    padding: 2px 4px;
  }
  .clear:hover {
    color: var(--txt);
  }
  .help {
    color: var(--txt-faded);
    border: 1px solid var(--border);
    border-radius: 50%;
    width: 16px;
    height: 16px;
    font-size: 10px;
    line-height: 1;
    flex: none;
  }
  .help:hover {
    color: var(--txt-full);
    border-color: var(--txt-faded);
  }
</style>
