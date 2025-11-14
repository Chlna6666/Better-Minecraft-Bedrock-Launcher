import React, { useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeSanitize from "rehype-sanitize";
import "./update-modal.css";
import close from "../assets/feather/x.svg";
import externalLink from "../assets/feather/external-link.svg";
import IconButton from "./IconButton";

function Spinner() {
    return (
        <svg
            className="um-spinner"
            viewBox="0 0 50 50"
            width="18"
            height="18"
            aria-hidden="true"
        >
            <circle
                cx="25"
                cy="25"
                r="20"
                fill="none"
                strokeWidth="4"
                stroke="currentColor"
                strokeLinecap="round"
            />
        </svg>
    );
}

export default function UpdateModal({
                                        open,
                                        onClose,
                                        release,
                                        onDownload,
                                        downloading = false,
                                        progress = null, // optional: number 0-100, 若为 null 则显示不确定进度
                                        compact = false, // 新增：compact 模式，窗口更紧凑
                                    }) {
    const latest = release || null;

    const prettyTag = useMemo(() => {
        if (!latest || !latest.tag) return "";
        return latest.tag.startsWith("v") ? latest.tag : `v${latest.tag}`;
    }, [latest]);

    const published = useMemo(() => {
        if (!latest?.published_at) return "未知";
        try {
            const d = new Date(latest.published_at);
            return d.toLocaleString("zh-CN", {
                year: "numeric",
                month: "short",
                day: "numeric",
                hour: "2-digit",
                minute: "2-digit",
            });
        } catch {
            return latest.published_at;
        }
    }, [latest]);

    const assetName = latest?.asset_name ?? "无可用二进制/安装包";
    const assetUrl = latest?.asset_url ?? "";
    const prerelease = latest?.prerelease ?? false;
    const changelog = latest?.body ?? "";

    if (!open) return null;

    return (
        <div
            className="um-backdrop"
            role="dialog"
            aria-modal="true"
            aria-labelledby="um-title"
        >
            <div
                className={`um-modal ${compact ? "um-modal--compact" : ""}`}
                role="document"
            >
                <header className="um-header">
                    <div className="um-title-block">
                        <div className="um-badge">{prettyTag}</div>
                        <div className="um-title-area">
                            <h2 id="um-title" className="um-title">
                                {latest?.name ?? "检测到新版本"}
                            </h2>
                            <div className="um-sub">{assetName}</div>
                        </div>
                    </div>
                    <IconButton
                        className="um-close"
                        icon={<img src={close} alt="关闭" />}
                        title="关闭更新弹窗"
                        onClick={onClose}
                        size="sm"
                    />
                </header>

                <main className="um-body">
                    <div className="um-grid">
                        <div className="um-meta">
                            <div className="um-meta-row">
                                <div className="um-label">发布</div>
                                <div className="um-value">{published}</div>
                            </div>
                            <div className="um-meta-row">
                                <div className="um-label">类型</div>
                                <div className="um-value">{prerelease ? "预发布" : "稳定发布"}</div>
                            </div>
                            <div className="um-meta-row">
                                <div className="um-label">文件</div>
                                <div className="um-value um-asset">{assetName}</div>
                            </div>
                            {latest?.asset_size ? (
                                <div className="um-meta-row">
                                    <div className="um-label">大小</div>
                                    <div className="um-value">{latest.asset_size}</div>
                                </div>
                            ) : null}
                        </div>

                        <div className="um-changelog">
                            <div className="um-changelog-title">更新日志</div>
                            {changelog ? (
                                <div className="um-changelog-body">
                                    <ReactMarkdown
                                        remarkPlugins={[remarkGfm]}
                                        rehypePlugins={[rehypeSanitize]}
                                        components={{
                                            a: ({ node, ...props }) => (
                                                <a {...props} target="_blank" rel="noreferrer" />
                                            ),
                                            code: ({ node, inline, className, children, ...props }) => {
                                                if (inline) {
                                                    return (
                                                        <code className={className} {...props}>
                                                            {children}
                                                        </code>
                                                    );
                                                }
                                                return (
                                                    <pre {...props}>
                            <code className={className}>{children}</code>
                          </pre>
                                                );
                                            },
                                        }}
                                    >
                                        {changelog}
                                    </ReactMarkdown>
                                </div>
                            ) : (
                                <div className="um-changelog-empty">暂无变更说明</div>
                            )}
                        </div>
                    </div>

                    <div className="um-notes">
                        <div>建议：点击「更新」将会在下载完成后自动替换更新启动。</div>
                        <div>提示：在 设置 → 启动器 中可关闭自动检查更新。</div>
                    </div>

                    <div className="um-progress-area" aria-hidden={!downloading}>
                        {downloading ? (
                            <>
                                <div className="um-progress-row">
                                    <div className="um-progress-left">
                                        <span className="um-progress-label">下载中</span>
                                        {progress !== null ? (
                                            <span className="um-progress-percent">{Math.round(progress)}%</span>
                                        ) : (
                                            <span className="um-progress-percent">…</span>
                                        )}
                                    </div>
                                    <div className="um-progress-right">
                                        <Spinner />
                                    </div>
                                </div>

                                <div className="um-progress-bar">
                                    <div
                                        className={`um-progress-fill ${
                                            progress === null ? "indeterminate" : ""
                                        }`}
                                        style={{
                                            width:
                                                progress !== null
                                                    ? `${Math.max(0, Math.min(100, progress))}%`
                                                    : undefined,
                                        }}
                                        aria-valuemin={0}
                                        aria-valuemax={100}
                                        aria-valuenow={progress !== null ? Math.round(progress) : undefined}
                                        role="progressbar"
                                    />
                                </div>
                            </>
                        ) : null}
                    </div>
                </main>

                <footer className="um-actions">
                    <div className="um-left">
                        <a
                            className={`um-link ${assetUrl ? "" : "um-link-disabled"}`}
                            href={assetUrl || "#"}
                            target="_blank"
                            rel="noreferrer"
                            onClick={(e) => {
                                if (!assetUrl) e.preventDefault();
                            }}
                        >
                            在浏览器下载{" "}
                            <IconButton
                                icon={<img src={externalLink} alt="外部链接图标" />}
                                size="sm"
                                className="um-ext-icon"
                                disabled
                                aria-hidden="true"
                            />
                        </a>
                    </div>

                    <div className="um-right">
                        <button className="um-btn um-btn-ghost" onClick={onClose}>
                            稍后
                        </button>

                        <button
                            className="um-btn um-btn-primary"
                            onClick={() => onDownload && onDownload(latest)}
                            disabled={downloading || !latest || !assetUrl}
                            aria-disabled={downloading || !latest || !assetUrl}
                        >
                            {downloading ? (
                                <>
                                    <Spinner /> <span className="um-btn-text">下载中…</span>
                                </>
                            ) : (
                                <>
                                    <span className="um-btn-text">更新</span>
                                </>
                            )}
                        </button>
                    </div>
                </footer>
            </div>
        </div>
    );
}
