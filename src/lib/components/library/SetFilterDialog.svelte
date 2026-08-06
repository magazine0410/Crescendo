<script lang="ts" module>
  // Singleton dialog: mounted once in +page.svelte, opened from anywhere (the
  // custom tab's right-click menu) via openSetFilter(), mirroring ContextMenu.
  let open = $state(false);
  export function openSetFilter() {
    open = true;
  }
</script>

<script lang="ts">
  import { library, parseCustomTag } from "$lib/stores/library.svelte";

  /** Tags offered in the dropdown — each resolves through the backend's
      browse_column whitelist, so every pick is guaranteed to work. Kept
      alphabetical; %year% and %date% both group by release year. genre / artist /
      album / albumartist are deliberately omitted (they're the four built-in tabs)
      but still work if typed in manually. */
  const TAGS = [
    "bitdepth", "bitrate", "codec", "date", "disc", "duration",
    "grouping", "rating", "samplerate", "track", "year",
  ];

  let value = $state("");
  let menuOpen = $state(false);
  let comboEl = $state<HTMLDivElement>();
  let inputEl = $state<HTMLInputElement>();

  // Seed the field with the current raw filter text each time the dialog opens,
  // and make sure the dropdown never lingers between opens.
  $effect(() => {
    if (open) value = library.customTag;
    else menuOpen = false;
  });

  const preview = $derived(parseCustomTag(value));
  // Highlight the current tag in the dropdown when the text names a known one.
  const selected = $derived(preview && TAGS.includes(preview) ? preview : "");

  function pick(tag: string) {
    value = `%${tag}%`;
    menuOpen = false;
    inputEl?.focus();
  }

  function toggleMenu() {
    menuOpen = !menuOpen;
    if (!menuOpen) inputEl?.focus();
  }

  // Close the dropdown on a pointer press anywhere outside the combo.
  $effect(() => {
    if (!menuOpen) return;
    const onDown = (e: PointerEvent) => {
      if (comboEl && !comboEl.contains(e.target as Node)) menuOpen = false;
    };
    window.addEventListener("pointerdown", onDown, true);
    return () => window.removeEventListener("pointerdown", onDown, true);
  });

  function focusSelect(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  function close() {
    open = false;
  }

  async function save() {
    await library.setCustomTag(value.trim());
    close();
  }

  // Window-level: Enter saves, Escape closes the dialog. The combo intercepts
  // Escape first when the dropdown is open (see onComboKeydown).
  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") close();
    else if (e.key === "Enter") save();
  }

  // On the input: Escape closes just the dropdown (not the dialog) when it's
  // open; ArrowDown opens it.
  function onComboKeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && menuOpen) {
      menuOpen = false;
      e.stopPropagation(); // keep the dialog open — just close the dropdown
      e.preventDefault();
    } else if (e.key === "ArrowDown" && !menuOpen) {
      menuOpen = true;
      e.preventDefault();
    }
  }
</script>

<svelte:window onkeydown={open ? onKeydown : undefined} />

{#if open}
  <div
    class="backdrop"
    role="presentation"
    onclick={(e) => e.target === e.currentTarget && close()}
  >
    <div class="dialog" role="dialog" aria-label="Set custom filter">
      <header>
        <span>Set filter</span>
        <button class="x" type="button" onclick={close}>✕</button>
      </header>
      <section>
        <label for="customtag">Tag to browse by</label>
        <div class="combo" class:open={menuOpen} bind:this={comboEl}>
          <input
            id="customtag"
            type="text"
            placeholder="e.g. %year%"
            spellcheck="false"
            autocomplete="off"
            bind:value
            bind:this={inputEl}
            onkeydown={onComboKeydown}
            use:focusSelect
          />
          <button
            class="caret"
            type="button"
            tabindex="-1"
            aria-label="Choose a tag"
            aria-expanded={menuOpen}
            onclick={toggleMenu}
          >
            <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor"
              stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="m6 9 6 6 6-6" />
            </svg>
          </button>
          {#if menuOpen}
            <ul class="menu" role="listbox">
              {#each TAGS as t (t)}
                <li>
                  <button
                    class="opt"
                    class:sel={selected === t}
                    type="button"
                    role="option"
                    aria-selected={selected === t}
                    onclick={() => pick(t)}
                  >
                    %{t}%
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
        <p class="hint">
          Pick a tag from the list, or type a single %tag%. The tab is renamed to the
          tag and lists each distinct value with its track count. Multiple tags, or a
          tag that isn't stored, show nothing.
        </p>
        <p class="preview">
          {#if value.trim() === ""}
            <span class="muted">Filter cleared — the tab resets to “custom”.</span>
          {:else if preview === null}
            <span class="bad">Not a single tag — the list will be empty.</span>
          {:else}
            Header: <b>{preview}</b>
          {/if}
        </p>
      </section>
      <footer>
        <button class="btn" type="button" onclick={close}>Cancel</button>
        <button class="btn primary" type="button" onclick={save}>Set filter</button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 300;
  }
  .dialog {
    width: 420px;
    max-width: 90vw;
    background: var(--bg-main);
    border: 1px solid var(--popup-border);
    box-shadow: 0 8px 40px rgba(0, 0, 0, 0.7);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 18px;
    border-bottom: 1px solid var(--hairline);
    font-weight: 600;
  }
  .x {
    color: var(--txt-faded);
  }
  .x:hover {
    color: var(--txt);
  }
  section {
    padding: 14px 18px;
  }
  label {
    display: block;
    color: var(--txt-faded);
    margin-bottom: 4px;
    font-size: 11px;
  }
  /* Combobox: a type-in field with a dropdown of the browsable tags. */
  .combo {
    position: relative;
    display: flex;
    align-items: stretch;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    margin-bottom: 8px;
  }
  .combo:focus-within {
    border-color: var(--txt-faded);
  }
  .combo input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    padding: 6px 8px;
    color: var(--txt);
  }
  .caret {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    color: var(--txt-faded);
    border-left: 1px solid var(--border);
  }
  .caret:hover {
    color: var(--txt);
  }
  .caret svg {
    transition: transform 0.12s ease;
  }
  .combo.open .caret svg {
    transform: rotate(180deg);
  }
  .menu {
    position: absolute;
    top: calc(100% + 2px);
    left: 0;
    right: 0;
    max-height: 220px;
    overflow-y: auto;
    background: var(--bg-main);
    border: 1px solid var(--popup-border);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.6);
    z-index: 10;
    padding: 3px 0;
  }
  .opt {
    display: block;
    width: 100%;
    text-align: left;
    padding: 5px 10px;
    color: var(--txt);
    font-size: 12px;
  }
  .opt:hover {
    background: var(--hover-row);
  }
  .opt.sel {
    color: var(--txt-full);
    background: var(--selection);
  }
  .hint {
    color: var(--txt-superfaded);
    font-size: 11px;
    line-height: 1.5;
  }
  .preview {
    margin-top: 8px;
    font-size: 11px;
    color: var(--txt);
    min-height: 1em;
  }
  .preview b {
    color: var(--txt-full);
  }
  .preview .muted {
    color: var(--txt-faded);
  }
  .preview .bad {
    color: var(--search-invalid);
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 18px;
    border-top: 1px solid var(--sidesline);
  }
  .btn {
    border: 1px solid var(--border);
    padding: 5px 12px;
    color: var(--txt);
    height: 30px;
  }
  .btn:hover {
    background: var(--hover-row);
  }
  .btn.primary {
    color: var(--txt-full);
    border-color: var(--txt-faded);
  }
</style>
