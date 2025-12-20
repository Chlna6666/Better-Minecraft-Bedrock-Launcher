import React, { useState, useRef, useEffect, useMemo, useLayoutEffect } from 'react';
import { createPortal } from 'react-dom';
import './Select.css';

export default function Select({
                                   value,
                                   onChange,
                                   options = [],
                                   placeholder = 'Select...',
                                   disabled = false,
                                   className = '',
                                   size = 'md',
                                   dropdownMatchButton = true, // 即使设为 true，现在也会取 max(按钮宽, 内容宽)
                                   maxHeight = 240,
                                   style = {},
                               }) {
    const [open, setOpen] = useState(false);
    const [highlight, setHighlight] = useState(-1);
    const rootRef = useRef(null);
    const listRef = useRef(null);
    const measureRef = useRef(null); // [新增] 用于隐形测量宽度的 ref

    // 存储计算后的位置和尺寸
    const [layout, setLayout] = useState({
        top: 0,
        left: 0,
        width: 0,
        transformOrigin: 'top center',
        direction: 'down' // 'down' | 'up'
    });

    // 数据标准化
    const normalized = useMemo(() => {
        return options.map((opt, idx) => {
            if (typeof opt === 'string' || typeof opt === 'number') {
                const s = String(opt);
                return { value: s, label: s, __orig: opt, disabled: false, _idx: idx };
            }
            const raw = opt.value ?? '';
            return { value: String(raw), label: opt.label ?? String(raw), __orig: raw, disabled: !!opt.disabled, _idx: idx };
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

    // [核心逻辑] 计算位置、方向和宽度
    const updatePosition = () => {
        if (!open || !rootRef.current) return;

        const rect = rootRef.current.getBoundingClientRect();
        const viewportHeight = window.innerHeight;
        const viewportWidth = window.innerWidth;
        const gap = 6;

        // 1. 测量内容宽度 (如果 measureRef 存在)
        let contentWidth = rect.width;
        if (measureRef.current) {
            contentWidth = measureRef.current.offsetWidth;
        }
        // 最终宽度：至少是按钮宽度，如果内容更宽则撑开 (且不超过视口宽)
        const finalWidth = Math.min(Math.max(rect.width, contentWidth), viewportWidth - 20);

        // 2. 智能判断方向 (向上还是向下)
        // 估算菜单高度 (每个选项约 36px + padding)
        const estimatedMenuHeight = Math.min(normalized.length * 36 + 12, maxHeight);

        const spaceBelow = viewportHeight - rect.bottom;
        const spaceAbove = rect.top;

        let direction = 'down';
        let top = rect.bottom + gap;
        let transformOrigin = 'top center';

        // 如果下方空间不够，且上方空间比下方大，则向上展开
        if (spaceBelow < estimatedMenuHeight && spaceAbove > spaceBelow) {
            direction = 'up';
            top = rect.top - gap; // 这里的 top 是菜单的底部基准线，稍后在 CSS 或 style 里处理
            transformOrigin = 'bottom center';
        }

        // 3. 处理水平溢出 (防止右边超出屏幕)
        let left = rect.left;
        if (left + finalWidth > viewportWidth) {
            left = viewportWidth - finalWidth - 10;
        }

        setLayout({
            top,
            left,
            width: finalWidth,
            direction,
            transformOrigin
        });
    };

    // 每次打开或窗口变化时重新计算
    useLayoutEffect(() => {
        if (open) {
            updatePosition();
            window.addEventListener('scroll', updatePosition, true);
            window.addEventListener('resize', updatePosition);
        }
        return () => {
            window.removeEventListener('scroll', updatePosition, true);
            window.removeEventListener('resize', updatePosition);
        };
    }, [open, normalized.length]); // 依赖选项长度，因为这影响高度

    // 点击外部关闭
    useEffect(() => {
        const handleClickOutside = (e) => {
            if (
                rootRef.current && !rootRef.current.contains(e.target) &&
                listRef.current && !listRef.current.contains(e.target)
            ) {
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

    const handleKeyDown = (e) => {
        if (disabled) return;
        if (e.key === 'Enter' || e.key === ' ') {
            if (!open) { e.preventDefault(); setOpen(true); }
            else if (highlight >= 0) { e.preventDefault(); handleSelect(normalized[highlight]); }
        } else if (e.key === 'Escape') {
            setOpen(false);
        } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            if (!open) setOpen(true);
            const delta = e.key === 'ArrowDown' ? 1 : -1;
            setHighlight(prev => {
                let next = prev + delta;
                if (next >= normalized.length) next = 0;
                if (next < 0) next = normalized.length - 1;
                return next;
            });
        }
    };

    // [新增] 隐形测量层：用于获取最长选项的自然宽度
    const measureLayer = open ? (
        <div
            ref={measureRef}
            className="select-measure-layer"
            style={{ ...sizeVars, opacity: 0, position: 'fixed', top: -9999, left: -9999, pointerEvents: 'none' }}
        >
            <ul className="select-list" style={{ width: 'max-content', display: 'inline-block' }}>
                {normalized.map((opt, idx) => (
                    <li key={idx} className="select-item">{opt.label}</li>
                ))}
            </ul>
        </div>
    ) : null;

    // Portal 内容
    const dropdownMenu = (
        <div
            className={`select-list-wrapper ${open ? 'open' : ''}`}
            ref={listRef}
            style={{
                position: 'fixed',
                left: layout.left,
                // 如果是向下，top 就是 top；如果是向上，bottom 就是视口高 - top
                top: layout.direction === 'down' ? layout.top : 'auto',
                bottom: layout.direction === 'up' ? (window.innerHeight - layout.top) : 'auto',
                width: layout.width,
                transformOrigin: layout.transformOrigin,
                zIndex: 99999,
                ...sizeVars
            }}
        >
            <ul
                className="select-list"
                role="listbox"
                style={{ '--select-max-height': typeof maxHeight === 'number' ? `${maxHeight}px` : maxHeight }}
            >
                {normalized.map((opt, idx) => {
                    const isSelected = selected && String(opt.value) === String(selected.value);
                    return (
                        <li
                            key={opt._idx}
                            role="option"
                            className={`select-item ${isSelected ? 'is-selected' : ''} ${highlight === idx ? 'is-highlighted' : ''} ${opt.disabled ? 'is-disabled' : ''}`}
                            onClick={() => handleSelect(opt)}
                            onMouseEnter={() => setHighlight(idx)}
                        >
                            {opt.label}
                        </li>
                    );
                })}
            </ul>
        </div>
    );

    return (
        <>
            <div
                className={`select-root ${disabled ? 'is-disabled' : ''} ${className}`}
                ref={rootRef}
                style={{ ...sizeVars, ...style }}
            >
                <button
                    type="button"
                    className="select-btn"
                    disabled={disabled}
                    onClick={() => !disabled && setOpen(!open)}
                    onKeyDown={handleKeyDown}
                >
                    <span className={selected ? 'select-value' : 'select-placeholder'}>
                        {displayLabel}
                    </span>
                    <span className="select-arrow">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                            <path d="m6 9 6 6 6-6"/>
                        </svg>
                    </span>
                </button>
            </div>

            {/* 测量层 (挂在 body 上) */}
            {open && createPortal(measureLayer, document.body)}

            {/* 实际菜单 (挂在 body 上) */}
            {open && createPortal(dropdownMenu, document.body)}
        </>
    );
}