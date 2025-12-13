import { useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeSanitize from "rehype-sanitize";
import { motion, AnimatePresence } from "framer-motion";
import { X, ExternalLink, Download, Clock, Tag, Database, Activity, Hourglass } from "lucide-react";
import { formatBytes } from "../utils/fileSize";
import "./update-modal.css";
import { TaskSnapshot } from "../types/updater.ts";

// --- Types ---
export interface ReleaseData {
    tag: string;
    name: string;
    body: string;
    published_at: string;
    prerelease: boolean;
    asset_name?: string;
    asset_url?: string;
    asset_size?: number;
}

interface UpdateModalProps {
    open: boolean;
    onClose: () => void;
    release?: ReleaseData | null;
    onDownload: (release: ReleaseData) => void;
    downloading?: boolean;
    snapshot?: TaskSnapshot | null;
    compact?: boolean;
    onCancel?: () => void;
}

// --- Animation Variants ---
const overlayVariants = {
    hidden: { opacity: 0 },
    visible: { opacity: 1, transition: { duration: 0.2 } },
    exit: { opacity: 0, transition: { duration: 0.2, delay: 0.1 } },
};

const modalVariants = {
    hidden: { opacity: 0, scale: 0.95, y: 20 },
    visible: {
        opacity: 1,
        scale: 1,
        y: 0,
        transition: { type: "spring", stiffness: 300, damping: 25 },
    },
    exit: { opacity: 0, scale: 0.95, y: 10, transition: { duration: 0.2 } },
};

const contentVariants = {
    hidden: { opacity: 0, x: -20 },
    visible: { opacity: 1, x: 0 },
    exit: { opacity: 0, x: 20 },
};

// --- Components ---

const ProgressBar = ({ snapshot }: { snapshot: TaskSnapshot | null }) => {
    // 解析数据
    const percent = snapshot?.percent ?? 0;
    const isIndeterminate = snapshot?.total === null || snapshot?.total === 0;

    // 格式化数据
    const speed = formatBytes(snapshot?.speed_bytes_per_sec || 0) + "/s";
    const downloaded = formatBytes(snapshot?.done || 0);
    const total = snapshot?.total ? formatBytes(snapshot.total) : "未知";
    const eta = snapshot?.eta === "unknown" ? "--:--" : snapshot?.eta;

    return (
        <div className="um-progress-wrapper">
            {/* 1. 顶部：百分比大数字 */}
            <div className="um-progress-header">
                <span className="um-progress-status-text">
                    {snapshot?.stage === "extracting" ? "正在解压..." : "下载中..."}
                </span>
                <span className="um-progress-percent">
                    {isIndeterminate ? "--" : Math.floor(percent)}<span className="symbol">%</span>
                </span>
            </div>

            {/* 2. 进度条主体 */}
            <div className="um-progress-track">
                <motion.div
                    className={`um-progress-fill ${isIndeterminate ? "indeterminate" : ""}`}
                    initial={{ width: 0 }}
                    animate={{ width: isIndeterminate ? "100%" : `${percent}%` }}
                    transition={isIndeterminate ? { repeat: Infinity, duration: 1.5 } : { type: "spring", stiffness: 50, damping: 15 }}
                />
            </div>

            {/* 3. 底部：仪表盘数据网格 */}
            <div className="um-stats-dashboard">
                <div className="um-stat-card">
                    <div className="um-stat-icon-wrapper blue">
                        <Activity size={14} />
                    </div>
                    <div className="um-stat-content">
                        <span className="um-stat-label">下载速度</span>
                        <span className="um-stat-value">{speed}</span>
                    </div>
                </div>

                <div className="um-stat-card">
                    <div className="um-stat-icon-wrapper orange">
                        <Hourglass size={14} />
                    </div>
                    <div className="um-stat-content">
                        <span className="um-stat-label">剩余时间</span>
                        <span className="um-stat-value">{eta}</span>
                    </div>
                </div>

                <div className="um-stat-card">
                    <div className="um-stat-icon-wrapper green">
                        <Database size={14} />
                    </div>
                    <div className="um-stat-content">
                        <span className="um-stat-label">已下载</span>
                        <span className="um-stat-value">{downloaded} <span className="sub">/ {total}</span></span>
                    </div>
                </div>
            </div>
        </div>
    );
};

export default function UpdateModal({
                                        open,
                                        onClose,
                                        release,
                                        onDownload,
                                        downloading = false,
                                        snapshot = null,
                                        compact = false,
                                        onCancel,
                                    }: UpdateModalProps) {
    const latest = release || null;

    // Data processing
    const { prettyTag, published, assetUrl, changelog, isPrerelease } = useMemo(() => {
        if (!latest) return {};

        const tag = latest.tag?.startsWith("v") ? latest.tag : `v${latest.tag || "0.0.0"}`;

        let pubDate = "未知";
        try {
            if (latest.published_at) {
                pubDate = new Date(latest.published_at).toLocaleString("zh-CN", {
                    year: "numeric", month: "short", day: "numeric",
                });
            }
        } catch {}

        return {
            prettyTag: tag,
            published: pubDate,
            assetName: latest.asset_name ?? "无可用文件",
            assetUrl: latest.asset_url ?? "",
            changelog: latest.body ?? "",
            isPrerelease: latest.prerelease ?? false
        };
    }, [latest]);

    // 如果未打开，不渲染任何内容
    if (!open) return null;

    return (
        <AnimatePresence>
            {open && (
                <motion.div
                    className="um-backdrop"
                    variants={overlayVariants}
                    initial="hidden"
                    animate="visible"
                    exit="exit"
                    role="dialog"
                >
                    <motion.div
                        className={`um-modal ${compact ? "um-modal--compact" : ""}`}
                        variants={modalVariants}
                        layout
                    >
                        {/* Header Area */}
                        <div className="um-header">
                            <div className="um-header-content">
                                <div className="um-badge-group">
                                    <span className={`um-badge ${isPrerelease ? "beta" : "stable"}`}>
                                        {isPrerelease ? "测试版" : "正式版"}
                                    </span>
                                    <span className="um-version">{prettyTag}</span>
                                </div>
                                <h2 className="um-title">{latest?.name || "版本更新"}</h2>
                            </div>
                            {!downloading && (
                                <button
                                    className="um-icon-btn"
                                    onClick={onClose}
                                    title="关闭"
                                >
                                    <X size={20} />
                                </button>
                            )}
                        </div>

                        {/* Content Switcher */}
                        <div className="um-content-wrapper">
                            <AnimatePresence mode="wait">
                                {downloading ? (
                                    /* ---------------- Downloading State ---------------- */
                                    <motion.div
                                        key="downloading"
                                        className="um-download-state"
                                        variants={contentVariants}
                                        initial="hidden"
                                        animate="visible"
                                        exit="exit"
                                    >
                                        <div className="um-download-icon-wrapper">
                                            <div className="um-download-pulse"></div>
                                            <Download className="um-download-icon-svg" size={32} />
                                        </div>

                                        <ProgressBar snapshot={snapshot} />

                                        {onCancel && (
                                            <div className="um-download-actions">
                                                <button className="um-btn ghost sm" onClick={onCancel}>
                                                    取消下载
                                                </button>
                                            </div>
                                        )}
                                    </motion.div>
                                ) : (
                                    /* ---------------- Info State ---------------- */
                                    <motion.div
                                        key="info"
                                        className="um-info-state"
                                        variants={contentVariants}
                                        initial="hidden"
                                        animate="visible"
                                        exit="exit"
                                    >
                                        {/* Meta Grid */}
                                        <div className="um-meta-grid">
                                            <div className="um-meta-item">
                                                <Clock size={14} /> <span>{published}</span>
                                            </div>
                                            {latest?.asset_size && (
                                                <div className="um-meta-item">
                                                    <Database size={14} />
                                                    <span>{formatBytes(latest.asset_size, { defaultBytes: 0, defaultText: "未知" })}</span>
                                                </div>
                                            )}
                                        </div>

                                        {/* Changelog */}
                                        <div className="um-changelog-container custom-scrollbar">
                                            <div className="um-changelog-label">
                                                <Tag size={14} /> 更新日志
                                            </div>
                                            <div className="um-markdown-body">
                                                {changelog ? (
                                                    <ReactMarkdown
                                                        remarkPlugins={[remarkGfm]}
                                                        rehypePlugins={[rehypeSanitize]}
                                                        components={{
                                                            a: (props) => <a {...props} target="_blank" rel="noreferrer" />,
                                                        }}
                                                    >
                                                        {changelog}
                                                    </ReactMarkdown>
                                                ) : (
                                                    <div className="um-empty-log">暂无详细说明</div>
                                                )}
                                            </div>
                                        </div>

                                        <div className="um-footer-note">
                                            提示：在 设置 → 启动器 中可关闭自动检查更新。
                                        </div>

                                        {/* Actions */}
                                        <div className="um-actions">
                                            <a
                                                href={assetUrl || "#"}
                                                target="_blank"
                                                rel="noreferrer"
                                                className={`um-external-link ${!assetUrl ? "disabled" : ""}`}
                                            >
                                                <ExternalLink size={14} /> 浏览器下载
                                            </a>
                                            <div className="um-actions-right">
                                                <button className="um-btn ghost" onClick={onClose}>
                                                    稍后
                                                </button>
                                                <button
                                                    className="um-btn primary"
                                                    onClick={() => latest && onDownload(latest)}
                                                    disabled={!latest || !assetUrl}
                                                >
                                                    立即更新
                                                </button>
                                            </div>
                                        </div>
                                    </motion.div>
                                )}
                            </AnimatePresence>
                        </div>
                    </motion.div>
                </motion.div>
            )}
        </AnimatePresence>
    );
}