import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import './AssetContextMenu.css';

export interface ContextMenuItem {
    label: string;
    icon?: React.ReactNode;
    action: string;
    danger?: boolean;
    disabled?: boolean;
    separator?: boolean;
}

interface Props {
    x: number;
    y: number;
    items: ContextMenuItem[];
    onClose: () => void;
    onAction: (action: string) => void;
}

export const AssetContextMenu: React.FC<Props> = ({ x, y, items, onClose, onAction }) => {
    const menuRef = useRef<HTMLDivElement>(null);
    const [pos, setPos] = useState<{ top: number; left: number }>({ top: y, left: x });

    // 点击外部关闭
    useEffect(() => {
        const handleClickOutside = (e: MouseEvent) => {
            if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
                onClose();
            }
        };
        const handleScroll = () => onClose();

        // 禁用菜单内的右键默认行为
        const handleContextMenu = (e: MouseEvent) => {
            if (menuRef.current && menuRef.current.contains(e.target as Node)) {
                e.preventDefault();
            }
        };

        document.addEventListener('mousedown', handleClickOutside);
        document.addEventListener('contextmenu', handleContextMenu);
        window.addEventListener('scroll', handleScroll, true);

        return () => {
            document.removeEventListener('mousedown', handleClickOutside);
            document.removeEventListener('contextmenu', handleContextMenu);
            window.removeEventListener('scroll', handleScroll, true);
        };
    }, [onClose]);

    useLayoutEffect(() => {
        const el = menuRef.current;
        if (!el) return;

        const padding = 10;
        const { innerWidth, innerHeight } = window;
        const rect = el.getBoundingClientRect();

        let left = x;
        let top = y;

        // Prefer flipping (left/up) over hard clamping to keep the menu near the click point.
        if (left + rect.width > innerWidth - padding) left = left - rect.width;
        if (top + rect.height > innerHeight - padding) top = top - rect.height;

        // Final clamp to viewport
        if (left + rect.width > innerWidth - padding) left = innerWidth - padding - rect.width;
        if (top + rect.height > innerHeight - padding) top = innerHeight - padding - rect.height;
        if (left < padding) left = padding;
        if (top < padding) top = padding;

        setPos({ left, top });
    }, [x, y, items.length]);

    return createPortal(
        <div className="asset-context-menu" style={pos} ref={menuRef}>
            {items.map((item, i) => {
                if (item.separator) return <div key={i} className="ctx-divider" />;
                return (
                    <div
                        key={item.action + i}
                        className={`ctx-item ${item.danger ? 'danger' : ''} ${item.disabled ? 'disabled' : ''}`}
                        onClick={() => {
                            if (!item.disabled) {
                                onAction(item.action);
                                onClose();
                            }
                        }}
                    >
                        {item.icon && <span className="ctx-icon">{item.icon}</span>}
                        <span>{item.label}</span>
                    </div>
                );
            })}
        </div>,
        document.body
    );
};
