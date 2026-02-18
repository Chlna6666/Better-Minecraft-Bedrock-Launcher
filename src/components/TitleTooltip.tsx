import { useEffect } from "react";

const TOOLTIP_ID = "bm-title-tooltip";
const DATA_STASH = "data-bm-title-stashed";

function ensureTooltipEl(): HTMLDivElement {
  let el = document.getElementById(TOOLTIP_ID) as HTMLDivElement | null;
  if (el) return el;

  el = document.createElement("div");
  el.id = TOOLTIP_ID;
  el.className = "bm-title-tooltip";
  el.setAttribute("role", "tooltip");
  el.dataset.show = "false";
  document.body.appendChild(el);
  return el;
}

function findTitledElement(start: EventTarget | null): HTMLElement | null {
  const el = start instanceof HTMLElement ? start : null;
  if (!el) return null;
  return (el.closest?.("[title]") as HTMLElement | null) ?? null;
}

export function TitleTooltip(): null {
  useEffect(() => {
    const tooltip = ensureTooltipEl();

    let activeEl: HTMLElement | null = null;
    let rafPending = false;
    let lastX = 0;
    let lastY = 0;
    let tipW = 0;
    let tipH = 0;

    const hide = () => {
      tooltip.dataset.show = "false";
      tooltip.textContent = "";
      tooltip.style.transform = "translate3d(-9999px, -9999px, 0)";

      if (activeEl) {
        const stashed = activeEl.getAttribute(DATA_STASH);
        if (stashed !== null) {
          activeEl.setAttribute("title", stashed);
          activeEl.removeAttribute(DATA_STASH);
        }
      }
      activeEl = null;
      tipW = 0;
      tipH = 0;
    };

    const position = (x: number, y: number) => {
      if (tooltip.dataset.show !== "true") return;

      const offset = 12;
      const padding = 8;

      // Compute size once per show; fallback to a quick read if needed.
      if (!tipW || !tipH) {
        const r = tooltip.getBoundingClientRect();
        tipW = r.width;
        tipH = r.height;
      }

      const maxX = Math.max(padding, window.innerWidth - padding - tipW);
      const maxY = Math.max(padding, window.innerHeight - padding - tipH);

      const px = Math.min(maxX, Math.max(padding, x + offset));
      const py = Math.min(maxY, Math.max(padding, y + offset));

      tooltip.style.transform = `translate3d(${Math.round(px)}px, ${Math.round(py)}px, 0)`;
    };

    const requestPosition = () => {
      if (rafPending) return;
      rafPending = true;
      requestAnimationFrame(() => {
        rafPending = false;
        position(lastX, lastY);
      });
    };

    const showFor = (el: HTMLElement, titleText: string) => {
      const text = titleText.trim();
      if (!text) return;

      if (!el.hasAttribute(DATA_STASH)) {
        el.setAttribute(DATA_STASH, titleText);
        el.removeAttribute("title");
      }

      activeEl = el;
      tooltip.textContent = titleText;
      tooltip.dataset.show = "true";
      tipW = 0;
      tipH = 0;

      // Measure next frame after content is in place.
      requestAnimationFrame(() => {
        const r = tooltip.getBoundingClientRect();
        tipW = r.width;
        tipH = r.height;
        requestPosition();
      });
    };

    const onPointerOver = (e: PointerEvent) => {
      const el = findTitledElement(e.target);
      if (!el) return;

      const title = el.getAttribute("title");
      if (!title) return;

      lastX = e.clientX;
      lastY = e.clientY;
      showFor(el, title);
      requestPosition();
    };

    const onPointerMove = (e: PointerEvent) => {
      lastX = e.clientX;
      lastY = e.clientY;
      requestPosition();
    };

    const onPointerOut = (e: PointerEvent) => {
      const el = findTitledElement(e.target);
      if (!activeEl || !el) return;
      if (el !== activeEl) return;
      hide();
    };

    const onFocusIn = (e: FocusEvent) => {
      const el = findTitledElement(e.target);
      if (!el) return;

      const title = el.getAttribute("title");
      if (!title) return;

      const r = el.getBoundingClientRect();
      lastX = Math.round(r.left + Math.min(r.width, 16) / 2);
      lastY = Math.round(r.top + Math.min(r.height, 16) / 2);
      showFor(el, title);
      requestPosition();
    };

    const onFocusOut = (e: FocusEvent) => {
      if (!activeEl) return;
      if (e.target !== activeEl) return;
      hide();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") hide();
    };

    const onResize = () => {
      tipW = 0;
      tipH = 0;
      requestPosition();
    };

    // Capture phase ensures we can stash/remove titles before the browser paints native tooltip.
    window.addEventListener("pointerover", onPointerOver, { capture: true, passive: true });
    window.addEventListener("pointerout", onPointerOut, { capture: true, passive: true });
    window.addEventListener("pointermove", onPointerMove, { passive: true });
    window.addEventListener("focusin", onFocusIn, true);
    window.addEventListener("focusout", onFocusOut, true);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("blur", hide, true);
    window.addEventListener("resize", onResize, true);

    return () => {
      hide();
      window.removeEventListener("pointerover", onPointerOver, true);
      window.removeEventListener("pointerout", onPointerOut, true);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("focusin", onFocusIn, true);
      window.removeEventListener("focusout", onFocusOut, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("blur", hide, true);
      window.removeEventListener("resize", onResize, true);

      // Remove DOM node to fully release resources.
      tooltip.remove();
    };
  }, []);

  return null;
}

