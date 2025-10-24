// Section.jsx  （增强版）
import React, {
    useState,
    useEffect,
    useCallback,
    useMemo,
    Suspense,
    useRef,
} from "react";
import "./Section.css";

function TabButton({ k, label, active, onSelect }) {
    return (
        <button
            key={k}
            className={`section-tab ${active ? "active" : ""}`}
            onClick={() => onSelect(k)}
            aria-pressed={active}
            type="button"
        >
            {label}
        </button>
    );
}
const MemoTabButton = React.memo(TabButton);

function Section({
                     id,
                     tabs = [],
                     defaultActive = tabs.length ? tabs[0].key : null,
                     animation = "bounce", // "bounce" | "fade" | "none"
                     animationDuration = 320,
                     keepAlive = false,
                     headerButtons = null,
                     className = "",
                     // NEW: called before changing tab; return false to block the change
                     beforeTabChange = null,
                 }) {
    const [active, setActive] = useState(defaultActive);
    const [mounted, setMounted] = useState(() =>
        new Set(defaultActive ? [defaultActive] : [])
    );

    const contentRootRef = useRef(null);
    const paneRefs = useRef(new Map());
    const pendingTimeoutRef = useRef(null);
    const pendingListenerRef = useRef(null);

    useEffect(() => {
        if (defaultActive) {
            setActive(defaultActive);
            setMounted((s) => {
                const ns = new Set(s);
                ns.add(defaultActive);
                return ns;
            });
        }
    }, [defaultActive]);

    const attemptFocus = useCallback((node, maxMs = 600, interval = 40) => {
        if (!node) return;
        const selectors =
            'input:not([disabled]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), [tabindex]:not([tabindex="-1"])';
        const start = Date.now();

        const tryFocus = () => {
            try {
                const el = node.querySelector ? node.querySelector(selectors) : null;
                if (el && typeof el.focus === "function") {
                    el.focus({ preventScroll: true });
                    return;
                }
                if (node && typeof node.focus === "function") {
                    node.focus({ preventScroll: true });
                    return;
                }
            } catch {
                // ignore
            }
            if (Date.now() - start < maxMs) {
                setTimeout(tryFocus, interval);
            }
        };
        tryFocus();
    }, []);

    const clearPending = useCallback(() => {
        if (pendingTimeoutRef.current) {
            clearTimeout(pendingTimeoutRef.current);
            pendingTimeoutRef.current = null;
        }
        if (pendingListenerRef.current && pendingListenerRef.current.node) {
            try {
                pendingListenerRef.current.node.removeEventListener(
                    "transitionend",
                    pendingListenerRef.current.fn
                );
            } catch {}
            pendingListenerRef.current = null;
        }
    }, []);

    useEffect(() => {
        return () => {
            clearPending();
        };
    }, [clearPending]);

    const handleTabChange = useCallback(
        (newKey) => {
            if (!newKey || newKey === active) return;

            // check beforeTabChange hook
            try {
                if (typeof beforeTabChange === "function") {
                    const ok = beforeTabChange(newKey);
                    if (!ok) return;
                }
            } catch {
                // if hook throws, proceed with default behavior
            }

            clearPending();

            setMounted((s) => {
                const ns = new Set(s);
                ns.add(newKey);
                return ns;
            });

            setActive(newKey);

            const node = () => paneRefs.current.get(newKey) || contentRootRef.current;

            const onTransitionFinished = () => {
                const n = node();
                attemptFocus(n);
                if (!keepAlive) {
                    setMounted(() => new Set([newKey]));
                }
                clearPending();
            };

            const targetNode = node();
            if (targetNode) {
                const fn = (ev) => {
                    if (
                        ev.target &&
                        (ev.propertyName === "opacity" || ev.propertyName === "transform")
                    ) {
                        onTransitionFinished();
                    }
                };
                try {
                    targetNode.addEventListener("transitionend", fn);
                    pendingListenerRef.current = { node: targetNode, fn };
                } catch {}
            }

            pendingTimeoutRef.current = setTimeout(() => {
                onTransitionFinished();
            }, Math.max(0, animationDuration) + 80);
        },
        [active, animationDuration, clearPending, attemptFocus, keepAlive, beforeTabChange]
    );

    const tabMap = useMemo(() => {
        const m = new Map();
        tabs.forEach((t) => m.set(t.key, t));
        return m;
    }, [tabs]);

    const renderTabContent = useCallback(
        (key) => {
            const tab = tabMap.get(key);
            if (!tab) return null;
            const Comp = tab.component;
            if (React.isValidElement(Comp)) return Comp;
            return <Comp />;
        },
        [tabMap]
    );

    return (
        <div id={id} className={`section-root ${className}`}>
            <div className="section-header">
                <div className="section-tabs">
                    {tabs.map((t) => (
                        <MemoTabButton
                            key={t.key}
                            k={t.key}
                            label={t.label}
                            active={active === t.key}
                            onSelect={handleTabChange}
                        />
                    ))}
                </div>

                <div className="section-header-right">{headerButtons}</div>
            </div>

            <div
                ref={contentRootRef}
                className={`section-content ${animation === "fade" ? "fade-mode" : ""}`}
                style={{ ["--section-transition-ms"]: `${animationDuration}ms` }}
            >
                {Array.from(mounted).map((k) => {
                    const isActive = k === active;
                    return (
                        <div
                            key={k}
                            ref={(el) => {
                                if (el) paneRefs.current.set(k, el);
                            }}
                            className={`section-pane ${isActive ? "is-active" : "is-inactive"}`}
                            aria-hidden={!isActive}
                        >
                            <Suspense fallback={null}>{renderTabContent(k)}</Suspense>
                        </div>
                    );
                })}
            </div>
        </div>
    );
}

export default React.memo(Section);
