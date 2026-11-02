<script lang="ts">
  import { ui, type SearchSpan } from "$lib/stores/ui.svelte";
  import { prefs, SEARCH_FAVORITES_MAX, SEARCH_HISTORY_MAX } from "$lib/stores/prefs.svelte";
  import { call } from "$lib/api";
  import Icon from "./common/Icon.svelte";

  let inputEl: HTMLInputElement | null = $state(null);
  let mirrorEl: HTMLDivElement | null = $state(null);
  let rootEl: HTMLDivElement | null = $state(null);
  let historyOpen = $state(false);
  let favoritesOpen = $state(false);

  export function focus() {
    inputEl?.focus();
  }

  // Classify the query (debounced) to tint the input term by term: plain free
  // text stays white, a term using search syntax turns green, and one that does
  // not parse turns red like Eole's keyboard-search does on a miss.
  $effect(() => {
    const q = ui.searchQuery;
    if (!q.trim()) {
      ui.searchSpans = [];
      return;
    }
    const t = setTimeout(async () => {
      try {
        ui.searchSpans = await call<SearchSpan[]>("search_highlight", { query: q });
      } catch {
        ui.searchSpans = [];
      }
    }, 200);
    return () => clearTimeout(t);
  });

  // The input paints its own text transparent; this list is what the mirror
  // above it draws instead. Offsets are UTF-8 byte offsets from the parser, so
  // the text is cut out of the encoded query — the library is full of titles
  // that are not ASCII.
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  let pieces = $derived.by(() => {
    const bytes = encoder.encode(ui.searchQuery);
    // Spans lag the text by one debounce, so clamp them to what is typed now
    // and never cut a character in half: the tail of a shortened query simply
    // renders plain until the next classification arrives.
    const stop = (at: number) => {
      let i = Math.min(at, bytes.length);
      while (i < bytes.length && (bytes[i] & 0xc0) === 0x80) i++;
      return i;
    };
    const out: { text: string; kind: SearchSpan["kind"] }[] = [];
    const push = (from: number, to: number, kind: SearchSpan["kind"]) => {
      if (to > from) out.push({ text: decoder.decode(bytes.slice(from, to)), kind });
    };
    let at = 0;
    for (const span of ui.searchSpans) {
      const start = stop(span.start);
      const end = stop(span.end);
      if (end <= at) continue;
      push(at, start, "plain");
      push(Math.max(at, start), end, span.kind);
      at = end;
    }
    push(at, bytes.length, "plain");
    return out;
  });

  // Keep the mirror on the same horizontal scroll as the input it covers.
  function syncScroll() {
    if (mirrorEl && inputEl) mirrorEl.scrollLeft = inputEl.scrollLeft;
  }
  $effect(() => {
    void pieces;
    requestAnimationFrame(syncScroll);
  });

  // ---- History: the last few searches, most recent first ----

  /** A search was run: put it at the top of the history. The search runs
      live while typing, so "run" means the query was settled on — Enter,
      leaving the field, clearing it, or picking a saved one. */
  function remember(query: string) {
    const q = query.trim();
    if (!q) return;
    const rest = prefs.values.search_history.filter((h) => h !== q);
    void prefs.set("search_history", [q, ...rest].slice(0, SEARCH_HISTORY_MAX));
  }

  function search(query: string) {
    ui.searchQuery = query;
    remember(query);
    historyOpen = false;
    favoritesOpen = false;
  }

  function clear() {
    remember(ui.searchQuery);
    ui.searchQuery = "";
  }

  function onBlur() {
    historyOpen = false;
    // Moving on to something else in the window ends the search; the window
    // itself losing focus mid-typing does not.
    if (document.hasFocus()) remember(ui.searchQuery);
  }

  // ---- Favorites: saved searches behind the star ----

  const current = $derived(ui.searchQuery.trim());
  const favorites = $derived(prefs.values.search_favorites);
  const isFavorite = $derived(current !== "" && favorites.includes(current));
  const favoritesFull = $derived(favorites.length >= SEARCH_FAVORITES_MAX);

  function saveFavorite() {
    if (!current || isFavorite || favoritesFull) return;
    void prefs.set("search_favorites", [...favorites, current]);
  }

  function removeFavorite(query: string) {
    void prefs.set("search_favorites", favorites.filter((f) => f !== query));
  }

  function onWindowPointerDown(e: PointerEvent) {
    if (rootEl && !rootEl.contains(e.target as Node)) favoritesOpen = false;
  }

  function onWindowKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") favoritesOpen = false;
  }

  // ---- Completion: a favorite that starts with what was typed ----

  // `stem` is the text the user typed, before any Tab completion: the
  // favorites offered are the ones starting with it, and Tab walks them in
  // turn. Any change to the query that a Tab did not make — typing, clearing,
  // picking a history entry — starts over from the new text.
  let stem = $state("");
  let candidateIndex = $state(0);
  let completed: string | null = null;
  $effect(() => {
    const q = ui.searchQuery;
    if (q === completed) return;
    completed = null;
    stem = q;
    candidateIndex = 0;
  });

  const candidates = $derived.by(() => {
    if (!prefs.values.search_autocomplete || !stem.trim()) return [];
    const lower = stem.toLowerCase();
    return favorites.filter((f) => f !== stem && f.toLowerCase().startsWith(lower));
  });

  // The rest of the offered favorite, drawn faded after the typed text. Empty
  // once a Tab has filled the favorite in, so cycling shows only the counter.
  const ghost = $derived.by(() => {
    const c = candidates[candidateIndex];
    const q = ui.searchQuery;
    return c && c.toLowerCase().startsWith(q.toLowerCase()) ? c.slice(q.length) : "";
  });

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Tab") {
      const n = candidates.length;
      if (n === 0) return;
      e.preventDefault();
      let i = Math.min(candidateIndex, n - 1);
      // Already showing this favorite in full: Tab moves on to the next one.
      if (ui.searchQuery === candidates[i]) i = (i + (e.shiftKey ? n - 1 : 1)) % n;
      candidateIndex = i;
      completed = candidates[i];
      ui.searchQuery = completed;
      historyOpen = false;
    } else if (e.key === "Enter") {
      remember(ui.searchQuery);
      historyOpen = false;
    } else if (e.key === "Escape") {
      historyOpen = false;
    }
  }
