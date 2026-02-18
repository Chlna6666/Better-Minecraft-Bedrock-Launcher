const TOOLTIP_ID = "bm-title-tooltip";
const DATA_STASH = "bmTitleStashed";

function ensureTooltipEl(): HTMLDivElement {
  let el = document.getElementById(TOOLTIP_ID) as HTMLDivElement | null;
  if (el) return el;

  el = document.createElement("div");
  el.id = TOOLTIP_ID;
  el.className = "bm-title-tooltip";
  el.setAttribute("role", "tooltip");
  document.body.appendChild(el);
  return el;
}

function findTitledElement(start: EventTarget | null): HTMLElement | null {
  const el = start instanceof HTMLElement ? start : null;
  if (!el) return null;

  // Prefer the closest element with a `title` attribute.
  return el.closest?.("[title]") as HTMLElement | null;
}

export function initTitleTooltip(): void {
  const tooltip = ensureTooltipEl();

  let activeEl: HTMLElement | null = null;
  let rafPending = false;
  let lastX = 0;
  let lastY = 0;

  const hide = () => {
    tooltip.dataset.show = "false";
    tooltip.textContent = "";
    tooltip.style.transform = "translate3d(-9999px, -9999px, 0)";

    if (activeEl) {
      const stashed = activeEl.getAttribute(`data-${DATA_STASH}`);
      if (stashed !== null) {
        activeEl.setAttribute("title", stashed);
        activeEl.removeAttribute(`data-${DATA_STASH}`);
      }
    }
    activeEl = null;
  };

  const showFor = (el: HTMLElement, titleText: string) => {
    if (!titleText.trim()) return;

    // Stash + remove native tooltip.
    if (!el.hasAttribute(`data-${DATA_STASH}`)) {
      el.setAttribute(`data-${DATA_STASH}`, titleText);
      el.removeAttribute("title");
    }

    activeEl = el;
    tooltip.textContent = titleText;
    tooltip.dataset.show = "true";
  };

  const position = (x: number, y: number) => {
    // Avoid layout churn if hidden.
    if (tooltip.dataset.show !== "true") return;

    const offset = 12;
    const padding = 8;

    const rect = tooltip.getBoundingClientRect();
    const maxX = Math.max(padding, window.innerWidth - padding - rect.width);
    const maxY = Math.max(padding, window.innerHeight - padding - rect.height);

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

  const onEnter = (e: Event) => {
    const el = findTitledElement(e.target);
    if (!el) return;

    const title = el.getAttribute("title");
    if (!title) return;

    showFor(el, title);
    requestPosition();
  };

  const onMove = (e: MouseEvent) => {
    lastX = e.clientX;
    lastY = e.clientY;
    requestPosition();
  };

  const onLeave = (e: Event) => {
    // Only hide when the active element is left.
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

    // For keyboard focus, anchor to element center-ish.
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

  window.addEventListener("mouseover", onEnter, true);
  window.addEventListener("mouseout", onLeave, true);
  window.addEventListener("mousemove", onMove, true);
  window.addEventListener("focusin", onFocusIn, true);
  window.addEventListener("focusout", onFocusOut, true);
  window.addEventListener("scroll", hide, true);
  window.addEventListener("keydown", onKeyDown, true);

  // If the window loses focus, avoid a stuck tooltip.
  window.addEventListener("blur", hide, true);
}

