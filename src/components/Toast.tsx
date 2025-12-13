import React, {
    createContext,
    useContext,
    useState,
    useCallback,
    useRef,
    useEffect,
    ReactNode,
} from "react";
import { createPortal } from "react-dom";
import "./Toast.css";
import {AlertCircle, CheckCircle, Info} from "lucide-react";

// --- Types ---
export type ToastType = "info" | "success" | "error";
export type ToastPosition =
    | "top-left"
    | "top-right"
    | "bottom-left"
    | "bottom-right";

interface ToastData {
    id: string;
    text: string;
    type: ToastType;
    duration: number;
    exiting: boolean; // 控制退出动画类名
    onClose?: () => void;
}

interface ToastOptions {
    duration?: number;
    onClose?: () => void;
    id?: string;
}

interface ToastContextValue {
    show: (params: { text: string; type?: ToastType } & ToastOptions) => string;
    success: (text: string, opts?: ToastOptions) => string;
    error: (text: string, opts?: ToastOptions) => string;
    info: (text: string, opts?: ToastOptions) => string;
    remove: (id: string) => void;
    clear: () => void;
}

interface ToastProviderProps {
    children: ReactNode;
    position?: ToastPosition;
    maxToasts?: number;
    defaultDuration?: number;
    /**
     * 必须与 CSS 中的 --toast-anim-duration 保持一致或略大
     */
    exitAnimationDelay?: number;
}

// --- Context ---
const ToastContext = createContext<ToastContextValue | null>(null);

let idCounter = 0;
const genId = () => `toast_${Date.now()}_${++idCounter}`;


export function Toast({
                                  children,
                                  position = "bottom-right",
                                  maxToasts = 5,
                                  defaultDuration = 3000,
                                  exitAnimationDelay = 400, // 对应 CSS 动画时长
                              }: ToastProviderProps) {
    const [toasts, setToasts] = useState<ToastData[]>([]);

    // timers: Map<toastId, timeoutId>
    const timers = useRef(new Map<string, number>());

    // 1. 彻底移除（在动画结束后调用）
    const handleFinalRemove = useCallback((id: string) => {
        const t = timers.current.get(id);
        if (t) {
            window.clearTimeout(t);
            timers.current.delete(id);
        }

        setToasts((prev) => {
            const found = prev.find((item) => item.id === id);
            if (found?.onClose) {
                try {
                    found.onClose();
                } catch (e) {
                    console.error("Toast onClose error:", e);
                }
            }
            return prev.filter((item) => item.id !== id);
        });
    }, []);

    // 2. 触发退出流程（添加 exiting 类 -> 等待动画 -> 移除）
    const remove = useCallback(
        (id: string) => {
            setToasts((prev) =>
                prev.map((t) => (t.id === id ? { ...t, exiting: true } : t))
            );

            // 清除该 Toast 现有的自动关闭计时器（防止重复触发）
            const existingTimer = timers.current.get(id);
            if (existingTimer) {
                window.clearTimeout(existingTimer);
                timers.current.delete(id);
            }

            // 设置“最终移除”的计时器，等待 CSS 动画播完
            const exitTimer = window.setTimeout(() => {
                handleFinalRemove(id);
            }, exitAnimationDelay);

            timers.current.set(id, exitTimer);
        },
        [exitAnimationDelay, handleFinalRemove]
    );

    // 3. 显示 Toast
    const show = useCallback(
        ({
             text,
             type = "info",
             duration = defaultDuration,
             id,
             onClose,
         }: { text: string; type?: ToastType } & ToastOptions) => {
            const _id = id || genId();

            setToasts((prev) => {
                // 新增 Toast
                const newToast: ToastData = {
                    id: _id,
                    text,
                    type,
                    duration,
                    exiting: false,
                    onClose,
                };

                // 这里的逻辑是：如果已有太多，先标记旧的为 exiting
                // 注意：为了 UI 稳定性，我们通常只截断多余的，不立即强制删除 DOM
                // 但为了简单，这里直接保留最新的 maxToasts 个
                const nextToasts = [...prev, newToast];

                if (nextToasts.length > maxToasts) {
                    // 找到最旧的一个没有正在退出的 toast 触发移除
                    const oldestId = nextToasts.find(t => !t.exiting)?.id;
                    if(oldestId) {
                        // 我们不能在这里直接调用 remove(oldestId) 因为这会导致 setState 循环
                        // 更好的做法是在 useEffect 中监听长度变化，或者在这里只做截断
                        // 这里采用异步触发移除以保持 setState 纯净
                        setTimeout(() => remove(oldestId), 0);
                    }
                }

                return nextToasts;
            });

            // 设置自动关闭
            if (duration > 0) {
                const autoTimer = window.setTimeout(() => {
                    remove(_id);
                }, duration);
                timers.current.set(_id, autoTimer);
            }

            return _id;
        },
        [defaultDuration, maxToasts, remove]
    );

    // API 封装
    const success = useCallback(
        (text: string, opts?: ToastOptions) => show({ text, type: "success", ...opts }),
        [show]
    );
    const error = useCallback(
        (text: string, opts?: ToastOptions) => show({ text, type: "error", ...opts }),
        [show]
    );
    const info = useCallback(
        (text: string, opts?: ToastOptions) => show({ text, type: "info", ...opts }),
        [show]
    );

    const clear = useCallback(() => {
        setToasts((prev) => {
            prev.forEach((t) => {
                if (!t.exiting) remove(t.id);
            });
            return prev;
        });
    }, [remove]);

    // 卸载清理
    useEffect(() => {
        return () => {
            timers.current.forEach((t) => window.clearTimeout(t));
            timers.current.clear();
        };
    }, []);

    const api: ToastContextValue = { show, success, error, info, remove, clear };

    return (
        <ToastContext.Provider value={api}>
            {children}
            {createPortal(
                <div
                    className={`toast-container position-${position}`}
                    role="region"
                    aria-live="polite"
                >
                    {toasts.map((t) => (
                        <ToastItem
                            key={t.id}
                            {...t}
                            onCloseClick={() => remove(t.id)}
                        />
                    ))}
                </div>,
                document.body
            )}
        </ToastContext.Provider>
    );
}

// --- Hook ---
export function useToast() {
    const context = useContext(ToastContext);
    if (!context) {
        throw new Error("useToast must be used within a ToastProvider");
    }
    return context;
}

// --- Sub Component (Memoized for performance) ---
const ToastItem = React.memo(
    ({
         text,
         type,
         exiting,
         onCloseClick,
     }: ToastData & { onCloseClick: () => void }) => {
        return (
            <div
                className={`toast-item type-${type} ${exiting ? "exiting" : "entering"}`}
                onClick={onCloseClick}
                role="alert"
            >
                <div className="toast-icon">{getIcon(type)}</div>
                <div className="toast-content">{text}</div>
                <button
                    className="toast-close-btn"
                    aria-label="Close"
                    onClick={(e) => {
                        e.stopPropagation();
                        onCloseClick();
                    }}
                >
                    ×
                </button>
            </div>
        );
    }
);

function getIcon(type: ToastType) {
    const props = { size: 14, strokeWidth: 3 };

    switch (type) {
        case "success":
            return <CheckCircle {...props} />;
        case "error":
            return <AlertCircle {...props} />;
        case "info":
        default:
            return <Info {...props} />;
    }
}