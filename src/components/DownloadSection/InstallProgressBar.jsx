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
        setProgressData((p) => ({ ...p, percent: 0, processed: 0, total: 0, stage: isImport ? "importing" : "downloading" }));
        setOriginalExtension("");

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
            try { unlistenRef.current(); } catch (_) {}
            unlistenRef.current = null;
        }
    };

    // 允许 Windows 支持的文件名字符
    const sanitizeFileName = (name) => {
        if (!name) return "";
        let s = String(name).trim();
        // 移除换行、制表符
        s = s.replace(/[\r\n\t]/g, "");
        // 只替换 Windows 不允许的字符
        s = s.replace(/[\\/:*?"<>|]+/g, "_");
        // 再去掉首尾空格或下划线
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
        } else if (!safe.toLowerCase().endsWith(".appx") && !safe.toLowerCase().endsWith(".zip")) {
            safe = `${safe}${isImport && originalExtension ? originalExtension : ".appx"}`;
        }
        setCachedData({ fileName: safe });
        setProgressData((p) => ({ ...p, stage: isImport ? "importing" : "downloading", percent: 0, processed: 0, total: 0 }));
        setDisplayPercent(0);

        try {
            const off = await listen("install-progress", (e) => {
                let { processed = 0, total = 0, speed = "0 B/s", eta = "00:00:00", status, percent = 0, stage } = e.payload || {};
                if (isImport && stage === "downloading") {
                    stage = "importing";
                }

                if (prevStageRef.current && prevStageRef.current !== stage) {
                    if (prevStageRef.current === (isImport ? "importing" : "downloading") && stage === "extracting") {
                        cancelAnimationFrame(rafRef.current);
                        setDisplayPercent(0);
                        setProgressData({
                            processed,
                            total,
                            speed,
                            eta,
                            percent: 0,
                            status,
                            stage,
                        });
                        prevStageRef.current = stage;
                        return;
                    }
                }

                prevStageRef.current = stage;
                setProgressData({ processed, total, speed, eta, percent, status, stage });

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
        } catch (e) { console.error(e); }
        finally {
            setIsCancelling(false);
            cleanupListener();
            setIsDownloading(false);

            if (!error) {
                setProgressData({ processed: 0, total: 0, speed: "0 B/s", eta: "00:00:00", percent: 0 });
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
                                                <div className="info-value">{`${formatMB(progressData.processed)} / ${formatMB(progressData.total)} MB`}</div>
                                            </div>
                                            <div className="info-block">
                                                <div className="info-label">{t("InstallProgressBar.speed_label")}</div>
                                                <div className="info-value">{progressData.speed} · {progressData.eta}</div>
                                            </div>
                                        </div>

                                        <div className="actions-row">
                                            <button className="cancel-button" onClick={cancelInstall} disabled={isCancelling}>
                                                {isCancelling ? t("InstallProgressBar.cancelling") : t("InstallProgressBar.cancel_download")}
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