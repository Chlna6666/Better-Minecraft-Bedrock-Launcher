import { useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeSanitize from "rehype-sanitize";
import { X, ExternalLink, Download, Clock, Tag, Database, Activity, Hourglass } from "lucide-react";
import { formatBytes } from "../utils/fileSize";
import "./update-modal.css";
import { TaskSnapshot } from "../types/updater.ts";
import { useTranslation } from "react-i18next";

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

// --- Components ---

const ProgressBar = ({ snapshot, t }: { snapshot: TaskSnapshot | null; t: (key: string) => string }) => {
    // 解析数据
    const percent = snapshot?.percent ?? 0;
    const isIndeterminate = snapshot?.total === null || snapshot?.total === 0;
    // @ts-ignore // 如果类型定义还没改，先忽略报错以验证修复
    const rawSpeed = snapshot?.speedBytesPerSec || snapshot?.speed_bytes_per_sec || 0;
    const speed = formatBytes(rawSpeed) + "/s";

    const downloaded = formatBytes(snapshot?.done || 0);
    const total = snapshot?.total ? formatBytes(snapshot.total) : t("common.unknown");
    const eta = snapshot?.eta === "unknown" ? "--:--" : snapshot?.eta;

    return (
        <div className="um-progress-wrapper">
            {/* 1. 顶部：百分比大数字 */}
            <div className="um-progress-header">
                <span className="um-progress-status-text">
                    {snapshot?.stage === "extracting" ? t("UpdateModal.progress.extracting") : t("UpdateModal.progress.downloading")}
                </span>
                <span className="um-progress-percent">
                    {isIndeterminate ? "--" : Math.floor(percent)}<span className="symbol">%</span>
                </span>
            </div>

            {/* 2. 进度条主体 */}
            <div className="um-progress-track">
                <div
                    className={`um-progress-fill ${isIndeterminate ? "indeterminate" : ""}`}
                    style={{ width: isIndeterminate ? "100%" : `${percent}%` }}
                />
            </div>

            {/* 3. 底部：仪表盘数据网格 */}
            <div className="um-stats-dashboard">
                <div className="um-stat-card">
                    <div className="um-stat-icon-wrapper blue">
                        <Activity size={14} />
                    </div>
                    <div className="um-stat-content">
                        <span className="um-stat-label">{t("UpdateModal.progress.speed")}</span>
                        <span className="um-stat-value">{speed}</span>
                    </div>
                </div>

                <div className="um-stat-card">
                    <div className="um-stat-icon-wrapper orange">
                        <Hourglass size={14} />
                    </div>
                    <div className="um-stat-content">
                        <span className="um-stat-label">{t("UpdateModal.progress.eta")}</span>
                        <span className="um-stat-value">{eta}</span>
                    </div>
                </div>

                <div className="um-stat-card">
                    <div className="um-stat-icon-wrapper green">
                        <Database size={14} />
                    </div>
                    <div className="um-stat-content">
                        <span className="um-stat-label">{t("UpdateModal.progress.downloaded")}</span>
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
    const { i18n, t } = useTranslation();
    const latest = release || null;

    // Data processing
    const { prettyTag, published, assetUrl, changelog, isPrerelease } = useMemo(() => {
        if (!latest) return {};

        const tag = latest.tag?.startsWith("v") ? latest.tag : `v${latest.tag || "0.0.0"}`;

        let pubDate = t("common.unknown");
        try {
            if (latest.published_at) {
                const locale = (i18n.language || "en-US").replace('_', '-');
                pubDate = new Date(latest.published_at).toLocaleString(locale, {
                    year: "numeric", month: "short", day: "numeric",
                });
            }
        } catch {}

        return {
            prettyTag: tag,
            published: pubDate,
            assetName: latest.asset_name ?? t("UpdateModal.no_file"),
            assetUrl: latest.asset_url ?? "",
            changelog: latest.body ?? "",
            isPrerelease: latest.prerelease ?? false
        };
    }, [latest, i18n.language, t]);

    // 如果未打开，不渲染任何内容
    if (!open) return null;

    return (
        <div className="um-backdrop um-anim-backdrop" role="dialog">
            <div className={`um-modal um-anim-modal ${compact ? "um-modal--compact" : ""}`}>
                        {/* Header Area */}
                        <div className="um-header">
                            <div className="um-header-content">
                                <div className="um-badge-group">
                                    <span className={`um-badge ${isPrerelease ? "beta" : "stable"}`}>
                                        {isPrerelease ? t("common.beta") : t("common.release")}
                                    </span>
                                    <span className="um-version">{prettyTag}</span>
                                </div>
                                <h2 className="um-title">{latest?.name || t("UpdateModal.title")}</h2>
                            </div>
                            {!downloading && (
                                <button
                                    className="um-icon-btn"
                                    onClick={onClose}
                                    data-bm-title={t("common.close")}
                                >
                                    <X size={20} />
                                </button>
                            )}
                        </div>

                        {/* Content Switcher */}
                        <div className="um-content-wrapper">
                            {downloading ? (
                                <div className="um-download-state um-anim-content">
                                    <div className="um-download-icon-wrapper">
                                        <div className="um-download-pulse"></div>
                                        <Download className="um-download-icon-svg" size={32} />
                                    </div>

                                    <ProgressBar snapshot={snapshot} t={t} />

                                    {onCancel && (
                                        <div className="um-download-actions">
                                            <button className="um-btn ghost sm" onClick={onCancel}>
                                                {t("UpdateModal.cancel_download")}
                                            </button>
                                        </div>
                                    )}
                                </div>
                            ) : (
                                <div className="um-info-state um-anim-content">
                                        {/* Meta Grid */}
                                        <div className="um-meta-grid">
                                            <div className="um-meta-item">
                                                <Clock size={14} /> <span>{published}</span>
                                            </div>
                                            {latest?.asset_size && (
                                                <div className="um-meta-item">
                                                    <Database size={14} />
                                                        <span>{formatBytes(latest.asset_size, { defaultBytes: 0, defaultText: t("common.unknown") })}</span>
                                                </div>
                                            )}
                                        </div>

                                        {/* Changelog */}
                                        <div className="um-changelog-container custom-scrollbar">
                                            <div className="um-changelog-label">
                                                <Tag size={14} /> {t("UpdateModal.changelog")}
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
                                                    <div className="um-empty-log">{t("UpdateModal.empty_log")}</div>
                                                )}
                                            </div>
                                        </div>

                                        <div className="um-footer-note">
                                            {t("UpdateModal.hint_auto_check")}
                                        </div>

                                        {/* Actions */}
                                        <div className="um-actions">
                                            <a
                                                href={assetUrl || "#"}
                                                target="_blank"
                                                rel="noreferrer"
                                                className={`um-external-link ${!assetUrl ? "disabled" : ""}`}
                                            >
                                                <ExternalLink size={14} /> {t("UpdateModal.browser_download")}
                                            </a>
                                            <div className="um-actions-right">
                                                <button className="um-btn ghost" onClick={onClose}>
                                                    {t("UpdateModal.later")}
                                                </button>
                                                <button
                                                    className="um-btn primary"
                                                    onClick={() => latest && onDownload(latest)}
                                                    disabled={!latest || !assetUrl}
                                                >
                                                    {t("UpdateModal.update_now")}
                                                </button>
                                            </div>
                                        </div>
                                </div>
                            )}
                        </div>
            </div>
        </div>
    );
}
