import React, { useState, useRef, useEffect, useMemo, useLayoutEffect } from 'react';
import './Select.css';
import { ChevronDown } from 'lucide-react';

export default function Select({
                                   value,
                                   onChange,
                                   options = [],
                                   placeholder = 'Select...',
                                   disabled = false,
                                   className = '',
                                   optionKey = 'value',
                                   size = 'md',                   // 'sm' | 'md' | 'lg' | number(px)
                                   fullWidth = false,             // 废弃：现在默认宽度100%，由父容器控制
                                   dropdownMatchButton = true,    // 下拉是否与按钮等宽
                                   maxHeight = 240,               // 下拉最大高度
                                   style = {},                    // 额外样式
                               }) {
    const [open, setOpen] = useState(false);
    const [highlight, setHighlight] = useState(-1);
    const rootRef = useRef(null);
    const listWrapperRef = useRef(null);
    const listRef = useRef(null);

    const [listWidth, setListWidth] = useState(null);
    const [listAlign, setListAlign] = useState('left');

    // 数据标准化
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

    // 尺寸映射
    const sizeVars = useMemo(() => {
        if (typeof size === 'number') {
            return {
                '--select-font-size': `${size}px`,
                '--select-padding': `${Math.round(size/2.5)}px ${Math.round(size)}px`,
            };
        }
        const map = {
            sm: { f: 13, pad: '6px 10px', itemPad: '6px 10px' },
            md: { f: 14, pad: '8px 12px', itemPad: '8px 12px' },
            lg: { f: 16, pad: '10px 14px', itemPad: '10px 14px' },
        }[size] ?? { f: 14, pad: '8px 12px', itemPad: '8px 12px' };

        return {
            '--select-font-size': `${map.f}px`,
            '--select-padding': map.pad,
            '--select-item-padding': map.itemPad,
        };
    }, [size]);

    // ---------- 智能定位逻辑 (保留核心，去除冗余) ----------
    const computingRef = useRef(false);

    // 寻找最近的裁剪容器
    function findClippingAncestor(el) {
        if (!el) return null;
        let node = el.parentElement;
        while (node && node !== document.documentElement) {
            const style = getComputedStyle(node);
            if (style.overflow !== 'visible' || style.overflowX !== 'visible' || style.overflowY !== 'visible') {
                return node;
            }
            node = node.parentElement;
        }
        return document.documentElement; // 默认为视口
    }

    useLayoutEffect(() => {
        if (!open) return;

        function compute() {
            if (computingRef.current || !rootRef.current || !listWrapperRef.current) return;
            computingRef.current = true;

            requestAnimationFrame(() => {
                const rootRect = rootRef.current.getBoundingClientRect();
                const clipNode = findClippingAncestor(rootRef.current);
                const clipRect = clipNode === document.documentElement
                    ? { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight, width: window.innerWidth }
                    : clipNode.getBoundingClientRect();

                const paddingGap = 8;
                const btnW = rootRect.width;

                // 1. 确定宽度
                let finalW = btnW;
                if (!dropdownMatchButton) {
                    // 如果不强制等宽，测量内容自然宽度
                    // 暂时取消宽度限制以测量
                    const prevW = listWrapperRef.current.style.width;
                    listWrapperRef.current.style.width = 'max-content';
                    const contentW = listWrapperRef.current.offsetWidth;
                    listWrapperRef.current.style.width = prevW; // 恢复
                    finalW = contentW;
                }

                // 确保不小于按钮宽度 (可选，看设计需求，这里设定为最小也是按钮宽)
                finalW = Math.max(finalW, btnW);

                // 2. 确定对齐方式 (左对齐还是右对齐)
                // 检查左侧空间
                const spaceRight = clipRect.right - rootRect.left - paddingGap; // 按钮左边到右边界的距离
                const spaceLeft = rootRect.right - clipRect.left - paddingGap;  // 按钮右边到左边界的距离

                let align = 'left';
                let renderedWidth = finalW;

                if (finalW <= spaceRight) {
                    align = 'left';
                } else if (finalW <= spaceLeft) {
                    align = 'right';
                } else {
                    // 两边都放不下，取空间大的一边，并限制宽度
                    if (spaceRight >= spaceLeft) {
                        align = 'left';
                        renderedWidth = Math.max(btnW, spaceRight);
                    } else {
                        align = 'right';
                        renderedWidth = Math.max(btnW, spaceLeft);
                    }
                }

                setListWidth(renderedWidth);
                setListAlign(align);
                computingRef.current = false;
            });
        }

        compute();
        window.addEventListener('resize', compute);
        return () => window.removeEventListener('resize', compute);
    }, [open, dropdownMatchButton]);

    // ---------- 事件监听 ----------
    useEffect(() => {
        const handleClickOutside = (e) => {
            if (rootRef.current && !rootRef.current.contains(e.target)) {
                setOpen(false);
            }
        };
        if (open) document.addEventListener('mousedown', handleClickOutside);
        return () => document.removeEventListener('mousedown', handleClickOutside);
    }, [open]);

    const handleSelect = (opt) => {
        if (opt.disabled) return;
        onChange && onChange(opt.__orig);
        setOpen(false);
    };

    // 键盘支持
    const handleKeyDown = (e) => {
        if (disabled) return;
        if (e.key === 'Enter' || e.key === ' ') {
            if (!open) {
                e.preventDefault();
                setOpen(true);
            } else if (highlight >= 0) {
                e.preventDefault();
                handleSelect(normalized[highlight]);
            }
        } else if (e.key === 'Escape') {
            setOpen(false);
        } else if (e.key === 'ArrowDown') {
            e.preventDefault();
            if (!open) setOpen(true);
            setHighlight(prev => {
                let next = prev + 1;
                if (next >= normalized.length) next = 0;
                while (normalized[next].disabled && next !== prev) {
                    next = (next + 1) % normalized.length;
                }
                scrollOptionIntoView(next);
                return next;
            });
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            if (!open) setOpen(true);
            setHighlight(prev => {
                let next = prev - 1;
                if (next < 0) next = normalized.length - 1;
                while (normalized[next].disabled && next !== prev) {
                    next = (next - 1 + normalized.length) % normalized.length;
                }
                scrollOptionIntoView(next);
                return next;
            });
        }
    };

    const scrollOptionIntoView = (index) => {
        const item = listRef.current?.children[index];
        item?.scrollIntoView({ block: 'nearest' });
    };

    // 动态样式
    const wrapperStyle = {
        width: listWidth ? `${listWidth}px` : '100%',
        [listAlign]: 0,
        right: listAlign === 'left' ? 'auto' : 0, // 重置另一边
        left: listAlign === 'right' ? 'auto' : 0,
    };

    return (
        <div
            className={`select-root ${disabled ? 'is-disabled' : ''} ${className}`}
            ref={rootRef}
            style={{ ...sizeVars, ...style }}
        >
            <button
                type="button"
                className="select-btn"
                aria-haspopup="listbox"
                aria-expanded={open}
                disabled={disabled}
                onClick={() => !disabled && setOpen(!open)}
                onKeyDown={handleKeyDown}
            >
                <span className={selected ? 'select-value' : 'select-placeholder'}>
                    {displayLabel}
                </span>

                {/* 箭头：使用 CSS 绘制或 SVG */}
                <span className="select-arrow">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                        <path d="m6 9 6 6 6-6"/>
                    </svg>
                </span>
            </button>

            <div
                className={`select-list-wrapper ${open ? 'open' : ''}`}
                ref={listWrapperRef}
                style={wrapperStyle}
            >
                <ul
                    className="select-list"
                    role="listbox"
                    ref={listRef}
                    style={{ '--select-max-height': typeof maxHeight === 'number' ? `${maxHeight}px` : maxHeight }}
                >
                    {normalized.map((opt, idx) => {
                        const isSelected = selected && String(opt.value) === String(selected.value);
                        return (
                            <li
                                key={opt._idx}
                                role="option"
                                aria-selected={isSelected}
                                className={`select-item ${isSelected ? 'is-selected' : ''} ${highlight === idx ? 'is-highlighted' : ''} ${opt.disabled ? 'is-disabled' : ''}`}
                                onClick={() => handleSelect(opt)}
                                onMouseEnter={() => setHighlight(idx)}
                            >
                                {opt.label}
                            </li>
                        );
                    })}
                    {normalized.length === 0 && (
                        <li className="select-item is-disabled" style={{justifyContent: 'center', opacity: 0.5}}>
                            No options
                        </li>
                    )}
                </ul>
            </div>
        </div>
    );
}