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

  // Open submenus to the left when the menu sits in the right portion of the
  // window, so a wide playlist list never spills off-screen.
  const flip = $derived(menu != null && menu.x + 360 > window.innerWidth);
</script>

<svelte:window onpointerdown={onWindowDown} onblur={closeContextMenu} />

{#if menu}
  <div
    class="ctx-menu"
    style:left={`${Math.min(menu.x, window.innerWidth - 200)}px`}
    style:top={`${Math.min(menu.y, window.innerHeight - menu.items.length * 26 - 12)}px`}
  >
    {#each menu.items as item, i (i)}
      {@render renderItem(item)}
    {/each}
  </div>
{/if}

{#snippet renderItem(item: MenuItem)}
  {#if item.separator}
    <div class="sep"></div>
  {:else if item.submenu}
    <div class="item-wrap" class:flip>
      <button class="item parent" class:disabled={item.disabled}>
        {item.label}
        <span class="arrow">›</span>
      </button>
      <div class="submenu">
        {#each item.submenu as sub, j (j)}
          {@render renderItem(sub)}
        {/each}
      </div>
    </div>
  {:else}
    <button class="item" class:disabled={item.disabled} onclick={() => pick(item)}>
      {#if item.checked !== undefined}
        <span class="check">{item.checked ? "✓" : ""}</span>
      {/if}
      {item.label}
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
  .check {
    display: inline-block;
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
  .item-wrap.flip > .submenu {
    left: auto;
    right: 100%;
  }
  .item-wrap:hover > .submenu {
    display: block;
  }
  .item-wrap:hover > .item.parent {
    background: var(--hover-row);
  }
</style>
