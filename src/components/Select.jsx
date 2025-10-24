import React, { useState, useRef, useEffect, useMemo, useLayoutEffect } from 'react';
import './Select.css';

export default function Select({
                                   value,
                                   onChange,
                                   options = [],
                                   placeholder = 'Select...',
                                   disabled = false,
                                   className = '',
                                   optionKey = 'value',
                                   size = 'md',                   // 'sm' | 'md' | 'lg' | number(px)
                                   fullWidth = false,             // 是否占满父容器宽度
                                   dropdownMatchButton = true,    // 下拉是否与按钮等宽
                                   maxHeight = 240,               // 下拉最大高度（像素或字符串）
                                   style = {},                    // 额外行内样式覆盖
                               }) {
    const [open, setOpen] = useState(false);
    const [highlight, setHighlight] = useState(-1);
    const rootRef = useRef(null);
    const listRef = useRef(null);

    const [listWidth, setListWidth] = useState(null);
    const [listAlign, setListAlign] = useState('left');

    const normalized = useMemo(() => {
        return options.map((opt, idx) => {
            if (typeof opt === 'string' || typeof opt === 'number') {
                const s = String(opt);
                return { value: s, label: s, __orig: opt, disabled: false, _idx: idx };
            }
            const raw = opt.value ?? '';
            return { value: String(raw), label: opt.label ?? String(raw), __orig: raw, disabled: !!opt.disabled, _idx: idx, _obj: opt };
        });
    }, [options]);

    const selected = normalized.find(o => String(o.__orig) === String(value) || o.value === String(value)) ?? null;
    const displayLabel = selected ? selected.label : placeholder;

    const sizeValues = useMemo(() => {
        if (typeof size === 'number') {
            return {
                '--select-font-size': `${size}px`,
                '--select-padding': `${Math.round(size/2.5)}px ${Math.round(size)}px`,
                '--select-item-padding': `${Math.round(size/2.5)}px ${Math.round(size/1.8)}px`,
                '--select-border-radius': `${Math.max(6, Math.round(size/3))}px`
            };
        }
        const map = {
            sm: { f: 13, pad: '6px 10px', itemPad: '6px 10px', rad: '6px' },
            md: { f: 14, pad: '8px 12px', itemPad: '8px 10px', rad: '8px' },
            lg: { f: 16, pad: '10px 14px', itemPad: '10px 12px', rad: '10px' },
        }[size] ?? { f: 14, pad: '8px 12px', itemPad: '8px 10px', rad: '8px' };

        return {
            '--select-font-size': `${map.f}px`,
            '--select-padding': map.pad,
            '--select-item-padding': map.itemPad,
            '--select-border-radius': map.rad,
        };
    }, [size]);

    // ---------- rAF 合并与并发保护 ----------
    const rafRef = useRef(null);           // 用于合并高频事件 (requestAnimationFrame)
    const computingRef = useRef(false);    // 防止 re-entrant compute

    // --------- 新增：寻找裁剪容器（最近的 overflow !== visible 的祖先） ----------
    function findClippingAncestor(el) {
        if (!el) return null;
        let node = el.parentElement;
        while (node && node !== document.documentElement) {
            const style = getComputedStyle(node);
            // 当 overflowX/Y 不是 visible 时，说明它会裁剪子元素（包括 hidden/auto/scroll/clip）
            if (style.overflow !== 'visible' || style.overflowX !== 'visible' || style.overflowY !== 'visible') {
                return node;
            }
            node = node.parentElement;
        }
        return null;
    }

    function getClippingRect(el) {
        // 返回相对于视口的裁剪矩形（left/right/top/bottom/width/height）
        const clipAncestor = findClippingAncestor(el);
        if (!clipAncestor) {
            return {
                left: 0,
                top: 0,
                right: window.innerWidth,
                bottom: window.innerHeight,
                width: window.innerWidth,
                height: window.innerHeight,
            };
        }
        const r = clipAncestor.getBoundingClientRect();
        return {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
            width: r.width,
            height: r.height,
        };
    }

    useLayoutEffect(() => {
        if (!open) return;

        function computeInner() {
            if (computingRef.current) return;
            computingRef.current = true;

            try {
                if (!rootRef.current) return;
                const rect = rootRef.current.getBoundingClientRect();
                const btnW = Math.max(0, Math.floor(rect.width));
                const paddingGap = 8;

                // 获取“裁剪矩形”（可能是视口，也可能是某个 overflow 容器）
                const clip = getClippingRect(rootRef.current);

                if (dropdownMatchButton) {
                    // 与按钮等宽（但也要保证不会超出裁剪容器）
                    // 最终宽度不能超过裁剪容器可用宽度
                    const maxAllowed = Math.max(0, Math.floor(clip.width - paddingGap * 2));
                    const finalBtnW = Math.min(btnW, maxAllowed || btnW);
                    setListWidth(prev => (prev === finalBtnW ? prev : finalBtnW));
                    setListAlign(prev => (prev === 'left' ? prev : 'left'));
                    computingRef.current = false;
                    return;
                }

                const listEl = listRef.current;
                if (!listEl) {
                    setListWidth(prev => (prev === null ? prev : null));
                    setListAlign(prev => (prev === 'left' ? prev : 'left'));
                    computingRef.current = false;
                    return;
                }

                // 临时设置以测量自然宽度（nowrap）
                const prevWidth = listEl.style.width;
                const prevWhite = listEl.style.whiteSpace;
                const prevMaxW = listEl.style.maxWidth;

                listEl.style.width = 'auto';
                listEl.style.whiteSpace = 'nowrap';
                listEl.style.maxWidth = '';

                requestAnimationFrame(() => {
                    try {
                        const contentW = Math.ceil(listEl.scrollWidth || 0);

                        // 最大允许宽度：受裁剪容器限制并留出 paddingGap 的边距
                        let finalWidth = Math.min(contentW, Math.max(0, Math.floor(clip.width - paddingGap * 2)));

                        // 计算在裁剪容器内的可用左右空间（注意 rect.left/right 与 clip.* 都是相对于视口的）
                        const fitsLeft = rect.left + finalWidth + paddingGap <= clip.right;
                        const fitsRight = rect.right - finalWidth - paddingGap >= clip.left;

                        let chosenAlign = 'left';
                        if (fitsLeft) chosenAlign = 'left';
                        else if (fitsRight) chosenAlign = 'right';
                        else {
                            // 两侧都放不下：选择可以放更多的那一侧，并把 finalWidth 限制到那侧可用宽度
                            const availableRight = Math.floor(clip.right - rect.left - paddingGap); // 从按钮左侧开始向右可用宽度
                            const availableLeft = Math.floor(rect.right - clip.left - paddingGap);   // 从按钮右侧向左可用宽度
                            if (availableRight >= availableLeft) {
                                chosenAlign = 'left';
                                finalWidth = Math.max(100, Math.min(finalWidth, availableRight));
                            } else {
                                chosenAlign = 'right';
                                finalWidth = Math.max(100, Math.min(finalWidth, availableLeft));
                            }
                        }

                        // 恢复样式
                        listEl.style.width = prevWidth;
                        listEl.style.whiteSpace = prevWhite;
                        listEl.style.maxWidth = prevMaxW;

                        // 仅在真正变化时 setState
                        setListWidth(prev => (prev === finalWidth ? prev : finalWidth));
                        setListAlign(prev => (prev === chosenAlign ? prev : chosenAlign));
                    } catch (err) {
                        // 失败也恢复样式
                        listEl.style.width = prevWidth;
                        listEl.style.whiteSpace = prevWhite;
                        listEl.style.maxWidth = prevMaxW;
                    } finally {
                        computingRef.current = false;
                    }
                });
            } catch (e) {
                computingRef.current = false;
            }
        }

        function scheduleCompute() {
            if (rafRef.current) return;
            rafRef.current = requestAnimationFrame(() => {
                rafRef.current = null;
                computeInner();
            });
        }

        // 立刻计算一次
        computeInner();

        window.addEventListener('resize', scheduleCompute);
        window.addEventListener('scroll', scheduleCompute, true);

        return () => {
            window.removeEventListener('resize', scheduleCompute);
            window.removeEventListener('scroll', scheduleCompute, true);
            if (rafRef.current) {
                cancelAnimationFrame(rafRef.current);
                rafRef.current = null;
            }
            computingRef.current = false;
        };
    }, [dropdownMatchButton, open, normalized, size]);

    // ---------- 点击文档外关闭 ----------
    useEffect(() => {
        function onDoc(e) {
            if (!rootRef.current) return;
            if (!rootRef.current.contains(e.target)) setOpen(false);
        }
        document.addEventListener('mousedown', onDoc);
        document.addEventListener('touchstart', onDoc);
        return () => {
            document.removeEventListener('mousedown', onDoc);
            document.removeEventListener('touchstart', onDoc);
        };
    }, []);

    // ---------- 键盘交互 ----------
    useEffect(() => {
        if (!open) {
            setHighlight(-1);
            return;
        }
        const moveHighlight = (dir) => {
            const len = normalized.length;
            if (len === 0) return;
            let i = highlight;
            do {
                i = (i + dir + len) % len;
            } while (normalized[i].disabled);
            setHighlight(i);
            const el = listRef.current?.querySelector(`[data-idx="${i}"]`);
            el?.scrollIntoView({ block: 'nearest' });
        };

        function onKey(e) {
            if (!open) return;
            if (e.key === 'ArrowDown') { e.preventDefault(); moveHighlight(1); }
            else if (e.key === 'ArrowUp') { e.preventDefault(); moveHighlight(-1); }
            else if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                if (highlight >= 0 && normalized[highlight] && !normalized[highlight].disabled) {
                    const sel = normalized[highlight];
                    onChange && onChange(sel.__orig);
                    setOpen(false);
                }
            } else if (e.key === 'Escape') {
                setOpen(false);
            } else if (e.key === 'Home') { e.preventDefault(); setHighlight(0); }
            else if (e.key === 'End') { e.preventDefault(); setHighlight(normalized.length - 1); }
        }

        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, [open, highlight, normalized, onChange]);

    const toggle = () => {
        if (disabled) return;
        setOpen(v => !v);
    };

    const handleSelect = (opt) => {
        if (opt.disabled) return;
        onChange && onChange(opt.__orig);
        setOpen(false);
    };

    const mergedRootStyle = {
        ...(fullWidth ? { width: '100%' } : {}),
        ...sizeValues,
        ...style,
    };

    const listWrapperStyle = {};
    if (listWidth != null) {
        listWrapperStyle.width = `${listWidth}px`;
    } else if (dropdownMatchButton) {
        const btnW = rootRef.current?.offsetWidth;
        if (btnW) listWrapperStyle.width = `${btnW}px`;
    }

    if (listAlign === 'left') {
        listWrapperStyle.left = 0;
        listWrapperStyle.right = 'auto';
    } else {
        listWrapperStyle.right = 0;
        listWrapperStyle.left = 'auto';
    }

    return (
        <div
            className={['select-root', className, disabled ? 'is-disabled' : ''].filter(Boolean).join(' ')}
            ref={rootRef}
            style={mergedRootStyle}
        >
            <button
                type="button"
                className="select-btn"
                aria-haspopup="listbox"
                aria-expanded={open}
                aria-disabled={disabled}
                onClick={toggle}
                onKeyDown={(e) => {
                    if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && !open) {
                        e.preventDefault();
                        setOpen(true);
                        const first = normalized.findIndex(o => !o.disabled);
                        setHighlight(first);
                    }
                }}
            >
                <span className={selected ? 'select-value' : 'select-placeholder'}>
                  {displayLabel}
                </span>
                <span className={`select-arrow ${open ? 'open' : ''}`} aria-hidden="true">▾</span>
            </button>

            <div
                className={`select-list-wrapper ${open ? 'open' : ''}`}
                role="presentation"
                style={listWrapperStyle}
            >
                <ul
                    className="select-list"
                    role="listbox"
                    tabIndex={-1}
                    ref={listRef}
                    aria-activedescendant={highlight >= 0 ? `opt-${highlight}` : undefined}
                    onMouseLeave={() => setHighlight(-1)}
                    style={{ ['--select-max-height']: typeof maxHeight === 'number' ? `${maxHeight}px` : String(maxHeight) }}
                >
                    {normalized.map((opt, idx) => {
                        const key = typeof optionKey === 'function' ? optionKey(opt._obj ?? opt.__orig) : (opt[optionKey] ?? opt.value ?? `opt-${idx}`);
                        const isSelected = selected && String(opt.value) === String(selected.value);
                        const isHighlighted = idx === highlight;
                        return (
                            <li
                                id={`opt-${idx}`}
                                data-idx={idx}
                                key={String(key) + '-' + idx}
                                role="option"
                                aria-selected={isSelected}
                                className={[ 'select-item', opt.disabled ? 'is-disabled' : '', isSelected ? 'is-selected' : '', isHighlighted ? 'is-highlighted' : '' ].filter(Boolean).join(' ')}
                                onClick={() => handleSelect(opt)}
                                onMouseEnter={() => setHighlight(idx)}
                            >
                                {opt.label}
                            </li>
                        );
                    })}
                </ul>
            </div>
        </div>
    );
}
