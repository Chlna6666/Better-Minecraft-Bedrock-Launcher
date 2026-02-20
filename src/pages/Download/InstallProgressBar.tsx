/* src/pages/Download/InstallProgressBar.tsx */
import React, {useCallback, useEffect, useReducer, useRef, useState} from "react";
import ReactDOM from "react-dom";
import {invoke} from "@tauri-apps/api/core";
import {basename} from "@tauri-apps/api/path";
import {useTranslation} from "react-i18next";
import {listen, UnlistenFn} from "@tauri-apps/api/event";
import "./InstallProgressBar.css";

// ================================================================================================
// 1. 类型定义
// ================================================================================================

interface TaskSnapshot {
    id: string;
    stage: string;
    total: number | null;
    done: number;
    speedBytesPerSec: number;
    eta: string;
    percent: number | null;
    status: string;
    message: string | null;
}

interface InstallProgressBarProps {
    version: string;
    packageId: string | null;
    versionType: number;
    md5?: string | null;
    onStatusChange: (isDownloading: boolean) => void;
    onCompleted: (packageId: string | null) => void;
    onCancel: (packageId: string | null) => void;
    isImport?: boolean;
    sourcePath?: string | null;
    children: React.ReactNode;
    isGDK?: boolean;
    autoExtractPath?: string | null; // when set, install from local path after user confirms
    forceDownload?: boolean; // ignore local cache and re-download
}

type CdnProbeResult = {
    base: string;
    url: string;
    latency_ms: number | null;
    error: string | null;
};

type CdnProbeResponse = {
    recommended_base: string | null;
    results: CdnProbeResult[];
};

interface ProgressData {
    processed: number;
    total: number;
    speed: string;
    eta: string;
    percent: number;
    stage: string;
    message: string;
}

type Status = 'confirming' | 'starting' | 'progress' | 'error' | 'cancelled' | 'completed';

type State = {
    status: Status;
    error: string | null;
    fileName: string;
    progress: ProgressData;
};

type Action =
    | { type: 'START_DOWNLOAD' }
    | { type: 'DOWNLOAD_STARTED' }
    | { type: 'UPDATE_PROGRESS'; payload: Partial<ProgressData> }
    | { type: 'SET_ERROR'; payload: string }
    | { type: 'RETRY' }
    | { type: 'REQUEST_CANCEL' }
    | { type: 'CONFIRM_CANCEL' }
    | { type: 'COMPLETE' }
    | { type: 'SET_FILENAME'; payload: string };

// ================================================================================================
// 2. 工具函数
// ================================================================================================

const formatSpeed = (bytesPerSec: number): string => {
    if (!bytesPerSec || isNaN(bytesPerSec) || bytesPerSec <= 0) return "0 B/s";
    const units = ["B/s", "KB/s", "MB/s", "GB/s"];
    let value = bytesPerSec;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) {
        value /= 1024;
        i++;
    }
    return `${value.toFixed(2)} ${units[i]}`;
};

const formatMegaBytes = (bytes: number): string => ((bytes || 0) / 1e6).toFixed(2);

const mapSnapshotToProgress = (snap: TaskSnapshot): Partial<ProgressData> => {
    return {
        processed: snap.done,
        total: snap.total || 0,
        speed: formatSpeed(snap.speedBytesPerSec),
        eta: snap.eta || "00:00:00",
        percent: snap.percent ?? (snap.total && snap.total > 0 ? (snap.done / snap.total) * 100 : 0),
        stage: snap.stage,
        message: snap.message || "",
    };
};

// ================================================================================================
// 3. Initial State & Reducer
// ================================================================================================

const initialState: State = {
    status: 'confirming',
    error: null,
    fileName: '',
    progress: {
        processed: 0,
        total: 0,
        speed: "0 B/s",
        eta: "00:00:00",
        percent: 0,
        stage: "ready",
        message: "",
    },
};

