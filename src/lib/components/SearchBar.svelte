<script lang="ts">
  import { ui, type SearchSpan } from "$lib/stores/ui.svelte";
  import { call } from "$lib/api";
  import Icon from "./common/Icon.svelte";

  let inputEl: HTMLInputElement | null = $state(null);
  let mirrorEl: HTMLDivElement | null = $state(null);

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
</script>

<div class="search">
  <span class="glass"><Icon name="search_icon_white" size={13} /></span>
  <div class="field">
    <input
      bind:this={inputEl}
      bind:value={ui.searchQuery}
      onscroll={syncScroll}
      onkeyup={syncScroll}
      onclick={syncScroll}
      type="text"
      placeholder="Search..."
      spellcheck="false"
      autocomplete="off"
    />
    <div class="mirror" bind:this={mirrorEl} aria-hidden="true">
      {#each pieces as piece, i (i)}<span class={piece.kind}>{piece.text}</span>{/each}
    </div>
  </div>
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
