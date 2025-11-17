import React, { useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { basename, extname } from "@tauri-apps/api/path";
import { useTranslation } from "react-i18next";
import "./InstallProgressBar.css";

/**
 * InstallProgressBar (task_id polling version) - dialog 版（使用 <dialog> + portal）
 *
 * 后端命令假定与原组件一致（download_appx / import_appx / get_task_status / cancel_task / extract_zip_appx）
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

    // --- 状态（保持和你原来的一致） ---
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

    // dialog ref for native dialog
    const dialogRef = useRef(null);

    // animate percent (保持你原逻辑)
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
    const startPolling = (taskId, onCompletedCallback = null) => {
        stopPolling();
        currentTaskIdRef.current = taskId;

        // reset stage tracking / visual state for a fresh task
        prevStageRef.current = null;
        targetPercentRef.current = 0;
        setDisplayPercent(0);
        setProgressData((p) => ({
            ...p,
            percent: "0.00%",
            processed: 0,
            total: 0,
            speed: "0 B/s",
            eta: "00:00:00",
        }));
        setHasProgressEvent(false);

        pollIntervalRef.current = window.setInterval(async () => {
            try {
                // 只传 task_id（snake_case），不要重复其他 key
                const snap = await invoke("get_task_status", { task_id: taskId, taskId: taskId });

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

                if (id) {
                    currentTaskIdRef.current = id;
                }

                const numericDone = Number(done || 0);
                const numericTotal = Number(total || 0);
                const numericPercent = (typeof percent === "number") ? percent : (percent ? parseFloat(String(percent)) : null);

                // 如果阶段发生变化 -> 重置动画相关状态（从 0 开始）
                const prevStage = prevStageRef.current;
                if (prevStage !== stage) {
                    // stage changed (包括第一次 poll)，重置显示与目标
                    targetPercentRef.current = 0;
                    setDisplayPercent(0);
                    setHasProgressEvent(false);
                    // 也把进度计数清空，避免显示上个阶段的 processed/total
                    setProgressData((p) => ({
                        ...p,
                        processed: 0,
                        total: 0,
                        percent: "0.00%",
                        speed: "0 B/s",
                        eta: "00:00:00",
                        stage: stage || (isImport ? "importing" : "downloading"),
                    }));
                }
                // 更新 prevStage（无论是否相同）
                prevStageRef.current = stage || prevStageRef.current;

                const percentStr = (numericPercent !== null && numericPercent !== undefined && !Number.isNaN(numericPercent))
                    ? `${numericPercent.toFixed(2)}%`
                    : (numericTotal > 0 ? `${((numericDone / numericTotal) * 100).toFixed(2)}%` : "0.00%");

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

                const target = (numericPercent !== null && numericPercent !== undefined && !Number.isNaN(numericPercent))
                    ? numericPercent
                    : (numericTotal > 0 ? (numericDone / numericTotal * 100) : 0);
                targetPercentRef.current = Math.max(0, Math.min(100, target));

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
                                const extractTaskId = await invoke("extract_zip_appx", {
                                    fileName: cachedData?.fileName || fileNameInput,
                                    destination: destination,
                                    forceReplace: true,
                                    deleteSignature: true,
                                });

                                if (extractTaskId) {
                                    // startPolling 时会重置 prevStageRef 和显示状态
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

    // startDownload / cancelInstall 保持逻辑（仅略作小修改：保证 snake_case 调用）
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
        if (!isDownloading && !error && !confirmed) {
            finishedRef.current = true;
            setOpen(false);
            setConfirmed(false);
            setIsStarting(false);
            startingRef.current = false;
            stopPolling();
            onCancel?.(packageId);
            // close dialog if present
            if (dialogRef.current && typeof dialogRef.current.close === "function") {
                try { dialogRef.current.close(); } catch (e) {}
            }
            return;
        }

        if (!isDownloading && !error) return;

        setIsCancelling(true);
        try {
            const currentTaskId = currentTaskIdRef.current;
            if (currentTaskId) {
                // 使用 snake_case 参数名
                await invoke("cancel_task", { task_id: currentTaskId, taskId: currentTaskId });
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

    // 确认开始 / 取消前的处理
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
        // close native dialog if open
        if (dialogRef.current && typeof dialogRef.current.close === "function") {
            try { dialogRef.current.close(); } catch (e) {}
        }
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

    // ---- dialog open/close effect ----
    useEffect(() => {
        const dlg = dialogRef.current;
        if (!dlg) return;

        try {
            if (open) {
                // showModal 会抛出 error 如果已打开，catch 掉
                if (typeof dlg.showModal === "function" && !dlg.open) {
                    dlg.showModal();
                }
            } else {
                if (typeof dlg.close === "function" && dlg.open) {
                    dlg.close();
                }
            }
        } catch (e) {
            // 某些浏览器可能不支持 dialog
            console.warn("dialog.showModal error:", e);
        }

        // 绑定 cancel（Esc/backdrop）事件，阻止默认关闭并使用我们的逻辑
        const onCancelEvent = (ev) => {
            // ev.preventDefault(); // 如果不希望 dialog 自动关闭，可以 preventDefault 再做自定义
            // 这里我们允许默认关闭并走 cancelInstall 行为
            // 但为了避免重复调用（dialog 自动关闭同时 state 变化），我们直接调用 cancelInstall
            cancelInstall();
        };
        dlg.addEventListener && dlg.addEventListener("cancel", onCancelEvent);

        return () => {
            dlg.removeEventListener && dlg.removeEventListener("cancel", onCancelEvent);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [open, dialogRef.current]);

    // 根据浏览器能力选择渲染方式：如果支持 dialog 元素且 showModal 存在 -> 使用 dialog；否则退回 overlay DOM
    const supportsDialog = typeof HTMLDialogElement === "function" || (typeof document !== "undefined" && "showModal" in document.createElement("dialog"));

    // --- Modal content (抽取复用) ---
    const modalContent = (
        <div className={`install-modal ${error ? "modal-error" : ""}`} aria-live="polite" role="dialog">
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
    );

    // 挂载到 body 的 portal（如果支持 dialog，会把 modalContent 放进 dialog）
    return (
        <>
            <span style={{ cursor: isDownloading ? "default" : "pointer" }}>{children}</span>

            {supportsDialog ? ReactDOM.createPortal(
                <dialog
                    ref={dialogRef}
                    className="install-dialog"
                    aria-label={isImport ? t("InstallProgressBar.import_title") : t("InstallProgressBar.confirm_title")}
                >
                    {modalContent}
                </dialog>,
                document.body
            ) : ReactDOM.createPortal(
                // 回退 overlay（旧实现样式，保证兼容）
                <div className="modal-overlay" role="dialog" aria-modal="true">
                    {modalContent}
                </div>,
                document.body
            )}
        </>
    );
};

export default InstallProgressBar;
