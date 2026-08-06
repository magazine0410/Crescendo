<script lang="ts" module>
  // Singleton dialog: mounted once in +page.svelte, opened from the playlist
  // sidebar's right-click menu, mirroring SetFilterDialog.
  let open = $state(false);
  let targets = $state<number[]>([]);

  /** Ask for a collection name for these playlists. */
  export function openSetCollection(playlistIds: number[]) {
    targets = playlistIds;
    open = true;
  }
</script>

<script lang="ts">
  import { playlists } from "$lib/stores/playlists.svelte";

  let value = $state("");
  let inputEl = $state<HTMLInputElement>();

  // Seed with the collection the playlists are already in, when they agree on
  // one, so re-opening the dialog edits rather than starts from blank.
  $effect(() => {
    if (!open) return;
    const ids = new Set(
      targets.map((id) => playlists.list.find((p) => p.id === id)?.collection_id ?? null),
    );
    const only = ids.size === 1 ? [...ids][0] : null;
    value = only == null ? "" : (playlists.collections.find((c) => c.id === only)?.name ?? "");
  });

  const names = $derived(playlists.collections.map((c) => c.name));
  const count = $derived(targets.length);

  function focusSelect(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  function close() {
    open = false;
  }

  async function save() {
    await playlists.setCollection(targets, value.trim());
    close();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") close();
    else if (e.key === "Enter") save();
  }
</script>

<svelte:window onkeydown={open ? onKeydown : undefined} />

{#if open}
  <div
    class="backdrop"
    role="presentation"
    onclick={(e) => e.target === e.currentTarget && close()}
  >
    <div class="dialog" role="dialog" aria-label="Set collection">
      <header>
        <span>Set collection</span>
        <button class="x" type="button" onclick={close}>✕</button>
      </header>
      <section>
        <label for="collname">
          Collection for {count === 1 ? "this playlist" : `these ${count} playlists`}
        </label>
        <input
          id="collname"
          type="text"
          placeholder="e.g. Video games"
          spellcheck="false"
          autocomplete="off"
          list="collection-names"
          bind:value
          bind:this={inputEl}
          use:focusSelect
        />
        <datalist id="collection-names">
          {#each names as n (n)}<option value={n}></option>{/each}
        </datalist>
        <p class="hint">
          An existing name adds them to that collection; a new one creates it.
          Leave it empty to take them out of their collection.
        </p>
        {#if names.length > 0}
          <div class="existing">
            {#each names as n (n)}
              <button type="button" class="chip" onclick={() => { value = n; inputEl?.focus(); }}>
                {n}
              </button>
            {/each}
          </div>
        {/if}
      </section>
      <footer>
        <button type="button" class="btn" onclick={close}>Cancel</button>
        <button type="button" class="btn primary" onclick={save}>Save</button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 60;
  }
  .dialog {
    width: 380px;
    max-width: 90vw;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--hairline);
    color: var(--txt-full);
  }
  .x {
    color: var(--txt-faded);
    font-size: 11px;
  }
  .x:hover {
    color: var(--txt);
  }
  section {
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  label {
    color: var(--txt-faded);
    font-size: 11px;
  }
  input {
    width: 100%;
    padding: 6px 8px;
    border: 1px solid var(--border);
    background: var(--bg-main);
    color: var(--txt);
  }
  input:focus {
    border-color: var(--selection);
  }
  .hint {
    color: var(--txt-faded);
    font-size: 10px;
    margin: 0;
  }
  .existing {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 2px;
  }
  .chip {
    padding: 3px 8px;
    border: 1px solid var(--hairline);
    color: var(--txt-faded);
    font-size: 11px;
  }
  .chip:hover {
    color: var(--txt-full);
    border-color: var(--selection);
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 10px 12px;
    border-top: 1px solid var(--hairline);
  }
  .btn {
    padding: 5px 12px;
    border: 1px solid var(--border);
    color: var(--txt);
  }
  .btn:hover {
    background: var(--hover-row);
  }
  .btn.primary {
    border-color: var(--selection);
    color: var(--txt-full);
  }
</style>
