import {
  call,
  isTauri,
  type FileDragResult,
  type FileDragSource,
} from "$lib/api";
import { plugins } from "$lib/stores/plugins.svelte";

export const FILE_DRAG_LIMIT = 2_000;

export interface FileDragOptions {
  source: () => FileDragSource | null | Promise<FileDragSource | null>;
  disabled?: () => boolean;
}

const DRAG_THRESHOLD = 5;

function notify(message: string, error = false) {
  window.dispatchEvent(
    new CustomEvent("crescendo:file-drag-notice", { detail: { message, error } }),
  );
}

/** Svelte action that turns a primary-button pointer gesture into a native OS
    file drag. Pointer events are used instead of HTML drag-and-drop because
    Tauri reserves the latter for files entering the application. */
export function fileDrag(node: HTMLElement, initial: FileDragOptions) {
  let options = initial;
  let origin: { x: number; y: number; pointerId: number } | null = null;
  let pointerDown = false;
  let starting = false;
  let suppressClick = false;

  function clearMoveListener() {
    window.removeEventListener("pointermove", onPointerMove);
  }

  function clearListeners() {
    clearMoveListener();
    window.removeEventListener("pointerup", onPointerUp);
    window.removeEventListener("pointercancel", onPointerUp);
  }

  function onPointerDown(event: PointerEvent) {
    // A fresh intentional press means an earlier native drag produced no
    // synthetic click; do not eat the click belonging to this new gesture.
    suppressClick = false;
    if (!isTauri || !plugins.fileDragEnabled || event.button !== 0 || options.disabled?.()) return;
    const target = event.target instanceof Element ? event.target : null;
    const nestedControl = target?.closest("button, input, select, textarea, a[href]");
    if (nestedControl && nestedControl !== node) return;
    clearListeners();
    origin = { x: event.clientX, y: event.clientY, pointerId: event.pointerId };
    pointerDown = true;
    starting = false;
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp, { once: true });
    window.addEventListener("pointercancel", onPointerUp, { once: true });
  }

  function onPointerUp() {
    pointerDown = false;
    origin = null;
    node.classList.remove("file-drag-active");
    clearListeners();
  }

  async function onPointerMove(event: PointerEvent) {
    if (!origin || starting || event.pointerId !== origin.pointerId) return;
    if (
      Math.abs(event.clientX - origin.x) < DRAG_THRESHOLD &&
      Math.abs(event.clientY - origin.y) < DRAG_THRESHOLD
    )
      return;

    starting = true;
    suppressClick = true;
    node.classList.add("file-drag-active");
    // Keep the up/cancel listeners while an async virtual-list selection is
    // being resolved, so releasing early really does cancel the drag.
    clearMoveListener();
    try {
      const source = await options.source();
      // Resolving sparse virtual-list selections can require a few page reads.
      // Do not unexpectedly begin a drag after the user has already released.
      if (!source || !pointerDown) return;
      const result = await call<FileDragResult>("plugin_file_drag_start", { source });
      if (result.truncated) {
        notify(`Dragging the first ${FILE_DRAG_LIMIT.toLocaleString()} tracks`);
      }
    } catch (reason) {
      const message =
        typeof reason === "object" && reason && "message" in reason
          ? String((reason as { message: unknown }).message)
          : String(reason);
      notify(message, true);
    } finally {
      pointerDown = false;
      origin = null;
      clearListeners();
      node.classList.remove("file-drag-active");
      starting = false;
    }
  }

  function onClick(event: MouseEvent) {
    if (!suppressClick) return;
    suppressClick = false;
    event.preventDefault();
    event.stopImmediatePropagation();
  }

  node.classList.add("file-draggable");
  node.addEventListener("pointerdown", onPointerDown);
  node.addEventListener("click", onClick, true);

  return {
    update(next: FileDragOptions) {
      options = next;
    },
    destroy() {
      clearListeners();
      node.classList.remove("file-draggable", "file-drag-active");
      node.removeEventListener("pointerdown", onPointerDown);
      node.removeEventListener("click", onClick, true);
    },
  };
}

declare global {
  interface WindowEventMap {
    "crescendo:file-drag-notice": CustomEvent<{ message: string; error: boolean }>;
  }
}
