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
                                md5 = null,
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
        percent: "0.00%", // ensure string format
        status: null,
        stage: "downloading",
    });
    const [displayPercent, setDisplayPercent] = useState(0);

    const [isStarting, setIsStarting] = useState(false);
    const [isDownloading, setIsDownloading] = useState(false);
    const [isCancelling, setIsCancelling] = useState(false);
    const [error, setError] = useState(null);
    const [cachedData, setCachedData] = useState(null);

    const [confirmed, setConfirmed] = useState(false);
    const [fileNameInput, setFileNameInput] = useState(String(version || ""));
    const [originalExtension, setOriginalExtension] = useState("");
    const [open, setOpen] = useState(true);

    // 标志：是否已收到第一个进度事件（用于 header 文案切换）
    const [hasProgressEvent, setHasProgressEvent] = useState(false);

    const startingRef = useRef(false);
    const unlistenRef = useRef(null);
    const finishedRef = useRef(false);
    const rafRef = useRef(null);
    const prevStageRef = useRef(null);

    const isTransitioningRef = useRef(false);
    const listenerCounterRef = useRef(0);
    const activeListenerTokenRef = useRef(0);
    const targetPercentRef = useRef(0);

    useEffect(() => {
        const parsed = parseFloat(String(progressData.percent).replace("%", "")) || 0;
        targetPercentRef.current = parsed;
    }, [progressData.percent]);

    useEffect(() => {
        const step = () => {
            setDisplayPercent((cur) => {
                const target = Number(targetPercentRef.current) || 0;
                const diff = target - cur;
                const delta = diff * 0.08;
                return Math.abs(diff) < 0.01 ? target : cur + delta;
            });
            rafRef.current = requestAnimationFrame(step);
        };
        rafRef.current = requestAnimationFrame(step);
        return () => cancelAnimationFrame(rafRef.current);
    }, []);

    useEffect(() => {
        async function setInitialFileName() {
            if (isImport && sourcePath) {
                try {
                    const fullBase = await basename(sourcePath);
                    const ext = await extname(sourcePath);
                    const baseName = ext ? fullBase.slice(0, -(ext.length + 1)) : fullBase;
                    setFileNameInput(baseName);
                    setOriginalExtension(ext ? `.${ext}` : "");
                } catch (e) {
                    setFileNameInput(String(version || ""));
                    setOriginalExtension("");
                }
            } else {
                setFileNameInput(String(version || ""));
                setOriginalExtension("");
            }
        }
        setInitialFileName();
    }, [isImport, sourcePath, version]);

    const cleanupListener = async () => {
        if (unlistenRef.current) {
            try {
                const res = unlistenRef.current();
                if (res && typeof res.then === "function") {
                    await res;
                }
            } catch (e) {
                console.warn("cleanupListener error:", e);
            } finally {
                unlistenRef.current = null;
            }
        }
    };

    useEffect(() => {
        finishedRef.current = false;
        startingRef.current = false;
        setIsStarting(false);
        cleanupListener().catch(() => {});
        setError(null);
        setIsDownloading(false);
        setIsCancelling(false);

        setConfirmed(false);
        setOpen(true);

        setCachedData(null);
        setDisplayPercent(0);
        setProgressData((p) => ({
            ...p,
            percent: "0.00%",
            processed: 0,
            total: 0,
            stage: isImport ? "importing" : "downloading",
        }));
        setOriginalExtension("");

        prevStageRef.current = null;
        isTransitioningRef.current = false;

        setHasProgressEvent(false);

        return () => {
            cleanupListener().catch(() => {});
            startingRef.current = false;
            setIsStarting(false);
            onStatusChange?.(false);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [packageId, isImport, sourcePath]);

    useEffect(() => {
        onStatusChange?.(isDownloading);
    }, [isDownloading, onStatusChange]);

    const sanitizeFileName = (name) => {
        if (!name) return "";
        let s = String(name).trim();
        s = s.replace(/[\r\n\t]/g, "");
        s = s.replace(/[\\/:*?"<>|]+/g, "_");
        s = s.replace(/^[\s_]+|[\s_]+$/g, "");
        return s;
    };

    const startDownload = async (fileName) => {
        if (!startingRef.current) {
            startingRef.current = true;
            setIsStarting(true);
        }

        setError(null);
        setIsDownloading(true);
        setHasProgressEvent(false);

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
            percent: "0.00%",
            processed: 0,
            total: 0,
        }));
        setDisplayPercent(0);

        try {
            cleanupListener().catch(() => {});

            const myToken = ++listenerCounterRef.current;
            activeListenerTokenRef.current = myToken;

            const off = await listen("install-progress", (e) => {
                if (myToken !== activeListenerTokenRef.current) return;
                if (finishedRef.current) return;

                let {
                    processed = 0,
                    total = 0,
                    speed = "0 B/s",
                    eta = "00:00:00",
                    status,
                    percent = "0.00%",
                    stage,
                } = e.payload || {};

                if (isImport && stage === "downloading") stage = "importing";
                const numericPercent = parseFloat(String(percent).replace("%", "")) || 0;

                if (!hasProgressEvent) {
                    setHasProgressEvent(true);
                }

                const prevStage = prevStageRef.current;
                if (prevStage && prevStage !== stage) {
                    const downloadStage = isImport ? "importing" : "downloading";
                    if (prevStage === downloadStage && stage === "extracting") {
                        isTransitioningRef.current = true;
                        setDisplayPercent(0);
                        setProgressData({
                            processed: 0,
                            total: 0,
                            speed,
                            eta,
                            percent: `${numericPercent.toFixed(2)}%`,
                            status,
                            stage,
                        });
                        prevStageRef.current = stage;
                        requestAnimationFrame(() => {
                            requestAnimationFrame(() => {
                                isTransitioningRef.current = false;
                            });
                        });
                        return;
                    }
                    prevStageRef.current = stage;
                } else if (!prevStageRef.current) {
                    prevStageRef.current = stage;
                }

                const effectivePercent = isTransitioningRef.current ? 0 : numericPercent;

                setProgressData({
                    processed,
                    total,
                    speed,
                    eta,
                    percent: `${numericPercent.toFixed(2)}%`,
                    status,
                    stage,
                });

                targetPercentRef.current = effectivePercent;

                if (status === "completed" && stage === "extracting" && !finishedRef.current) {
                    finishedRef.current = true;
                    cleanupListener().catch(() => {});
                    setIsDownloading(false);
                    setIsStarting(false);
                    startingRef.current = false;
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
                const result = await invoke("download_appx", { packageId: fullId, fileName: safe, md5: md5 });
                if (result === "cancelled") {
                    if (!finishedRef.current) {
                        finishedRef.current = true;
                        setIsDownloading(false);
                        setIsStarting(false);
                        startingRef.current = false;
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
            setIsStarting(false);
            startingRef.current = false;
            cleanupListener().catch(() => {});
        } finally {
            cleanupListener().catch(() => {});
            setIsDownloading(false);
            setIsStarting(false);
            startingRef.current = false;
        }
    };

    const cancelInstall = async () => {
        if (!isDownloading && !error && !confirmed) {
            finishedRef.current = true;
            setOpen(false);
            setConfirmed(false);
            setIsStarting(false);
            startingRef.current = false;
            activeListenerTokenRef.current = 0;
            cleanupListener().catch(() => {});
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
            activeListenerTokenRef.current = 0;
            cleanupListener().catch(() => {});
            setIsDownloading(false);
            setIsStarting(false);
            startingRef.current = false;

            if (!error) {
                setProgressData({
                    processed: 0,
                    total: 0,
                    speed: "0 B/s",
                    eta: "00:00:00",
                    percent: "0.00%",
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
        if (isStarting) return;
        setIsStarting(true);
        startingRef.current = true;
        setConfirmed(true);
        setOpen(true);
        onStatusChange?.(true);
        startDownload(fileNameInput);
    };

    const handleCancelBeforeStart = () => {
        setOpen(false);
        finishedRef.current = true;
        setConfirmed(false);
        setIsStarting(false);
        startingRef.current = false;
        activeListenerTokenRef.current = 0;
        cleanupListener().catch(() => {});
        onCancel?.(packageId);
    };

    const cancelButtonText = isCancelling
        ? t("InstallProgressBar.cancelling")
        : progressData.stage === "extracting"
            ? t("InstallProgressBar.cancel")
            : t("InstallProgressBar.cancel_download");

    const processedLabel = (() => {
        if (progressData.stage === "extracting") {
            return t("InstallProgressBar.extracted_label");
        }
        if (progressData.stage === "importing") {
            return t("InstallProgressBar.imported_label");
        }
        return t("InstallProgressBar.processed_label");
    })();

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
                                    <div className="modal-subtitle">{isImport ? t("InstallProgressBar.import_sub") : t("InstallProgressBar.confirm_sub")}</div>
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
                                        <button className="cancel-button" onClick={handleCancelBeforeStart} disabled={isStarting}>
                                            {t("InstallProgressBar.cancel")}
                                        </button>
                                        <button className="retry-button" onClick={handleConfirmStart} disabled={isStarting}>
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
                                                // 若是下载阶段并且仍未收到第一个进度事件，则显示 parsing_url
                                                : (progressData.stage === "downloading" && isDownloading && !hasProgressEvent)
                                                    ? t("InstallProgressBar.parsing_url") || "正在解析URL"
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
                                            <button
                                                className="retry-button"
                                                onClick={() => {
                                                    if (isStarting) return;
                                                    setIsStarting(true);
                                                    startingRef.current = true;
                                                    setError(null);
                                                    startDownload(cachedData?.fileName || fileNameInput);
                                                }}
                                                disabled={isCancelling || isStarting}
                                            >
                                                {t("InstallProgressBar.retry")}
                                            </button>
                                        </div>
                                    </div>
                                ) : (
                                    <div className="download-progress-body">
                                        <div className="progress-row">
                                            <div className="progress-percentage">
                                                {/* 百分比区域恢复为始终显示 percent */}
                                                {progressData.percent}
                                            </div>
                                            <div className="progress-bar-outer" key={progressData.stage}>
                                                <div
                                                    className="progress-bar-inner"
                                                    style={{ width: `${Math.max(0, Math.min(100, displayPercent))}%` }}
                                                />
                                            </div>
                                        </div>

                                        <div className="progress-info-grid">
                                            <div className="info-block">
                                                <div className="info-label">{processedLabel}</div>
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
