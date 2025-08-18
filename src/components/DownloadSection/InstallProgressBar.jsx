import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import "./InstallProgressBar.css";

const InstallProgressBar = ({
                                version,
                                packageId,
                                versionType,
                                onStatusChange,
                                onCompleted,
                                onCancel,
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

    const unlistenRef = useRef(null);
    const startingRef = useRef(false);
    const finishedRef = useRef(false);
    const rafRef = useRef(null);
    const prevStageRef = useRef(null);

    // 平滑动画：displayPercent -> progressData.percent
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
        finishedRef.current = false;
        startDownload();
        return () => {
            cleanupListener();
            startingRef.current = false;
            onStatusChange?.(false);
        };
    }, [packageId]);

    useEffect(() => {
        onStatusChange?.(isDownloading);
    }, [isDownloading, onStatusChange]);

    const cleanupListener = () => {
        if (unlistenRef.current) {
            try { unlistenRef.current(); } catch (_) {}
            unlistenRef.current = null;
        }
    };

    const startDownload = async () => {
        if (startingRef.current) return;
        startingRef.current = true;
        setError(null);
        setIsDownloading(true);

        const revision = "1";
        const fullId = `${packageId}_${revision}`;
        const fileName = `${version}.appx`;
        setCachedData({ fullId, fileName });
        setProgressData((p) => ({ ...p, stage: "downloading", percent: 0, processed: 0, total: 0 }));
        setDisplayPercent(0);

        try {
            const off = await listen("install-progress", (e) => {
                const {
                    processed = 0,
                    total = 0,
                    speed = "0 B/s",
                    eta = "00:00:00",
                    status,
                    percent = 0,
                    stage,
                } = e.payload || {};

                // 阶段切换时立即重置 progress
                if (prevStageRef.current && prevStageRef.current !== stage) {
                    if (prevStageRef.current === "downloading" && stage === "extracting") {
                        cancelAnimationFrame(rafRef.current); // 停止动画
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

            const result = await invoke("download_appx", { packageId: fullId, fileName });
            if (result === "cancelled") {
                if (!finishedRef.current) {
                    finishedRef.current = true;
                    onStatusChange?.(false);
                    onCancel?.(packageId);
                }
                return;
            }

            await invoke("extract_zip_appx", {
                fileName,
                destination: result,
                forceReplace: true,
                deleteSignature: true,
            });
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
    const showModal = isDownloading || error || (typeof progressData.percent === "number" && progressData.percent > 0);

    return (
        <>
            <span style={{ cursor: isDownloading ? "default" : "pointer" }}>{children}</span>

            {showModal && (
                <div className="modal-overlay" role="dialog" aria-modal="true">
                    <div className={`install-modal ${error ? "modal-error" : ""}`} aria-live="polite">
                        <div className="modal-header">
                            <div className="modal-title">
                                {progressData.stage === "extracting"
                                    ? t("InstallProgressBar.stage_extracting")
                                    : t("InstallProgressBar.stage_downloading")}
                            </div>
                            <div className="modal-subtitle">
                                {progressData.stage === "extracting"
                                    ? t("InstallProgressBar.extracting_sub")
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
                                    <button className="retry-button" onClick={() => { setError(null); startDownload(); }} disabled={isCancelling}>
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
                    </div>
                </div>
            )}
        </>
    );
};

export default InstallProgressBar;