function reducer(state: State, action: Action): State {
    switch (action.type) {
        case 'START_DOWNLOAD':
            return { ...state, status: 'starting', error: null };
        case 'DOWNLOAD_STARTED':
            return { ...state, status: 'progress' };
        case 'UPDATE_PROGRESS':
            return { ...state, progress: { ...state.progress, ...action.payload } };
        case 'SET_ERROR':
            return { ...state, status: 'error', error: action.payload };
        case 'RETRY':
            return { ...initialState, fileName: state.fileName, status: 'starting' };
        case 'REQUEST_CANCEL':
        case 'CONFIRM_CANCEL':
            return { ...state, status: 'cancelled' };
        case 'COMPLETE':
            return { ...state, status: 'completed', progress: { ...state.progress, percent: 100 } };
        case 'SET_FILENAME':
            return { ...state, fileName: action.payload };
        default:
            return state;
    }
}

// ================================================================================================
// 4. [新] 独立动画组件：SmoothProgressBar
//    核心优化：完全脱离 React Render 周期，使用 rAF 直接操作 DOM 样式。
// ================================================================================================

const SmoothProgressBar = React.memo(({ targetPercent }: { targetPercent: number }) => {
    const barRef = useRef<HTMLDivElement>(null);
    const glowRef = useRef<HTMLDivElement>(null);
    const textRef = useRef<HTMLSpanElement>(null);
    const currentPercentRef = useRef(0);
    const rafRef = useRef<number | null>(null);

    useEffect(() => {
        // 当目标改变时，启动动画循环
        const animate = () => {
            if (!barRef.current) return;

            const target = targetPercent;
            const current = currentPercentRef.current;
            const diff = target - current;

            // 缓动逻辑：每次移动差距的 20%
            let next = current + diff * 0.2;

            // 如果非常接近，直接到位并停止
            if (Math.abs(diff) < 0.1) {
                next = target;
                rafRef.current = null; // 标记停止
            }

            currentPercentRef.current = next;

            // 1. 更新进度条宽度 (Layout/Paint 优化: 只修改 transform 或 width，width 触发 layout 但在这里是必要的)
            barRef.current.style.width = `${next}%`;

            // 2. 更新辉光
            if (glowRef.current) {
                glowRef.current.style.width = `${next}%`;
                glowRef.current.style.opacity = next > 0 ? '1' : '0';
            }

            // 3. 更新大号数字 (Text Content)，完全绕过 React Diff
            if (textRef.current) {
                textRef.current.textContent = `${next.toFixed(1)}%`;
            }

            if (rafRef.current !== null) {
                rafRef.current = requestAnimationFrame(animate);
            }
        };

        // 如果没有正在运行的动画，启动它
        if (rafRef.current === null) {
            rafRef.current = requestAnimationFrame(animate);
        }

        return () => {
            if (rafRef.current !== null) {
                cancelAnimationFrame(rafRef.current);
                rafRef.current = null;
            }
        };
    }, [targetPercent]);

    return (
        <>
            {/* 将大号数字移到这里，由本组件直接管理 DOM */}
            <div className="bm-install-header left-align" style={{marginBottom: 0}}>
                {/* 占位，保持布局结构 */}
                <div style={{display:'none'}}></div>
                <span ref={textRef} className="bm-percent-large tabular-nums" style={{ marginLeft: 'auto' }}>
                    0.0%
                </span>
            </div>

            <div className="bm-progress-track-wrapper">
                <div className="bm-progress-bar-track">
                    <div ref={barRef} className="bm-progress-bar-fill" style={{ width: '0%' }} />
                    <div ref={glowRef} className="bm-progress-bar-glow" style={{ width: '0%', opacity: 0 }} />
                </div>
            </div>
        </>
    );
});

// ================================================================================================
// 5. Views
// ================================================================================================

