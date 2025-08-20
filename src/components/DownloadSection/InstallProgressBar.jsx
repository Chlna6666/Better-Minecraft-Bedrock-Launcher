import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { basename, extname } from "@tauri-apps/api/path";
import { useTranslation } from "react-i18next";
import "./InstallProgressBar.css";

const InstallProgressBar = ({
                                version,
                                packageId,
                                versionType,
                                onStatusChange,
                                onCompleted,
                                onCancel,
                                isImport = false,
                                sourcePath = null,
                                children,
                            }) => {
    const { t } = useTranslation();

    const [progressData, setProgressData] = useState({
        processed: 0,
        total: 0,
        speed: "0 B/s",
        eta: "00:00:00",
        percent: 0,
        status: null,
        stage: "downloading",
    });
    const [displayPercent, setDisplayPercent] = useState(0);
    const [isDownloading, setIsDownloading] = useState(false);
    const [isCancelling, setIsCancelling] = useState(false);
    const [error, setError] = useState(null);
    const [cachedData, setCachedData] = useState(null);

    const [confirmed, setConfirmed] = useState(false);
    const [fileNameInput, setFileNameInput] = useState(String(version || ""));
    const [originalExtension, setOriginalExtension] = useState("");
    const [open, setOpen] = useState(true);

    const unlistenRef = useRef(null);
    const startingRef = useRef(false);
    const finishedRef = useRef(false);
    const rafRef = useRef(null);
    const prevStageRef = useRef(null);

    // 过渡标记（切阶段时短暂忽略新阶段的 percent）
    const isTransitioningRef = useRef(false);

    useEffect(() => {
        cancelAnimationFrame(rafRef.current);
        const step = () => {
            setDisplayPercent((cur) => {
                const target = typeof progressData.percent === "number" ? progressData.percent : 0;
                const diff = target - cur;
                const delta = diff * 0.08;
                const next = Math.abs(diff) < 0.02 ? target : cur + delta;
                return Math.max(0, Math.min(100, next));
            });
            rafRef.current = requestAnimationFrame(step);
        };
        rafRef.current = requestAnimationFrame(step);
        return () => cancelAnimationFrame(rafRef.current);
    }, [progressData.percent]);

    useEffect(() => {
        async function setInitialFileName() {
            if (isImport && sourcePath) {
                const fullBase = await basename(sourcePath);
                const ext = await extname(sourcePath);
                const baseName = ext ? fullBase.slice(0, -(ext.length + 1)) : fullBase;
                setFileNameInput(baseName);
                setOriginalExtension(ext ? `.${ext}` : "");
            } else {
                setFileNameInput(String(version || ""));
                setOriginalExtension("");
            }
        }
        setInitialFileName();
    }, [isImport, sourcePath, version]);

    useEffect(() => {
        finishedRef.current = false;
        startingRef.current = false;
        cleanupListener();
        setError(null);
        setIsDownloading(false);
        setIsCancelling(false);

        setConfirmed(false);
        setOpen(true);

        setCachedData(null);
        setDisplayPercent(0);
        setProgressData((p) => ({
            ...p,
            percent: 0,
            processed: 0,
            total: 0,
            stage: isImport ? "importing" : "downloading",
        }));
        setOriginalExtension("");

        prevStageRef.current = null;
        isTransitioningRef.current = false;

        return () => {
            cleanupListener();
            startingRef.current = false;
            onStatusChange?.(false);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [packageId, isImport, sourcePath]);

    useEffect(() => {
        onStatusChange?.(isDownloading);
    }, [isDownloading, onStatusChange]);

    const cleanupListener = () => {
        if (unlistenRef.current) {
            try {
                unlistenRef.current();
            } catch (_) {}
            unlistenRef.current = null;
        }
    };

    const sanitizeFileName = (name) => {
        if (!name) return "";
        let s = String(name).trim();
        s = s.replace(/[\r\n\t]/g, "");
        s = s.replace(/[\\/:*?"<>|]+/g, "_");
        s = s.replace(/^[\s_]+|[\s_]+$/g, "");
        return s;
    };

    const startDownload = async (fileName) => {
        if (startingRef.current) return;
        startingRef.current = true;
        setError(null);
        setIsDownloading(true);

        let safe = sanitizeFileName(fileName || "");
        if (!safe) {
            safe = `${version}${isImport && originalExtension ? originalExtension : ".appx"}`;
        } else if (
            !safe.toLowerCase().endsWith(".appx") &&
            !safe.toLowerCase().endsWith(".zip")
        ) {
            safe = `${safe}${isImport && originalExtension ? originalExtension : ".appx"}`;
        }
        setCachedData({ fileName: safe });
        setProgressData((p) => ({
            ...p,
            stage: isImport ? "importing" : "downloading",
            percent: 0,
            processed: 0,
            total: 0,
        }));
        setDisplayPercent(0);

        try {
            const off = await listen("install-progress", (e) => {
                let {
                    processed = 0,
                    total = 0,
                    speed = "0 B/s",
                    eta = "00:00:00",
                    status,
                    percent = 0,
                    stage,
                } = e.payload || {};

                // import 的后端可能仍叫 downloading -> 统一成 importing
                if (isImport && stage === "downloading") stage = "importing";

                // percent 强制为数值并 clamp
                percent = Number(percent);
                if (!Number.isFinite(percent)) percent = 0;
                percent = Math.max(0, Math.min(100, percent));

                const prevStage = prevStageRef.current;
                if (prevStage && prevStage !== stage) {
                    const downloadStage = isImport ? "importing" : "downloading";
                    if (prevStage === downloadStage && stage === "extracting") {
                        // ---------- 关键修复 1 ----------
                        // 切换到 extracting 时同时把 processed/total/percent 都重置为 0
                        // 否则如果后端瞬间发下载完成的 processed===total，会显示“已下载”
                        isTransitioningRef.current = true;
                        setDisplayPercent(0);
                        setProgressData({
                            processed: 0,   // <- 置 0
                            total: 0,       // <- 置 0
                            speed,
                            eta,
                            percent: 0,
                            status,
                            stage,
                        });
                        prevStageRef.current = stage;
                        // 两帧后结束过渡，保证浏览器把 0% 渲染出来
                        requestAnimationFrame(() => {
                            requestAnimationFrame(() => {
                                isTransitioningRef.current = false;
                            });
                        });
                        return; // 忽略这次 payload 的 percent/processed（等解压真正上报数据）
                    }
                    prevStageRef.current = stage;
                } else if (!prevStageRef.current) {
                    prevStageRef.current = stage;
                }

                const effectivePercent = isTransitioningRef.current ? 0 : percent;

                // ---------- 小改：当 extracting 且后端没提供 total 时，不把先前下载 total 再显示 ----------
                // 如果当前阶段是 extracting 且后端发送的 total 与之前下载 total 相同（可能是残留），
                // 但 percent 为 0，则优先保留 0/0，避免误显示已下载。
                if (stage === "extracting" && isTransitioningRef.current) {
                    // 过渡期上面已经提前返回了，所以正常分支到这里时意味着不是过渡期间
                }

                setProgressData({
                    processed,
                    total,
                    speed,
                    eta,
                    percent: effectivePercent,
                    status,
                    stage,
                });

                if (status === "completed" && stage === "extracting" && !finishedRef.current) {
                    finishedRef.current = true;
                    cleanupListener();
                    setIsDownloading(false);
                    onStatusChange?.(false);
                    onCompleted?.(packageId);
                }
            });
            unlistenRef.current = off;

            if (isImport) {
                await invoke("import_appx", { sourcePath: sourcePath, fileName: safe });
            } else {
                const revision = "1";
                const fullId = `${packageId}_${revision}`;
                const result = await invoke("download_appx", { packageId: fullId, fileName: safe });
                if (result === "cancelled") {
                    if (!finishedRef.current) {
                        finishedRef.current = true;
                        onStatusChange?.(false);
                        onCancel?.(packageId);
                    }
                    return;
                }

                await invoke("extract_zip_appx", {
                    fileName: safe,
                    destination: result,
                    forceReplace: true,
                    deleteSignature: true,
                });
            }
        } catch (e) {
            console.error(e);
            setError(e?.message ?? String(e) ?? "Unknown error");
            setIsDownloading(false);
            cleanupListener();
            startingRef.current = false;
        } finally {
            cleanupListener();
            setIsDownloading(false);
            startingRef.current = false;
        }
    };

    const cancelInstall = async () => {
        if (!isDownloading && !error && !confirmed) {
            finishedRef.current = true;
            setOpen(false);
            onCancel?.(packageId);
            return;
        }

        if (!isDownloading && !error) return;
        setIsCancelling(true);
        try {
            await invoke("cancel_install", { fileName: cachedData?.fileName });
        } catch (e) {
            console.error(e);
        } finally {
            setIsCancelling(false);
            cleanupListener();
            setIsDownloading(false);

            if (!error) {
                setProgressData({
                    processed: 0,
                    total: 0,
                    speed: "0 B/s",
                    eta: "00:00:00",
                    percent: 0,
                });
                setDisplayPercent(0);
                setCachedData(null);
            }

            if (!finishedRef.current) {
                finishedRef.current = true;
                onStatusChange?.(false);
                onCancel?.(packageId);
            }
        }
    };

    const formatMB = (bytes) => ((bytes || 0) / 1e6).toFixed(2);

    const showModal = open;

    const handleConfirmStart = () => {
        setConfirmed(true);
        setOpen(true);
        onStatusChange?.(true);
        startDownload(fileNameInput);
    };

    const handleCancelBeforeStart = () => {
        setOpen(false);
        finishedRef.current = true;
        onCancel?.(packageId);
    };

    // 根据当前阶段选择取消按钮文案
    const cancelButtonText = isCancelling
        ? t("InstallProgressBar.cancelling")
        : progressData.stage === "extracting"
            ? t("InstallProgressBar.cancel") // 或者添加专门的键 InstallProgressBar.cancel_extracting
            : t("InstallProgressBar.cancel_download");

    return (
        <>
            <span style={{ cursor: isDownloading ? "default" : "pointer" }}>{children}</span>

            {showModal && (
                <div className="modal-overlay" role="dialog" aria-modal="true">
                    <div className={`install-modal ${error ? "modal-error" : ""}`} aria-live="polite">
                        {!confirmed && !error ? (
                            <>
                                <div className="modal-header">
                                    <div className="modal-title">{isImport ? t("InstallProgressBar.import_title") : t("InstallProgressBar.confirm_title")}</div>
                                    <div className="modal-subtitle">{isImport ? t("InstallProgressBar.import_sub",) : t("InstallProgressBar.confirm_sub")}</div>
                                </div>

                                <div className="download-progress-body">
                                    <label className="filename-label">{t("InstallProgressBar.filename_label")}</label>
                                    <input
                                        className="filename-input"
                                        value={fileNameInput}
                                        onChange={(e) => setFileNameInput(e.target.value)}
                                        onKeyDown={(e) => {
                                            if (e.key === "Enter") handleConfirmStart();
                                            if (e.key === "Escape") handleCancelBeforeStart();
                                        }}
                                    />
                                    <div className="confirm-actions">
                                        <button className="cancel-button" onClick={handleCancelBeforeStart}>
                                            {t("InstallProgressBar.cancel")}
                                        </button>
                                        <button className="retry-button" onClick={handleConfirmStart}>
                                            {isImport ? t("InstallProgressBar.start_import") : t("InstallProgressBar.start_download")}
                                        </button>
                                    </div>
                                </div>
                            </>
                        ) : null}

                        {(confirmed || error) && (
                            <>
                                <div className="modal-header">
                                    <div className="modal-title">
                                        {progressData.stage === "extracting"
                                            ? t("InstallProgressBar.stage_extracting")
                                            : isImport
                                                ? t("InstallProgressBar.stage_importing")
                                                : t("InstallProgressBar.stage_downloading")}
                                    </div>
                                    <div className="modal-subtitle">
                                        {progressData.stage === "extracting"
                                            ? t("InstallProgressBar.extracting_sub")
                                            : isImport
                                                ? t("InstallProgressBar.importing_sub")
                                                : t("InstallProgressBar.downloading_sub")}
                                    </div>
                                </div>

                                {error ? (
                                    <div className="download-progress-body">
                                        <div className="error-message" title={String(error)}>
                                            <pre className="error-text">{String(error)}</pre>
                                        </div>
                                        <div className="error-actions">
                                            <button className="cancel-button" onClick={cancelInstall} disabled={isCancelling}>
                                                {isCancelling ? t("InstallProgressBar.cancelling") : t("InstallProgressBar.close")}
                                            </button>
                                            <button className="retry-button" onClick={() => { setError(null); startDownload(cachedData?.fileName || fileNameInput); }} disabled={isCancelling}>
                                                {t("InstallProgressBar.retry")}
                                            </button>
                                        </div>
                                    </div>
                                ) : (
                                    <div className="download-progress-body">
                                        <div className="progress-row">
                                            <div className="progress-percentage">{displayPercent.toFixed(1)}%</div>
                                            <div className="progress-bar-outer" key={progressData.stage}>
                                                <div
                                                    className="progress-bar-inner"
                                                    style={{ width: `${Math.max(0, Math.min(100, displayPercent))}%` }}
                                                />
                                            </div>
                                        </div>

                                        <div className="progress-info-grid">
                                            <div className="info-block">
                                                <div className="info-label">{t("InstallProgressBar.processed_label")}</div>
                                                {/* ---------- 关键修复 2 ----------
                            当处于 extracting 而 total 为 0 时，显示占位而不是“已下载” */}
                                                <div className="info-value">
                                                    {progressData.stage === "extracting" && (!progressData.total || progressData.total === 0)
                                                        ? "—"
                                                        : `${formatMB(progressData.processed)} / ${formatMB(progressData.total)} MB`}
                                                </div>
                                            </div>
                                            <div className="info-block">
                                                <div className="info-label">{t("InstallProgressBar.speed_label")}</div>
                                                <div className="info-value">{progressData.speed} · {progressData.eta}</div>
                                            </div>
                                        </div>

                                        <div className="actions-row">
                                            <button className="cancel-button" onClick={cancelInstall} disabled={isCancelling}>
                                                {cancelButtonText}
                                            </button>
                                        </div>
                                    </div>
                                )}
                            </>
                        )}
                    </div>
                </div>
            )}
        </>
    );
};

export default InstallProgressBar;
