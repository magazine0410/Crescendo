<script lang="ts" module>
  export interface MenuItem {
    label: string;
    action?: () => void;
    separator?: boolean;
    disabled?: boolean;
    /** When set, renders a checkbox-style item (leading ✓ when true). */
    checked?: boolean;
    /** When set, the item opens a nested submenu on hover instead of acting. */
    submenu?: MenuItem[];
  }

  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  export function openContextMenu(e: MouseEvent, items: MenuItem[]) {
    e.preventDefault();
    e.stopPropagation();
    menu = { x: e.clientX, y: e.clientY, items };
  }

  export function closeContextMenu() {
    menu = null;
  }
</script>

<script lang="ts">
  function pick(item: MenuItem) {
    if (item.disabled || item.separator || item.submenu) return;
    closeContextMenu();
    item.action?.();
  }

  function onWindowDown(e: PointerEvent) {
    if (menu && !(e.target as HTMLElement).closest(".ctx-menu")) closeContextMenu();
  }

  /** Gap kept between a menu and the window edges. */
  const MARGIN = 6;

  let rootEl = $state<HTMLElement | null>(null);

  // Put the root menu fully inside the window. The size is measured, not
  // guessed from the item count: separators and wrapped labels are not 26px.
  $effect(() => {
    const el = rootEl;
    const m = menu;
    if (!el || !m) return;
    // Measure from the corner. Left at the pointer, a menu opened near an edge
    // is squeezed into the space that is left there — it wraps, and reads both
    // narrower and taller than it will be drawn once moved, which is the very
    // measurement the move is decided from.
    el.style.left = "0px";
    el.style.top = "0px";
    const width = el.offsetWidth;
    const height = el.offsetHeight;
    const left =
      m.x + width > window.innerWidth - MARGIN ? Math.max(MARGIN, m.x - width) : m.x;
    // Too tall to open downwards: flip it above the pointer, the way it
    // already flips to the pointer's left. Clamping it to the bottom of the
    // window instead left it lying over whatever was clicked — the transport
    // controls, for a menu opened from the bottom bar.
    const top =
      m.y + height > window.innerHeight - MARGIN ? Math.max(MARGIN, m.y - height) : m.y;
    el.style.left = `${left}px`;
    el.style.top = `${top}px`;
  });

  /**
   * Position a submenu against the window instead of against its parent item.
   * The height follows the free space, and the box moves up when it does not
   * fit below the item. A fixed `max-height` (60vh) let the lower part of the
   * scroll box fall outside the window, and the entries in that part could
   * never be scrolled into view — the more so for an item near the bottom.
   */
  function placeSubmenu(el: HTMLElement) {
    const wrap = el.parentElement as HTMLElement;

    function place() {
      // Show and unclamp first, so the natural height can be measured. The
      // inline value must be "none": an empty value lets the stylesheet limit
      // (60vh) apply again, and the measurement stays clamped.
      el.style.display = "block";
      el.style.maxHeight = "none";
      const natural = el.offsetHeight;
      const wrapRect = wrap.getBoundingClientRect();

      const avail = window.innerHeight - MARGIN * 2;
      const h = Math.min(natural, avail);
      el.style.maxHeight = `${avail}px`;

      let top = wrapRect.top - 5;
      if (top + h > window.innerHeight - MARGIN) top = window.innerHeight - MARGIN - h;
      if (top < MARGIN) top = MARGIN;
      el.style.top = `${top - wrapRect.top}px`;

      // Open to the left only when the right side has no room and the left
      // side has. The width is measured, not a guess at the label lengths.
      const w = el.offsetWidth;
      const flip = wrapRect.right + w > window.innerWidth - MARGIN && wrapRect.left - w >= MARGIN;
      el.style.left = flip ? "auto" : "100%";
      el.style.right = flip ? "100%" : "auto";
      el.scrollTop = 0;
    }

    function hide() {
      el.style.display = "";
    }

    wrap.addEventListener("pointerenter", place);
    wrap.addEventListener("pointerleave", hide);
    return {
      destroy() {
        wrap.removeEventListener("pointerenter", place);
        wrap.removeEventListener("pointerleave", hide);
      },
    };
  }
</script>

<svelte:window onpointerdown={onWindowDown} onblur={closeContextMenu} />

{#if menu}
  <div class="ctx-menu" bind:this={rootEl} style:left={`${menu.x}px`} style:top={`${menu.y}px`}>
    {#each menu.items as item, i (i)}
      {@render renderItem(item)}
    {/each}
  </div>
{/if}

{#snippet renderItem(item: MenuItem)}
  {#if item.separator}
    <div class="sep"></div>
  {:else if item.submenu}
    <div class="item-wrap">
      <button class="item parent" class:disabled={item.disabled}>
        {item.label}
        <span class="arrow">›</span>
      </button>
      <div class="submenu" use:placeSubmenu>
        {#each item.submenu as sub, j (j)}
          {@render renderItem(sub)}
        {/each}
      </div>
    </div>
  {:else}
    <button
      class="item"
      class:checkable={item.checked !== undefined}
      class:disabled={item.disabled}
      onclick={() => pick(item)}
    >
      {#if item.checked !== undefined}
        <span class="check">{item.checked ? "✓" : ""}</span>
      {/if}
      <span class="label">{item.label}</span>
    </button>
  {/if}
{/snippet}

<style>
  .ctx-menu {
    position: fixed;
    z-index: 500;
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    padding: 4px 0;
    min-width: 180px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.6);
  }
  .item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 5px 14px;
    color: var(--txt);
  }
  .item:hover:not(.disabled) {
    background: var(--hover-row);
  }
  .item.disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  /* Tick in its own column, so a label that wraps keeps its later lines
     under the first one instead of running back beneath the tick. */
  .item.checkable {
    display: flex;
    align-items: flex-start;
  }
  .check {
    flex: none;
    width: 1em;
    margin-right: 2px;
    text-align: center;
  }
  .sep {
    height: 1px;
    margin: 4px 8px;
    background: var(--hairline);
  }
  /* submenu */
  .item-wrap {
    position: relative;
  }
  .item.parent {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .arrow {
    color: var(--txt-faded);
    font-size: 14px;
    line-height: 1;
  }
  /* `left`, `top` and `max-height` are set by placeSubmenu() on hover; the
     values here only apply until the first measurement. */
  .submenu {
    display: none;
    position: absolute;
    left: 100%;
    top: -5px;
    min-width: 160px;
    max-height: 60vh;
    overflow-y: auto;
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    padding: 4px 0;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.6);
  }
  .item-wrap:hover > .submenu {
    display: block;
  }
  .item-wrap:hover > .item.parent {
    background: var(--hover-row);
  }
</style>