const ConfirmView: React.FC<{
    downloadVersion: string;
    versionType: number;
    fileName: string;
    isImport: boolean;
    isGdk: boolean;
    cdnLoading: boolean;
    cdnError: string | null;
    cdnResults: CdnProbeResult[];
    selectedCdnBase: string;
    onSelectCdnBase: (base: string) => void;
    onRefreshCdn: () => void;
    onFileNameChange: (name: string) => void;
    onConfirm: () => void;
    onCancel: () => void;
}> = React.memo(({
    downloadVersion,
    versionType,
    fileName,
    isImport,
    isGdk,
    cdnLoading,
    cdnError,
    cdnResults,
    selectedCdnBase,
    onSelectCdnBase,
    onRefreshCdn,
    onFileNameChange,
    onConfirm,
    onCancel,
}) => {
    const { t } = useTranslation();

    const getTypeLabel = () => {
        if (versionType === 0) return t('common.release');
        if (versionType === 1) return t('common.beta');
        if (versionType === 2) return t('common.preview');
        return '';
    };

    const getLatencyLevel = (latencyMs: number | null): 'fast' | 'ok' | 'slow' | 'bad' | 'fail' => {
        if (latencyMs == null) return 'fail';
        if (latencyMs <= 50) return 'fast';
        if (latencyMs <= 150) return 'ok';
        if (latencyMs <= 300) return 'slow';
        return 'bad';
    };

    const renderLatencyText = (latencyMs: number | null) => {
        if (latencyMs == null) return t("InstallProgressBar.gdk_cdn_failed");
        return `${latencyMs} ms`;
    };

    return (
        <>
            <div className="bm-install-header">
                <div className="bm-icon-circle confirm">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                        <polyline points="7 10 12 15 17 10" />
                        <line x1="12" y1="15" x2="12" y2="3" />
                    </svg>
                </div>
                <div className="bm-install-header-main">
                    <div className="bm-install-header-text">
                        <h2 className="bm-install-title">{isImport ? t("InstallProgressBar.import_title") : t("InstallProgressBar.confirm_title")}</h2>
                        <p className="bm-install-subtitle">{isImport ? t("InstallProgressBar.import_sub") : t("InstallProgressBar.confirm_sub")}</p>
                    </div>
                    {!isImport && (
                        <div className="bm-meta-row">
                            <span className="bm-chip version tabular-nums">{downloadVersion}</span>
                            {getTypeLabel() && <span className="bm-chip type">{getTypeLabel()}</span>}
                            <span className={`bm-chip platform ${isGdk ? 'gdk' : 'uwp'}`}>{isGdk ? 'GDK' : 'UWP'}</span>
                        </div>
                    )}
                </div>
            </div>
            <div className="bm-install-body">
                <div className="bm-input-group">
                    <label htmlFor="filename-input" className="bm-input-label">{t("InstallProgressBar.filename_label")}</label>
                    <input
                        id="filename-input"
                        className="bm-modern-input"
                        value={fileName}
                        onChange={(e) => onFileNameChange(e.target.value)}
                        onKeyDown={(e) => e.key === "Enter" && onConfirm()}
                        autoFocus
                    />
                </div>

                {isGdk && !isImport && (
                    <div className="bm-input-group bm-cdn-group">
                        <div className="bm-cdn-header">
                            <label className="bm-input-label" style={{ marginBottom: 0 }}>
                                {t("InstallProgressBar.gdk_cdn_label")}
                            </label>
                            <button
                                type="button"
                                className="bm-btn secondary bm-cdn-retest"
                                onClick={onRefreshCdn}
                                disabled={cdnLoading}
                            >
                                {cdnLoading ? t("InstallProgressBar.gdk_cdn_testing") : t("InstallProgressBar.gdk_cdn_retest")}
                            </button>
                        </div>

                        <div className="bm-cdn-list" role="radiogroup" aria-label={t("InstallProgressBar.gdk_cdn_label")}>
                            {(cdnResults.length ? cdnResults : [{ base: selectedCdnBase || t("InstallProgressBar.gdk_cdn_unknown"), url: "", latency_ms: null, error: null }]).map((r) => {
                                const isSelected = r.base === selectedCdnBase;
                                const level = getLatencyLevel(r.latency_ms);
                                return (
                                    <button
                                        key={r.base}
                                        type="button"
                                        className={`bm-cdn-item ${isSelected ? 'selected' : ''}`}
                                        onClick={() => onSelectCdnBase(r.base)}
                                        disabled={cdnLoading || !r.base}
                                        role="radio"
                                        aria-checked={isSelected}
                                    >
                                        <div className="bm-cdn-left">
                                            <div className="bm-cdn-base tabular-nums" data-bm-title={r.base}>
                                                {r.base}
                                            </div>
                                            {r.error && (
                                                <div className="bm-cdn-error" data-bm-title={r.error}>
                                                    {r.error}
                                                </div>
                                            )}
                                        </div>
                                        <div className={`bm-cdn-badge ${level}`}>
                                            {renderLatencyText(r.latency_ms)}
                                        </div>
                                    </button>
                                );
                            })}
                        </div>

                        {cdnError && (
                            <div className="bm-cdn-error-global">
                                {cdnError}
                            </div>
                        )}

                        <div className="bm-cdn-hint">
                            {t("InstallProgressBar.gdk_cdn_hint")}
                        </div>
                    </div>
                )}
            </div>
            <div className="bm-install-footer">
                <button className="bm-btn secondary" onClick={onCancel}>{t("InstallProgressBar.cancel")}</button>
                <button className="bm-btn primary" onClick={onConfirm}>
                    {isImport ? t("InstallProgressBar.start_import") : t("InstallProgressBar.start_download")}
                </button>
            </div>
        </>
    );
});

