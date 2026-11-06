<script lang="ts" module>
  export interface SelectOption {
    value: string;
    label: string;
  }
</script>

<script lang="ts">
  // Stands in for <select>, whose popup GTK draws outside the page, where CSS
  // cannot animate or theme it. Style it through the --select-* properties, which
  // a parent can set on any ancestor.
  import { dropdownTransition } from "$lib/motion";

  interface Props {
    /** Lets a `<label for>` name this control. */
    id?: string;
    value: string;
    options: SelectOption[];
    disabled?: boolean;
    title?: string;
    ariaLabel?: string;
    /** Called only when the choice differs from `value`. */
    onchange: (value: string) => void;
  }
  let { id, value, options, disabled = false, title, ariaLabel, onchange }: Props = $props();

  const listId = $props.id();
  /** Gap between the button and the list, and kept to the window edges. */
  const GAP = 1;
  const MARGIN = 6;

  let open = $state(false);
  /** The highlighted option while the list is open. */
  let active = $state(0);
  let buttonEl = $state<HTMLButtonElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);

  const selectedIndex = $derived(options.findIndex((o) => o.value === value));

  function show() {
    active = Math.max(0, selectedIndex);
    open = true;
  }

  function choose(index: number) {
    open = false;
    const next = options[index];
    if (next && next.value !== value) onchange(next.value);
  }

  function reveal(index: number) {
    listEl?.children[index]?.scrollIntoView({ block: "nearest" });
  }

  function move(index: number) {
    active = Math.max(0, Math.min(options.length - 1, index));
    reveal(active);
  }

  function onKeydown(e: KeyboardEvent) {
    const last = options.length - 1;
    if (!open) {
      // As a native select does here: the arrows change the value directly.
      // Space and Enter reach the button's own click.
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const step = e.key === "ArrowDown" ? 1 : -1;
        choose(Math.max(0, Math.min(last, selectedIndex + step)));
      }
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        move(active + 1);
        break;
      case "ArrowUp":
        move(active - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(last);
        break;
      case "Enter":
      case " ":
        choose(active);
        break;
      case "Escape":
        // Closes only the list, not a dialog around it.
        e.stopPropagation();
        open = false;
        break;
      case "Tab":
        open = false;
        return;
      default:
        return;
    }
    e.preventDefault();
  }

  $effect(() => {
    if (disabled) open = false;
  });

  /** Move a node out of any scroll box or stacking context around the button.
      Svelte removes a block's DOM from where it was created, so it would leave
      the node behind if the view around the button goes first. */
  function toBody(el: HTMLElement) {
    document.body.append(el);
    return { destroy: () => el.remove() };
  }

  /**
   * Place the list under the button. It opens above only when it does not fit
   * below and there is more room above; anchored by `bottom` there, it then grows
   * upwards. Must run before the transition measures the final height.
   */
  function placeList(el: HTMLElement) {
    const portal = toBody(el);
    const button = buttonEl!;
    const rect = button.getBoundingClientRect();
    el.style.fontSize = getComputedStyle(button).fontSize;
    el.style.minWidth = `${rect.width}px`;

    const below = window.innerHeight - rect.bottom - GAP - MARGIN;
    const above = rect.top - GAP - MARGIN;
    const up = el.offsetHeight > below && above > below;
    el.style.maxHeight = `${up ? above : below}px`;
    if (up) el.style.bottom = `${window.innerHeight - rect.top + GAP}px`;
    else el.style.top = `${rect.bottom + GAP}px`;
    el.style.left = `${Math.max(MARGIN, Math.min(rect.left, window.innerWidth - MARGIN - el.offsetWidth))}px`;
    reveal(active);
    return portal;
  }
</script>

<svelte:window
  onblur={() => (open = false)}
  onresize={() => (open = false)}
/>

<button
  {id}
  class="select"
  type="button"
  role="combobox"
  aria-haspopup="listbox"
  aria-expanded={open}
  aria-controls={listId}
  aria-activedescendant={open ? `${listId}-${active}` : undefined}
  aria-label={ariaLabel}
  {title}
  {disabled}
  bind:this={buttonEl}
  onclick={() => (open ? (open = false) : show())}
  onkeydown={onKeydown}
>
  <span class="current">{options[selectedIndex]?.label ?? ""}</span>
  <!-- Keep the button as wide as its longest option, as a native select is, so
       that a narrow one does not resize when the choice changes. -->
  {#each options as option (option.value)}
    <span class="sizer" aria-hidden="true">{option.label}</span>
  {/each}
</button>

{#if open}
  <!-- As with a native popup, a press outside only closes the list: it does not
       reach what is underneath, such as a dialog's backdrop, and neither does the
       wheel. In a block of its own, so it goes before the list's collapse ends. -->
  <div class="catcher" role="presentation" use:toBody onpointerdown={() => (open = false)}></div>
{/if}

{#if open}
  <!-- Pressing inside must not move focus: the keys stay with the button. -->
  <div
    id={listId}
    class="list"
    role="listbox"
    tabindex="-1"
    bind:this={listEl}
    onmousedown={(e) => e.preventDefault()}
    use:placeList
    transition:dropdownTransition
  >
    {#each options as option, i (option.value)}
      <button
        id={`${listId}-${i}`}
        class="option"
        class:active={i === active}
        class:selected={i === selectedIndex}
        type="button"
        role="option"
        tabindex="-1"
        aria-selected={i === selectedIndex}
        onpointermove={() => (active = i)}
        onclick={() => choose(i)}
      >
        {option.label}
      </button>
    {/each}
  </div>
{/if}

<style>
  .select {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    align-items: center;
    flex: none;
    width: var(--select-width, 100%);
    height: var(--select-height, auto);
    margin: var(--select-margin, 0);
    padding: var(--select-padding, 6px 26px 6px 8px);
    font-size: var(--select-font-size, inherit);
    text-align: left;
    color: var(--txt);
    background-color: var(--bg-panel);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%236e6e6e' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right var(--select-arrow-inset, 8px) center;
    background-size: var(--select-arrow-size, 12px);
    border: 1px solid var(--border);
    border-radius: var(--select-radius, 0);
    cursor: pointer;
  }
  .select:focus-visible {
    border-color: var(--txt-faded);
  }
  .select:disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  .current,
  .sizer {
    grid-area: 1 / 1;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .sizer {
    visibility: hidden;
    height: 0;
  }
  .catcher {
    position: fixed;
    inset: 0;
    z-index: 499;
  }
  .list {
    position: fixed;
    z-index: 500;
    box-sizing: border-box;
    width: max-content;
    max-width: calc(100vw - 12px);
    overflow-y: auto;
    padding: 3px 0;
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.6);
  }
  .option {
    display: block;
    width: 100%;
    padding: 5px 8px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    text-align: left;
    color: var(--txt);
  }
  .option.active {
    background: var(--hover-row);
  }
  .option.selected {
    color: var(--txt-full);
    font-weight: 600;
  }
</style>
