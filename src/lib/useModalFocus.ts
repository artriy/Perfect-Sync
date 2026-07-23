import { useEffect, useRef, type RefObject } from "react";

const FOCUSABLE = [
  "button:not([disabled])",
  "[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

interface ModalEntry {
  token: symbol;
  root: HTMLElement;
  previousHadInert: boolean;
}

const MODAL_STACK: ModalEntry[] = [];

/** Keeps keyboard focus inside an open desktop modal and restores its opener. */
export function useModalFocus(
  open: boolean,
  container: RefObject<HTMLElement | null>,
  onClose?: () => void,
): void {
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    if (!open) return;

    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const root = container.current;
    if (!root) return;

    const token = Symbol("modal");
    const previousModal = MODAL_STACK.at(-1);
    const previousHadInert = previousModal?.root.hasAttribute("inert") ?? false;
    previousModal?.root.setAttribute("inert", "");
    const modal = { token, root, previousHadInert };
    MODAL_STACK.push(modal);
    const isolatedBackground: HTMLElement[] = [];
    let branch: HTMLElement = root;
    while (branch.parentElement) {
      const parent = branch.parentElement;
      for (const sibling of parent.children) {
        if (
          sibling !== branch &&
          sibling instanceof HTMLElement &&
          !sibling.hasAttribute("data-modal-exempt") &&
          !sibling.hasAttribute("inert")
        ) {
          sibling.setAttribute("inert", "");
          isolatedBackground.push(sibling);
        }
      }
      if (parent === document.body) break;
      branch = parent;
    }
    const hadTabIndex = root.hasAttribute("tabindex");
    if (!hadTabIndex) root.tabIndex = -1;

    const initial =
      root.querySelector<HTMLElement>("[data-autofocus]") ??
      root.querySelector<HTMLElement>(FOCUSABLE) ??
      root;
    const focusFrame = window.requestAnimationFrame(() => {
      if (MODAL_STACK.at(-1)?.token === token && !root.contains(document.activeElement)) initial.focus();
    });
    initial.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (MODAL_STACK.at(-1)?.token !== token) return;
      if (event.key === "Escape" && closeRef.current) {
        event.preventDefault();
        event.stopImmediatePropagation();
        closeRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const items = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE));
      if (items.length === 0) {
        event.preventDefault();
        root.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      window.cancelAnimationFrame(focusFrame);
      const index = MODAL_STACK.indexOf(modal);
      const wasTop = index === MODAL_STACK.length - 1;
      if (index >= 0) MODAL_STACK.splice(index, 1);
      const nextModal = wasTop ? MODAL_STACK.at(-1) : undefined;
      if (nextModal && !previousHadInert) nextModal.root.removeAttribute("inert");
      for (const element of isolatedBackground) element.removeAttribute("inert");
      if (!hadTabIndex) root.removeAttribute("tabindex");
      if (wasTop) {
        const previousCanReceiveFocus =
          previous?.isConnected &&
          previous !== document.body &&
          previous !== document.documentElement &&
          !previous.matches(":disabled") &&
          !previous.closest("[inert]");
        const restoreTarget = previousCanReceiveFocus
          ? previous
          : (nextModal?.root.querySelector<HTMLElement>("[data-autofocus]") ??
            nextModal?.root.querySelector<HTMLElement>(FOCUSABLE) ??
            nextModal?.root);
        restoreTarget?.focus();
      }
    };
  }, [container, open]);
}