const LocalInstallConfirmView: React.FC<{
    fileName: string;
    versionType: number;
    isGdk: boolean;
    localPath: string;
    onFileNameChange: (name: string) => void;
    onConfirm: () => void;
    onCancel: () => void;
}> = React.memo(({ fileName, versionType, isGdk, localPath, onFileNameChange, onConfirm, onCancel }) => {
    const { t } = useTranslation();

    const getTypeLabel = () => {
        if (versionType === 0) return t('common.release');
        if (versionType === 1) return t('common.beta');
        if (versionType === 2) return t('common.preview');
        return '';
    };

    return (
        <>
            <div className="bm-install-header">
                <div className="bm-icon-circle confirm">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                        <polyline points="7 10 12 15 17 10" />
                        <line x1="12" y1="15" x2="12" y2="3" />
                    </svg>
                </div>
                <div className="bm-install-header-main">
                    <div className="bm-install-header-text">
                        <h2 className="bm-install-title">{t("InstallProgressBar.local_install_title")}</h2>
                        <p className="bm-install-subtitle">{t("InstallProgressBar.local_install_sub")}</p>
                    </div>
                    <div className="bm-meta-row">
                        {getTypeLabel() && <span className="bm-chip type">{getTypeLabel()}</span>}
                        <span className={`bm-chip platform ${isGdk ? 'gdk' : 'uwp'}`}>{isGdk ? 'GDK' : 'UWP'}</span>
                    </div>
                </div>
            </div>

            <div className="bm-install-body">
                <div className="bm-input-group">
                    <label htmlFor="filename-input" className="bm-input-label">{t("InstallProgressBar.folder_label")}</label>
                    <input
                        id="filename-input"
                        className="bm-modern-input"
                        value={fileName}
                        onChange={(e) => onFileNameChange(e.target.value)}
                        onKeyDown={(e) => e.key === "Enter" && onConfirm()}
                        autoFocus
                    />
                </div>
                <div className="bm-input-group">
                    <label className="bm-input-label">{t("InstallProgressBar.local_path_label")}</label>
                    <div className="bm-modern-input bm-path-readonly" title={localPath}>{localPath}</div>
                </div>

                <div className="bm-button-row">
                    <button className="bm-btn secondary" onClick={onCancel}>{t("common.cancel")}</button>
                    <button className="bm-btn primary" onClick={onConfirm}>{t("common.install")}</button>
                </div>
            </div>
        </>
    );
});

