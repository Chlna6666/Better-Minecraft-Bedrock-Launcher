import React, { createContext, useContext, useState, useCallback, useRef, useEffect } from "react";
import { createPortal } from "react-dom";
import "./Toast.css";


const ToastContext = createContext(null);
let idCounter = 0;
const genId = () => `toast_${Date.now()}_${++idCounter}`;

export function ToastProvider({ children, position = "bottom-right", maxToasts = 5, defaultDuration = 2000, exitAnimationDelay = 300 }) {
    const [toasts, setToasts] = useState([]);
    // timers: Map<toastId, timerId>
    const timers = useRef(new Map());

    // 在最终删除时统一调用（清理定时器，触发 onClose，移出 state）
    const handleFinalRemove = useCallback((id) => {
        const t = timers.current.get(id);
        if (t) {
            clearTimeout(t);
            timers.current.delete(id);
        }
        setToasts(prev => {
            const found = prev.find(tt => tt.id === id);
            if (found && typeof found.onClose === 'function') {
                try { found.onClose(); } catch(e) { console.warn(e); }
            }
            return prev.filter(tt => tt.id !== id);
        });
    }, []);

    // 标记为退出（执行退出动画），并在 exitAnimationDelay 后真正移除
    const remove = useCallback((id) => {
        setToasts(prev => prev.map(t => t.id === id ? { ...t, exiting: true } : t));

        // 清理之前可能存在的自动关闭计时器
        const prevTimer = timers.current.get(id);
        if (prevTimer) {
            clearTimeout(prevTimer);
            timers.current.delete(id);
        }

        // 安排最终删除（动画结束后）
        const exitTimer = window.setTimeout(() => {
            handleFinalRemove(id);
        }, exitAnimationDelay);
        timers.current.set(id, exitTimer);
    }, [handleFinalRemove, exitAnimationDelay]);

    // 添加 toast，并安排自动关闭
    const show = useCallback(({ text = "", type = "info", duration = defaultDuration, id = null, onClose = null }) => {
        const _id = id || genId();

        setToasts(prev => {
            return [...prev, { id: _id, text, type, duration, exiting: false, onClose }];
        });

        // 自动关闭
        const autoTimer = window.setTimeout(() => remove(_id), duration);
        timers.current.set(_id, autoTimer);

        // 强制限额：如果超出 maxToasts，用动画的方式删除最旧的
        setToasts(prev => {
            if (prev.length > maxToasts) {
                const overflow = prev.slice(0, prev.length - maxToasts);
                overflow.forEach(t => remove(t.id));
                return prev.slice(-maxToasts);
            }
            return prev;
        });

        return _id;
    }, [defaultDuration, maxToasts, remove]);

    const success = useCallback((text, opts = {}) => show({ text, type: "success", duration: opts.duration, onClose: opts.onClose }), [show]);
    const error = useCallback((text, opts = {}) => show({ text, type: "error", duration: opts.duration, onClose: opts.onClose }), [show]);
    const info = useCallback((text, opts = {}) => show({ text, type: "info", duration: opts.duration, onClose: opts.onClose }), [show]);

    const clear = useCallback(() => {
        setToasts(prev => {
            prev.forEach(t => {
                if (!t.exiting) remove(t.id);
            });
            return prev;
        });
    }, [remove]);

    // 组件卸载时清理所有计时器
    useEffect(() => {
        return () => {
            timers.current.forEach(v => clearTimeout(v));
            timers.current.clear();
        };
    }, []);

    const api = { show, success, error, info, remove, clear };

    return (
        <ToastContext.Provider value={api}>
            {children}
            {createPortal(
                <div className={`toast-container toast-${position}`} aria-live="polite" aria-atomic="true">
                    {toasts.map(t => (
                        <ToastItem key={t.id} toast={t} onRequestClose={() => remove(t.id)} />
                    ))}
                </div>,
                document.body
            )}
        </ToastContext.Provider>
    );
}

export const Toast = ToastProvider;

export function useToast() {
    const ctx = useContext(ToastContext);
    if (!ctx) throw new Error("useToast must be used within a ToastProvider");
    return ctx;
}

/* 简化的 ToastItem：不再在子组件维护退出计时器，所有行为由 Provider 统一控制 */
function ToastItem({ toast, onRequestClose }) {
    const { id, text, type = "info", exiting } = toast;

    return (
        <div
            className={`toast-item ${type} ${exiting ? "toast-exit" : "toast-enter"}`}
            role="status"
            aria-live="polite"
            onClick={() => onRequestClose()}
        >
            <div className="toast-content">
                <div className="toast-text">{text}</div>
            </div>
            <button
                className="toast-close"
                aria-label="Close"
                onClick={(e) => { e.stopPropagation(); onRequestClose(); }}
            >×</button>
        </div>
    );
}

