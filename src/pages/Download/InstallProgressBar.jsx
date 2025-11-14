import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { basename, extname } from "@tauri-apps/api/path";
import { useTranslation } from "react-i18next";
import "./InstallProgressBar.css";

/**
 * InstallProgressBar (task_id polling version)
 *
 * 必需后端命令：
 * - download_appx({ packageId, fileName, md5 }) -> returns task_id (String)
 * - import_appx({ sourcePath, fileName }) -> returns task_id (String)
 * - get_task_status({ taskId }) -> returns TaskSnapshot (serialized JSON)
 * - cancel_task({ taskId }) -> Result
 * - extract_zip_appx({ fileName, destination, forceReplace, deleteSignature }) -> Result (optional, used for download->extract step)
 *
 * 约定：后端在 download 完成时应把下载后的本地路径写入 TaskSnapshot.message（string），前端从 message 读取 destination。
 */

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
        percent: "0.00%",
        status: null,
        stage: isImport ? "importing" : "downloading",
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

    const [hasProgressEvent, setHasProgressEvent] = useState(false);

    // refs
    const startingRef = useRef(false);
    const finishedRef = useRef(false);
    const rafRef = useRef(null);
    const prevStageRef = useRef(null);
    const pollIntervalRef = useRef(null);
    const currentTaskIdRef = useRef(null);
    const targetPercentRef = useRef(0);

    // animate percent
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

    // set initial file name when importing
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

    // cleanup polling
    const stopPolling = () => {
        if (pollIntervalRef.current !== null) {
            clearInterval(pollIntervalRef.current);
            pollIntervalRef.current = null;
        }
        currentTaskIdRef.current = null;
    };

    useEffect(() => {
        // reset UI when packageId / mode change
        finishedRef.current = false;
        startingRef.current = false;
        setIsStarting(false);
        stopPolling();
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
        setHasProgressEvent(false);

        return () => {
            stopPolling();
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

    const formatSpeed = (bytesPerSec) => {
        if (!bytesPerSec || isNaN(bytesPerSec) || bytesPerSec <= 0) return "0 B/s";
        const units = ["B/s", "KB/s", "MB/s", "GB/s"];
        let v = bytesPerSec;
        let i = 0;
        while (v >= 1024 && i < units.length - 1) {
            v /= 1024;
            i++;
        }
        return `${v.toFixed(2)} ${units[i]}`;
    };

    const formatPercentFromSnapshot = (snap) => {
        if (snap.percent !== null && snap.percent !== undefined) {
            // if backend returns percent as number (0..100)
            const p = typeof snap.percent === "number" ? snap.percent : parseFloat(String(snap.percent) || "0");
            return `${p.toFixed(2)}%`;
        }
        if (snap.total && snap.total > 0) {
            const p = (snap.done / snap.total) * 100;
            return `${p.toFixed(2)}%`;
        }
        return "0.00%";
    };

    // poll snapshot every 500ms (adjustable)
// ---------- startPolling (替换原来函数体内实现) ----------
    const startPolling = (taskId, onCompletedCallback = null) => {
        stopPolling();
        // 先把传入 id 暂时保存（后面会以 snap.id 为准）
        currentTaskIdRef.current = taskId;
        pollIntervalRef.current = window.setInterval(async () => {
            try {
                // 只传 task_id（snake_case），不要重复其他 key
                const snap = await invoke("get_task_status", { task_id: taskId, taskId: taskId })

                // debug 日志（便于定位）——可以在 console 中观察
                console.debug("poll snap:", snap);

                const {
                    id,
                    stage,
                    total,
                    done,
                    speed_bytes_per_sec,
                    eta,
                    percent,
                    status,
                    message,
                } = snap || {};

                // 如果后端返回了 id，优先使用它（更新 currentTaskId）
                if (id) {
                    currentTaskIdRef.current = id;
                }

                // 防守式数值转换
                const numericDone = Number(done || 0);
                const numericTotal = Number(total || 0);
                const numericPercent = (typeof percent === "number") ? percent : (percent ? parseFloat(String(percent)) : null);

                // 标记已收到事件
                if (!hasProgressEvent) setHasProgressEvent(true);

                // 统一生成 percentStr（优先使用 numericPercent）
                const percentStr = (numericPercent !== null && numericPercent !== undefined && !Number.isNaN(numericPercent))
                    ? `${numericPercent.toFixed(2)}%`
                    : (numericTotal > 0 ? `${((numericDone / numericTotal) * 100).toFixed(2)}%` : "0.00%");

                // 更新 UI 数据
                setProgressData({
                    processed: numericDone,
                    total: numericTotal,
                    speed: formatSpeed(Number(speed_bytes_per_sec || 0)),
                    eta: eta || "unknown",
                    percent: percentStr,
                    status: status || null,
                    stage: stage || (isImport ? "importing" : "downloading"),
                    message: message || null,
                });

                // 把动画目标设置为数值（0..100）
                const target = (numericPercent !== null && numericPercent !== undefined && !Number.isNaN(numericPercent))
                    ? numericPercent
                    : (numericTotal > 0 ? (numericDone / numericTotal * 100) : 0);
                targetPercentRef.current = Math.max(0, Math.min(100, target));

                // 阶段切换逻辑：不要硬把 displayPercent 置 0（移除 setDisplayPercent(0)）
                const prevStage = prevStageRef.current;
                if (prevStage && prevStage !== stage) {
                    const downloadStage = isImport ? "importing" : "downloading";
                    if (prevStage === downloadStage && stage === "extracting") {
                        // 不要把进度条硬清零，保留平滑过渡：
                        // setDisplayPercent(0);  <-- 删除这行会避免进度条消失
                    }
                    prevStageRef.current = stage;
                } else if (!prevStageRef.current) {
                    prevStageRef.current = stage;
                }

                // 终态处理
                if (status === "completed") {
                    stopPolling();
                    finishedRef.current = true;
                    setIsDownloading(false);
                    setIsStarting(false);
                    startingRef.current = false;
                    onStatusChange?.(false);

                    if (!isImport) {
                        const destination = message;
                        if (destination) {
                            try {
                                // 注意：传入 snake_case 名称与 Rust 绑定一致
                                const extractTaskId = await invoke("extract_zip_appx", {
                                    fileName: cachedData?.fileName || fileNameInput,
                                    destination: destination,
                                    forceReplace: true,
                                    deleteSignature: true,
                                });

                                // 如果返回了 taskId（字符串），开始轮询解压任务
                                if (extractTaskId) {
                                    // 重要：确保 currentTaskIdRef 更新为 extractTaskId
                                    startPolling(String(extractTaskId));
                                } else {
                                    onCompleted?.(packageId);
                                }
                            } catch (e) {
                                console.error("extract_zip_appx error:", e);
                                setError(e?.message ?? String(e) ?? "extract error");
                            }
                        } else {
                            onCompleted?.(packageId);
                        }
                    } else {
                        onCompleted?.(packageId);
                    }
                } else if (status === "cancelled") {
                    stopPolling();
                    finishedRef.current = true;
                    setIsDownloading(false);
                    setIsStarting(false);
                    startingRef.current = false;
                    onStatusChange?.(false);
                    onCancel?.(packageId);
                } else if (status === "error") {
                    stopPolling();
                    finishedRef.current = true;
                    setIsDownloading(false);
                    setIsStarting(false);
                    startingRef.current = false;
                    setError(message || "task error");
                    onStatusChange?.(false);
                }
            } catch (pollErr) {
                console.error("poll get_task_status error:", pollErr);
                setError(String(pollErr));
                stopPolling();
                setIsDownloading(false);
                setIsStarting(false);
                startingRef.current = false;
            }
        }, 500);
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
            if (isImport) {
                // 注意：键名必须是 snake_case，和 Rust 函数参数名一致
                const taskId = await invoke("import_appx", {
                    source_path: sourcePath,
                    sourcePath: sourcePath,
                    file_name: safe,
                    fileName: safe,
                });
                if (!taskId) throw new Error("no task id returned from import_appx");
                startPolling(taskId);
            } else {
                const revision = "1";
                const fullId = `${packageId}_${revision}`;
                const taskId = await invoke("download_appx", {
                    packageId: fullId,
                    fileName: safe,
                    md5: md5,
                });
                if (!taskId) throw new Error("no task id returned from download_appx");
                startPolling(taskId);
            }
        } catch (e) {
            console.error(e);
            setError(e?.message ?? String(e) ?? "Unknown error");
            setIsDownloading(false);
            setIsStarting(false);
            startingRef.current = false;
            stopPolling();
        }
    };


    const cancelInstall = async () => {
        // if hasn't started we just close
        if (!isDownloading && !error && !confirmed) {
            finishedRef.current = true;
            setOpen(false);
            setConfirmed(false);
            setIsStarting(false);
            startingRef.current = false;
            stopPolling();
            onCancel?.(packageId);
            return;
        }

        if (!isDownloading && !error) return;

        setIsCancelling(true);
        try {
            const currentTaskId = currentTaskIdRef.current;
            if (currentTaskId) {
                await invoke("cancel_task", { taskId: currentTaskId });
                console.log("Cancelling taskId:", currentTaskIdRef.current);

            }
        } catch (e) {
            console.error("cancel error:", e);
        } finally {
            setIsCancelling(false);
            stopPolling();
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
                    status: null,
                    stage: isImport ? "importing" : "downloading",
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
        stopPolling();
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
