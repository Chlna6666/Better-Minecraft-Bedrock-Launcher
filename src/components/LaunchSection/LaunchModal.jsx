import * as shell from "@tauri-apps/plugin-shell";
import React, { useEffect, useMemo, useState } from "react";
import "./LaunchModal.css";

const ANIM_MS = 320; // 动画时长（ms）

/**
 * LaunchModal props:
 *  - open: boolean
 *  - launching: boolean
 *  - error: { code?: string, message?: string } | null
 *  - details?: string | { text: string, status?: "ok"|"info"|"error" } | string[]
 *  - onClose: () => void
 *  - onRetry?: () => void
 */
export default function LaunchModal({ open, launching, error, details, onClose, onRetry }) {
    const [mounted, setMounted] = useState(open);
    const [phase, setPhase] = useState(open ? "entering" : "exited");
    const [copied, setCopied] = useState(false);
    const [successPulse, setSuccessPulse] = useState(false);

    useEffect(() => {
        if (open) {
            setMounted(true);
            requestAnimationFrame(() => {
                setPhase("entering");
                const t = setTimeout(() => setPhase("entered"), ANIM_MS);
                return () => clearTimeout(t);
            });
        } else {
            if (mounted) {
                setPhase("exiting");
                const t = setTimeout(() => {
                    setPhase("exited");
                    setMounted(false);
                }, ANIM_MS);
                return () => clearTimeout(t);
            } else {
                setPhase("exited");
            }
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [open]);

    useEffect(() => {
        if (!mounted) return;
        const prev = document.body.style.overflow;
        document.body.style.overflow = "hidden";
        return () => { document.body.style.overflow = prev; };
    }, [mounted]);

    const { detailText, detailStatus } = useMemo(() => {
        let raw = details;
        if (Array.isArray(raw) && raw.length > 0) raw = raw[raw.length - 1];
        if (!raw) return { detailText: "", detailStatus: "info" };

        if (typeof raw === "object" && ("text" in raw || "status" in raw)) {
            const t = raw.text ?? String(raw);
            const s = (raw.status && ["ok", "info", "error"].includes(raw.status)) ? raw.status : "info";
            return { detailText: String(t), detailStatus: s };
        }

        const s = String(raw);
        const re = /\]\s*\[?[^\]]*\]?\s*(ok|info|error)\b/i;
        const m = s.match(re);
        const status = m ? m[1].toLowerCase() : "info";
        return { detailText: s, detailStatus: status };
    }, [details]);

    useEffect(() => {
        if (!launching && !error && open) {
            setSuccessPulse(true);
            const t = setTimeout(() => setSuccessPulse(false), 650);
            return () => clearTimeout(t);
        }
    }, [launching, error, open]);

    const buildCopyText = () => {
        const code = error?.code ?? "未知";
        const message = (error?.message && String(error.message)) || detailText || "";
        return `错误代码: ${code}\n错误信息: ${message}`;
    };

    const handleCopyError = async () => {
        const text = buildCopyText();
        try {
            if (navigator.clipboard && navigator.clipboard.writeText) {
                await navigator.clipboard.writeText(text);
            } else {
                const ta = document.createElement("textarea");
                ta.value = text;
                ta.setAttribute("readonly", "");
                ta.style.position = "absolute";
                ta.style.left = "-9999px";
                document.body.appendChild(ta);
                ta.select();
                document.execCommand("copy");
                document.body.removeChild(ta);
            }
            setCopied(true);
            setTimeout(() => setCopied(false), 1600);
        } catch (e) {
            console.warn("copy failed", e);
            setCopied(true);
            setTimeout(() => setCopied(false), 1600);
        }
    };

    // 只用错误代码搜索 —— 若无错误代码则不执行搜索
    const handleSearchError = async () => {
        const code = error?.code ? String(error.code).trim() : "";
        if (!code) {
            console.warn("没有错误代码可搜索");
            return;
        }
        const query = encodeURIComponent(code);
        const url = `https://www.bing.com/search?q=${query}`;

        // 优先使用 Tauri plugin-shell.open，在非 Tauri 或出错时回退到 window.open
        try {
            if (shell && typeof shell.open === "function") {
                // shell.open 返回 Promise<void>
                await shell.open(url);
                return;
            }
        } catch (e) {
            console.warn("plugin-shell open failed, fallback to window.open", e);
        }

        try {
            window.open(url, "_blank", "noopener");
        } catch (e) {
            console.warn("window.open failed", e);
        }
    };

    if (!mounted) return null;

    const overlayClass = `lm-overlay lm-overlay-${phase}`;
    const cardClass = `lm-card lm-card-${phase} ${successPulse ? "lm-card-success-pulse" : ""}`;
    const title = launching ? "正在启动…" : (error ? "启动失败" : "启动完成");

    return (
        <div className={overlayClass} role="dialog" aria-modal="true">
            <div className={cardClass} role="document" aria-live="polite">
                <div className="lm-header">
                    <div className="lm-header-left">
                        <h3 className="lm-title">{title}</h3>
                        <div className="lm-subtitle">
                            {launching ? "请稍候 — 正在准备启动流程" : (error ? "查看错误详情或重试" : "已完成")}
                        </div>
                    </div>
                    <div className="lm-header-right">
                        <button className="lm-close" onClick={() => onClose && onClose()} aria-label="关闭">✕</button>
                    </div>
                </div>

                <div className="lm-body">
                    {launching && (
                        <div className="lm-launching-compact">
                            <div className="lm-spinner-small" aria-hidden="true">
                                <svg viewBox="0 0 50 50" className="lm-spinner-svg" role="img" aria-label="loading">
                                    <circle className="lm-path" cx="25" cy="25" r="20" fill="none" strokeWidth="4" />
                                </svg>
                            </div>
                            <div className="lm-info-block">
                                <div className={`lm-detail-line lm-detail-${detailStatus}`}>
                                    <span className={`lm-dot lm-dot-${detailStatus}`} aria-hidden="true"></span>
                                    <span className="lm-detail-text">{detailText || "正在准备启动环境..."}</span>
                                </div>
                            </div>
                        </div>
                    )}

                    {!launching && error && (
                        <div className="lm-error-compact">
                            <div className="lm-error-row-compact">
                                <div className="lm-error-label-compact">错误代码:</div>
                                <button
                                    className="lm-error-value-compact lm-error-link"
                                    onClick={handleSearchError}
                                    title={error?.code ? "用错误代码在浏览器搜索" : "无错误代码可搜索"}
                                >
                                    {error.code ?? "未知"}
                                </button>
                            </div>
                            {detailText && (
                                <div className="lm-detail-block-compact">
                                    <div className={`lm-detail-line lm-detail-${detailStatus}`}>
                                        <span className={`lm-dot lm-dot-${detailStatus}`} aria-hidden="true"></span>
                                        <span className="lm-detail-text">{detailText}</span>
                                    </div>
                                </div>
                            )}
                        </div>
                    )}

                    {!launching && !error && (
                        <div className={`lm-success-compact ${successPulse ? "lm-success-pulse" : ""}`}>
                            <div className="lm-success-icon">✔</div>
                            <div className="lm-success-text">游戏已启动</div>
                            {detailText && (
                                <div className={`lm-detail-line lm-detail-${detailStatus}`}>
                                    <span className={`lm-dot lm-dot-${detailStatus}`} aria-hidden="true"></span>
                                    <span className="lm-detail-text">{detailText}</span>
                                </div>
                            )}
                        </div>
                    )}
                </div>

                <div className="lm-footer">
                    {!launching && error && (
                        <>
                            <button
                                className={`lm-btn lm-btn-outline lm-copy-btn ${copied ? "lm-copied" : ""}`}
                                onClick={handleCopyError}
                                title="复制错误信息"
                            >
                                {copied ? "已复制" : "复制错误信息"}
                            </button>

                            <button className="lm-btn lm-btn-outline" onClick={() => onClose && onClose()}>
                                关闭
                            </button>

                            {onRetry && <button className="lm-btn lm-btn-primary" onClick={() => onRetry && onRetry()}>重试</button>}
                        </>
                    )}
                    {!launching && !error && (
                        <button className="lm-btn lm-btn-primary" onClick={() => onClose && onClose()}>确定</button>
                    )}
                </div>
            </div>
        </div>
    );
}