</script>

<svelte:window
  onpointerdown={favoritesOpen ? onWindowPointerDown : undefined}
  onkeydown={favoritesOpen ? onWindowKeydown : undefined}
/>

<div class="search" bind:this={rootEl}>
  <button
    class="fav"
    class:saved={isFavorite}
    title={isFavorite ? "Favorite searches (this search is saved)" : "Favorite searches"}
    aria-expanded={favoritesOpen}
    onclick={() => {
      favoritesOpen = !favoritesOpen;
      historyOpen = false;
    }}
  >
    <Icon name="star" size={13} />
  </button>
  <span class="glass"><Icon name="search_icon_white" size={13} /></span>
  <div class="field">
    <input
      bind:this={inputEl}
      bind:value={ui.searchQuery}
      onscroll={syncScroll}
      onkeyup={syncScroll}
      onclick={() => {
        syncScroll();
        historyOpen = true;
        favoritesOpen = false;
      }}
      oninput={() => (historyOpen = false)}
      onkeydown={onKeydown}
      onblur={onBlur}
      type="text"
      placeholder="Search..."
      spellcheck="false"
      autocomplete="off"
    />
    <div class="mirror" bind:this={mirrorEl} aria-hidden="true">
      {#each pieces as piece, i (i)}<span class={piece.kind}>{piece.text}</span>{/each}{#if ghost}<span class="ghost">{ghost}</span>{/if}
    </div>
  </div>
  {#if candidates.length > 1}
    <span class="choices" title="Tab switches between the matching favorites">
      {candidateIndex + 1}/{candidates.length}
    </span>
  {/if}
  {#if ui.searchQuery}
    <button class="clear" onclick={clear} title="Clear search">✕</button>
  {/if}
  <button class="help" title="Search syntax help" onclick={() => (ui.searchHelpOpen = true)}>?</button>

  {#if historyOpen && prefs.values.search_history.length > 0}
    <!-- Pressing inside must not blur the input, which would close this. -->
    <div class="popup" role="presentation" onmousedown={(e) => e.preventDefault()}>
      <div class="head">Recent searches</div>
      {#each prefs.values.search_history as q (q)}
        <button class="row" onclick={() => search(q)}>{q}</button>
      {/each}
    </div>
  {/if}

  {#if favoritesOpen}
    <div class="popup">
      <div class="head">
        <span>Favorites</span>
        <span>{favorites.length}/{SEARCH_FAVORITES_MAX}</span>
      </div>
      {#if current && !isFavorite}
        <button class="row save" disabled={favoritesFull} onclick={saveFavorite}>
          {favoritesFull ? "No room for another favorite" : "Save current search"}
        </button>
      {/if}
      {#if favorites.length === 0}
        <div class="empty">No favorites yet. Type a search and save it here.</div>
      {/if}
      <div class="list">
        {#each favorites as q (q)}
          <div class="entry">
            <button class="row" onclick={() => search(q)}>{q}</button>
            <button class="remove" title="Remove from favorites" onclick={() => removeFavorite(q)}>✕</button>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .search {
    position: relative;
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
  .fav {
    display: flex;
    flex: none;
    color: var(--txt-faded);
  }
  .fav:hover,
  .fav.saved {
    color: var(--txt);
  }
  .fav.saved :global(svg) {
    fill: currentColor;
  }
  .glass {
    display: flex;
    flex: none;
    color: var(--txt-faded);
  }
  .field {
    position: relative;
    flex: 1;
    min-width: 0;
  }
  input {
    display: block;
    width: 100%;
    /* The mirror over it draws the text, tinted per term; the input keeps the
       caret, the selection and native input handling (IME included). */
    color: transparent;
    caret-color: var(--txt);
  }
  input::placeholder {
    color: var(--txt-faded);
  }
  input::selection {
    background: var(--selection);
    color: transparent;
  }
  .mirror {
    position: absolute;
    inset: 0;
    display: block;
    overflow: hidden;
    white-space: pre;
    pointer-events: none;
    color: var(--txt);
  }
  .mirror .syntax {
    color: var(--search-valid);
  }
  .mirror .invalid {
    color: var(--search-invalid);
  }
  .mirror .ghost {
    color: var(--txt-faded);
  }
  .choices {
    flex: none;
    color: var(--txt-faded);
    font-size: 10px;
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

  /* Both lists hang under the field, growing leftwards past it for long
     queries: the field sits at the window's right edge. */
  .popup {
    position: absolute;
    top: calc(100% + 3px);
    right: 0;
    min-width: 100%;
    max-width: 480px;
    padding: 4px 0;
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.6);
    z-index: 200;
  }
  .head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 4px 10px 5px;
    font-size: 10px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--txt-faded);
    white-space: nowrap;
  }
  .row {
    display: block;
    width: 100%;
    padding: 5px 10px;
    text-align: left;
    color: var(--txt);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row:hover,
  .row:focus-visible {
    background: var(--hover-row);
    color: var(--txt-full);
  }
  .row.save {
    color: var(--txt-semifaded);
    font-style: italic;
  }
  .row.save:disabled {
    color: var(--txt-faded);
    background: none;
    cursor: default;
  }
  .empty {
    padding: 5px 10px 6px;
    color: var(--txt-faded);
    white-space: nowrap;
  }
  .list {
    max-height: 50vh;
    overflow-y: auto;
  }
  .entry {
    display: flex;
    align-items: center;
  }
  .entry .row {
    flex: 1;
    min-width: 0;
  }
  .remove {
    flex: none;
    padding: 4px 10px 4px 6px;
    color: var(--txt-faded);
    font-size: 10px;
  }
  .remove:hover {
    color: var(--txt);
  }
</style>