const ProgressView: React.FC<{
    progress: ProgressData;
    onCancel: () => void;
}> = React.memo(({ progress, onCancel }) => {
    const { t } = useTranslation();
    let title = t("InstallProgressBar.stage_downloading");

    const knownStages: { [key: string]: string } = {
        "extracting": t("InstallProgressBar.stage_extracting"),
        "importing": t("InstallProgressBar.stage_importing"),
        "downloading": t("InstallProgressBar.stage_downloading"),
        "verifying_file": t("InstallProgressBar.stage_verifying", "Verifying..."),
        "initializing": t("InstallProgressBar.stage_initializing", "Initializing..."),
        "ready": t("InstallProgressBar.stage_ready", "Preparing..."),
    };
    if (knownStages[progress.stage]) title = knownStages[progress.stage];
    else if (progress.stage) title = progress.stage;

    const detail = progress.message || "";
    const processedLabel = (progress.stage === 'extracting' || progress.stage === 'importing')
        ? t("InstallProgressBar.extracted_label")
        : t("InstallProgressBar.processed_label");

    return (
        <>
            <div className="bm-install-header left-align" style={{marginBottom: 0}}>
                <div>
                    <h2 className="bm-install-title">{title}</h2>
                    {detail && (
                        <p className="bm-install-subtitle" style={{ margin: 0, marginTop: '4px', opacity: 0.8, fontSize: '0.85rem' }}>
                            {detail}
                        </p>
                    )}
                </div>
                {/* 这里的数字由 SmoothProgressBar 接管，此处占位为空 */}
            </div>

            <div className="bm-install-body">
                {/* 独立的动画组件 */}
                <SmoothProgressBar targetPercent={progress.percent} />

                <div className="bm-stats-grid">
                    <div className="bm-stat-item">
                        <span className="bm-stat-label">{processedLabel}</span>
                        <span className="bm-stat-value tabular-nums">{formatMegaBytes(progress.processed)} / {formatMegaBytes(progress.total)} MB</span>
                    </div>
                    <div className="bm-stat-item">
                        <span className="bm-stat-label">{t("InstallProgressBar.speed_label")}</span>
                        <span className="bm-stat-value tabular-nums">{progress.speed}</span>
                    </div>
                    <div className="bm-stat-item">
                        <span className="bm-stat-label">ETA</span>
                        <span className="bm-stat-value tabular-nums">{progress.eta}</span>
                    </div>
                </div>
            </div>
            <div className="bm-install-footer">
                <button className="bm-btn danger text-only" onClick={onCancel}>{t("InstallProgressBar.cancel")}</button>
            </div>
        </>
    );
});

const ErrorView: React.FC<{
    error: string;
    onRetry: () => void;
    onClose: () => void;
}> = React.memo(({ error, onRetry, onClose }) => {
    const { t } = useTranslation();
    return (
        <>
            <div className="bm-install-header">
                <div className="bm-icon-circle error">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <circle cx="12" cy="12" r="10"></circle>
                        <line x1="12" y1="8" x2="12" y2="12"></line>
                        <line x1="12" y1="16" x2="12.01" y2="16"></line>
                    </svg>
                </div>
                <h2 className="bm-install-title">{t("InstallProgressBar.error_title")}</h2>
            </div>
            <div className="bm-install-body">
                <div className="bm-error-container">
                    <pre className="bm-error-message">{error}</pre>
                </div>
            </div>
            <div className="bm-install-footer">
                <button className="bm-btn secondary" onClick={onClose}>{t("InstallProgressBar.close")}</button>
                <button className="bm-btn primary" onClick={onRetry}>{t("InstallProgressBar.retry")}</button>
            </div>
        </>
    );
});

// ================================================================================================
// 6. 主组件 (Main Component)
// ================================================================================================

const InstallProgressBar: React.FC<InstallProgressBarProps> = (props) => {
    const {
        version, packageId, md5, onStatusChange, onCompleted, onCancel,
        isImport = false, sourcePath = null, isGDK = false, autoExtractPath = null, forceDownload = false, children
    } = props;

    const [state, dispatch] = useReducer(reducer, initialState);
    const [isClosing, setIsClosing] = useState(false);
    const dialogRef = useRef<HTMLDialogElement>(null);
    const taskIdRef = useRef<string | null>(null);
    const isExtractingRef = useRef(false);
    const unlistenRef = useRef<Promise<UnlistenFn> | null>(null);
    // unused; kept to avoid noisy diffs in this file
    const autoExtractStartedRef = useRef(false);

    const [gdkCdnLoading, setGdkCdnLoading] = useState(false);
    const [gdkCdnError, setGdkCdnError] = useState<string | null>(null);
    const [gdkCdnResults, setGdkCdnResults] = useState<CdnProbeResult[]>([]);
    const [gdkSelectedCdnBase, setGdkSelectedCdnBase] = useState<string>("");
    const gdkSelectedCdnBaseRef = useRef<string>("");

    // 修复文件名
    useEffect(() => {
        const initFileName = async () => {
            let name = version || "";
            if (isImport && sourcePath) {
                try {
                    const fullBase = await basename(sourcePath);
                    name = fullBase.replace(/\.[^/.]+$/, "");
                } catch { /* fallback */ }
            }
            dispatch({ type: 'SET_FILENAME', payload: name });
        };
        initFileName();
    }, [version, isImport, sourcePath]);

    const gdkCdnBases = useRef<string[]>([
        "http://assets1.xboxlive.cn",
        "http://assets2.xboxlive.cn",
        "http://assets1.xboxlive.com",
        "http://assets2.xboxlive.com",
    ]);

    const getBaseFromUrl = (url: string): string => {
        try {
            const u = new URL(url);
            return `${u.protocol}//${u.host}`;
        } catch {
            return "";
        }
    };

    const applyCdnBase = (originalUrl: string, base: string): string => {
        const orig = new URL(originalUrl);
        const b = new URL(base);
        b.pathname = orig.pathname;
        b.search = orig.search;
        b.hash = "";
        return b.toString();
    };

    const refreshGdkCdn = useCallback(async () => {
        if (!isGDK || isImport || !packageId) return;
        setGdkCdnLoading(true);
        setGdkCdnError(null);
        try {
            const resp = await invoke<CdnProbeResponse>("probe_gdk_asset_cdns", {
                originalUrl: packageId,
                bases: gdkCdnBases.current,
            });
            const results = resp?.results || [];
            setGdkCdnResults(results);

            const recommended = resp?.recommended_base || "";
            const fallback = getBaseFromUrl(packageId);
            const picked = recommended || (results.find(r => r.latency_ms != null)?.base) || fallback || gdkCdnBases.current[0];
            setGdkSelectedCdnBase(picked);
            gdkSelectedCdnBaseRef.current = picked;
        } catch (e: any) {
            const fallback = getBaseFromUrl(packageId) || gdkCdnBases.current[0];
            setGdkSelectedCdnBase(fallback);
            gdkSelectedCdnBaseRef.current = fallback;
            setGdkCdnError(e?.message ? String(e.message) : String(e));
        } finally {
            setGdkCdnLoading(false);
        }
    }, [isGDK, isImport, packageId]);

    useEffect(() => {
        gdkSelectedCdnBaseRef.current = gdkSelectedCdnBase;
    }, [gdkSelectedCdnBase]);

    useEffect(() => {
        if (state.status !== 'confirming') return;
        if (!isGDK || isImport || !packageId) return;

        const base = getBaseFromUrl(packageId) || gdkCdnBases.current[0];
        setGdkSelectedCdnBase(base);
        gdkSelectedCdnBaseRef.current = base;
        setGdkCdnResults([]);
        setGdkCdnError(null);
        refreshGdkCdn();
    }, [state.status, isGDK, isImport, packageId, refreshGdkCdn]);

    // Dialog 控制
    useEffect(() => {
        const dialog = dialogRef.current;
        if (!dialog) return;
        if (state.status !== 'cancelled' && state.status !== 'completed' && !dialog.open) {
            dialog.showModal();
        }
    }, [state.status]);

    // 状态回调
    useEffect(() => {
        onStatusChange(['starting', 'progress'].includes(state.status));
        if (state.status === 'completed') onCompleted(packageId);
        if (state.status === 'cancelled') onCancel(packageId);
    }, [state.status, onStatusChange, onCompleted, onCancel, packageId]);

    // 清理
    useEffect(() => {
        return () => {
            if (unlistenRef.current) {
                unlistenRef.current.then(unlisten => unlisten());
                unlistenRef.current = null;
            }
        };
    }, []);

    // 任务更新处理
    const handleTaskUpdate = (snap: TaskSnapshot) => {
        if (snap.status === "completed" && isExtractingRef.current && snap.stage !== "extracting") {
            return;
        }

        dispatch({type: 'UPDATE_PROGRESS', payload: mapSnapshotToProgress(snap)});

        if (snap.status === "completed") {
            if (!isImport && !isExtractingRef.current) {
                if (snap.message) {
                    handleSwitchToExtract(snap.message);
                } else {
                    dispatch({ type: 'SET_ERROR', payload: "Download finished but no file path returned." });
                }
            } else {
                handleClose();
            }
        } else if (snap.status === "error") {
            dispatch({type: 'SET_ERROR', payload: snap.message || "Task failed"});
        } else if (snap.status === "cancelled") {
            handleClose(true);
        }
    };

    // 启动监听
    const startListening = async (taskId: string) => {
        const eventName = `task-update::${taskId}`;
        if (unlistenRef.current) {
            const oldUnlistenPromise = unlistenRef.current;
            unlistenRef.current = null;
            try { (await oldUnlistenPromise)(); } catch(e) {}
        }
        const unlistenPromise = listen<TaskSnapshot>(eventName, (event) => {
            handleTaskUpdate(event.payload);
        });
        unlistenRef.current = unlistenPromise;

        try {
            const initialSnap = await invoke<TaskSnapshot>("get_task_status", { taskId });
            if (initialSnap) handleTaskUpdate(initialSnap);
        } catch (e) { /* ignore */ }
    };

    // 切换到解压
    const handleSwitchToExtract = async (filePath: string) => {
        isExtractingRef.current = true;
        if (unlistenRef.current) {
            const oldUnlisten = unlistenRef.current;
            unlistenRef.current = null;
            (await oldUnlisten)();
        }

        dispatch({
            type: 'UPDATE_PROGRESS',
            payload: { stage: 'extracting', percent: 0, speed: '--', eta: '--', message: isGDK ? 'GDK Unpack...' : 'Extraction...' }
        });

        try {
            let extractTaskId: string;
            if (isGDK) {
                const folderName = state.fileName
                    .replace(/\.msixvc$/i, "")
                    .replace(/\.appx$/i, "")
                    .replace(/\.zip$/i, "");
                extractTaskId = await invoke("unpack_gdk", { inputPath: filePath, folderName: folderName });
            } else {
                extractTaskId = await invoke("extract_zip_appx", { fileName: state.fileName, destination: filePath, forceReplace: true, deleteSignature: true });
            }

            if (extractTaskId) {
                taskIdRef.current = extractTaskId;
                await startListening(extractTaskId);
            } else {
                throw new Error("Failed to start extraction task");
            }
        } catch (e: any) {
            dispatch({ type: 'SET_ERROR', payload: e.message || String(e) });
            isExtractingRef.current = false;
        }
    };

    // Local installs should still show the confirm view so users can edit the target name.

    // 初始启动逻辑
    useEffect(() => {
        if (state.status === 'starting') {
            const run = async () => {
                try {
                    isExtractingRef.current = false;

                    // Local install path: skip downloading, but still respect user-provided name.
                    if (!isImport && autoExtractPath) {
                        dispatch({ type: 'DOWNLOAD_STARTED' });
                        await handleSwitchToExtract(autoExtractPath);
                        return;
                    }

                    let safeName = state.fileName.trim();
                    while (safeName.endsWith('.')) safeName = safeName.slice(0, -1);
                    safeName = safeName.replace(/[\\/:*?"<>|]+/g, "_") || version;

                    let targetExt = isGDK ? '.msixvc' : '.appx';
                    if (isImport && sourcePath) {
                        const lowerSource = sourcePath.toLowerCase();
                        if (lowerSource.endsWith(".msixvc")) targetExt = ".msixvc";
                        else if (lowerSource.endsWith(".zip")) targetExt = ".zip";
                        else targetExt = ".appx";
                    }
                    if (!safeName.toLowerCase().endsWith(targetExt)) safeName += targetExt;

                    let taskId: string;
                    if (isImport && sourcePath) {
                        const isSourceGdk = sourcePath.toLowerCase().endsWith(".msixvc");
                        if (isSourceGdk) {
                            isExtractingRef.current = true;
                            const folderName = safeName.replace(/\.msixvc$/i, "");
                            taskId = await invoke("unpack_gdk", { inputPath: sourcePath, folderName: folderName });
                        } else {
                            taskId = await invoke("import_appx", { sourcePath, fileName: safeName });
                        }
                    } else if (isGDK) {
                        const base = gdkSelectedCdnBaseRef.current || getBaseFromUrl(packageId || "") || gdkCdnBases.current[0];
                        const url = applyCdnBase(String(packageId), base);
                        taskId = await invoke("download_resource", { url, fileName: safeName, md5, forceDownload });
                    } else {
                        const fullId = `${packageId}_1`;
                        taskId = await invoke("download_appx", { packageId: fullId, fileName: safeName, md5, forceDownload });
                    }

                    if (!taskId) throw new Error("Failed to get Task ID");
                    taskIdRef.current = taskId;
                    dispatch({ type: 'DOWNLOAD_STARTED' });
                    await startListening(taskId);
                } catch (e: any) {
                    dispatch({ type: 'SET_ERROR', payload: e.message || String(e) });
                }
            };
            run();
        }
    }, [state.status]);

    const handleClose = useCallback((isCancel = false) => {
        if (unlistenRef.current) {
            unlistenRef.current.then(unlisten => unlisten());
            unlistenRef.current = null;
        }
        const dialog = dialogRef.current;
        if (!dialog) return;

        setIsClosing(true);
        const onAnimationEnd = () => {
            dialog.close();
            setIsClosing(false);
            dispatch({ type: isCancel ? 'CONFIRM_CANCEL' : 'COMPLETE' });
            dialog.removeEventListener('animationend', onAnimationEnd);
        };
        dialog.addEventListener('animationend', onAnimationEnd);
    }, []);

    const handleCancelRequest = useCallback(async () => {
        if (unlistenRef.current) {
            (await unlistenRef.current)();
            unlistenRef.current = null;
        }
        if (taskIdRef.current) {
            try { await invoke("cancel_task", { taskId: taskIdRef.current }); } catch (e) { console.error(e); }
        }
        handleClose(true);
    }, [handleClose]);

    const renderContent = () => {
        return (
            <div className="bm-view-wrapper">
                {state.status === 'confirming' && (
                    (!isImport && autoExtractPath ? (
                        <LocalInstallConfirmView
                            fileName={state.fileName}
                            versionType={props.versionType}
                            isGdk={isGDK}
                            localPath={autoExtractPath}
                            onFileNameChange={(name) => dispatch({ type: 'SET_FILENAME', payload: name })}
                            onConfirm={() => dispatch({ type: 'START_DOWNLOAD' })}
                            onCancel={() => handleClose(true)}
                        />
                    ) : (
                        <ConfirmView
                            downloadVersion={version}
                            versionType={props.versionType}
                            fileName={state.fileName}
                            isImport={isImport}
                            isGdk={isGDK}
                            cdnLoading={gdkCdnLoading}
                            cdnError={gdkCdnError}
                            cdnResults={gdkCdnResults}
                            selectedCdnBase={gdkSelectedCdnBase}
                            onSelectCdnBase={(base) => setGdkSelectedCdnBase(base)}
                            onRefreshCdn={refreshGdkCdn}
                            onFileNameChange={(name) => dispatch({ type: 'SET_FILENAME', payload: name })}
                            onConfirm={() => dispatch({ type: 'START_DOWNLOAD' })}
                            onCancel={() => handleClose(true)}
                        />
                    ))
                )}
                {(state.status === 'starting' || state.status === 'progress') && (
                    <ProgressView
                        progress={state.progress}
                        onCancel={handleCancelRequest}
                    />
                )}
                {state.status === 'error' && (
                    <ErrorView
                        error={state.error!}
                        onRetry={() => dispatch({ type: 'RETRY' })}
                        onClose={() => handleClose(true)}
                    />
                )}
            </div>
        );
    };

    return (
        <>
            {children}
            {ReactDOM.createPortal(
                <dialog
                    ref={dialogRef}
                    className={`bm-install-dialog ${isClosing ? 'is-closing' : ''} ${isImport ? 'platform-import' : (isGDK ? 'platform-gdk' : 'platform-uwp')}`}
                    onCancel={(e) => { e.preventDefault(); handleCancelRequest(); }}
                >
                    {renderContent()}
                </dialog>,
                document.body
            )}
        </>
    );
};

export default InstallProgressBar;
