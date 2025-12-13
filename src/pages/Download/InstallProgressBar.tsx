import React, {useCallback, useEffect, useReducer, useRef, useState} from "react";
import ReactDOM from "react-dom";
import {invoke} from "@tauri-apps/api/core";
import {basename, extname} from "@tauri-apps/api/path";
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
}

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
// 4. Hook: 动画逻辑
// ================================================================================================

const useAnimatedPercent = (targetPercent: number) => {
    const [displayPercent, setDisplayPercent] = useState(0);
    const rafRef = useRef<number | null>(null);
    const currentRef = useRef<number>(0);

    const cancel = () => {
        if (rafRef.current !== null) {
            cancelAnimationFrame(rafRef.current);
            rafRef.current = null;
        }
    };

    useEffect(() => {
        if (targetPercent === 0 || isNaN(targetPercent)) {
            cancel();
            currentRef.current = 0;
            setDisplayPercent(0);
            return;
        }
        if (targetPercent >= 100) {
            cancel();
            currentRef.current = 100;
            setDisplayPercent(100);
            return;
        }
        if (targetPercent < currentRef.current - 1e-6) {
            cancel();
            currentRef.current = targetPercent;
            setDisplayPercent(targetPercent);
            return;
        }
        const easingFactor = 0.28;
        const step = () => {
            const cur = currentRef.current;
            const diff = targetPercent - cur;
            if (Math.abs(diff) < 0.1) {
                currentRef.current = targetPercent;
                setDisplayPercent(targetPercent);
                rafRef.current = null;
                return;
            }
            const next = cur + diff * easingFactor;
            currentRef.current = next;
            setDisplayPercent(next);
            rafRef.current = requestAnimationFrame(step);
        };
        rafRef.current = requestAnimationFrame(step);
        return () => cancel();
    }, [targetPercent]);

    return displayPercent;
};

// ================================================================================================
// 5. Views
// ================================================================================================

const ConfirmView: React.FC<{
    fileName: string;
    isImport: boolean;
    onFileNameChange: (name: string) => void;
    onConfirm: () => void;
    onCancel: () => void;
}> = React.memo(({ fileName, isImport, onFileNameChange, onConfirm, onCancel }) => {
    const { t } = useTranslation();
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
                <div>
                    <h2 className="bm-install-title">{isImport ? t("InstallProgressBar.import_title") : t("InstallProgressBar.confirm_title")}</h2>
                    <p className="bm-install-subtitle">{isImport ? t("InstallProgressBar.import_sub") : t("InstallProgressBar.confirm_sub")}</p>
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

const ProgressView: React.FC<{
    progress: ProgressData;
    displayPercent: number;
    onCancel: () => void;
}> = React.memo(({ progress, displayPercent, onCancel }) => {
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
            <div className="bm-install-header left-align">
                <div>
                    <h2 className="bm-install-title">{title}</h2>
                    {detail && (
                        <p className="bm-install-subtitle" style={{ margin: 0, marginTop: '4px', opacity: 0.8, fontSize: '0.85rem' }}>
                            {detail}
                        </p>
                    )}
                </div>
                <span className="bm-percent-large tabular-nums">{displayPercent.toFixed(1)}%</span>
            </div>
            <div className="bm-install-body">
                <div className="bm-progress-track-wrapper">
                    <div className="bm-progress-bar-track">
                        <div className="bm-progress-bar-fill" style={{ width: `${displayPercent}%` }} />
                        <div className="bm-progress-bar-glow" style={{ width: `${displayPercent}%`, opacity: displayPercent > 0 ? 1 : 0 }} />
                    </div>
                </div>
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
        isImport = false, sourcePath = null, isGDK = false, children
    } = props;

    const [state, dispatch] = useReducer(reducer, initialState);
    const [isClosing, setIsClosing] = useState(false);
    const dialogRef = useRef<HTMLDialogElement>(null);
    const taskIdRef = useRef<string | null>(null);

    // 关键：防止重复进入解压阶段的 Flag
    const isExtractingRef = useRef(false);

    const unlistenRef = useRef<Promise<UnlistenFn> | null>(null);
    const animatedPercent = useAnimatedPercent(state.progress.percent);

    // 初始化文件名
    useEffect(() => {
        const initFileName = async () => {
            let name = version || "";
            if (isImport && sourcePath) {
                try {
                    const fullBase = await basename(sourcePath);
                    const ext = await extname(sourcePath);
                    name = ext ? fullBase.slice(0, -ext.length) : fullBase;
                } catch { /* fallback */ }
            }
            dispatch({ type: 'SET_FILENAME', payload: name });
        };
        initFileName();
    }, [version, isImport, sourcePath]);

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

    // --------------------------------------------------------------------------------------------
    // [逻辑核心] 处理任务更新和完成逻辑
    // --------------------------------------------------------------------------------------------
    const handleTaskUpdate = (snap: TaskSnapshot) => {
        // 如果我们已经在解压了，就忽略之前下载任务可能的重复完成信号
        if (snap.status === "completed" && isExtractingRef.current && snap.stage !== "extracting") {
            return;
        }

        dispatch({type: 'UPDATE_PROGRESS', payload: mapSnapshotToProgress(snap)});

        if (snap.status === "completed") {
            // 如果是下载任务完成（非导入模式，且还没开始解压）
            if (!isImport && !isExtractingRef.current) {
                // 关键检查：必须要有 message (路径) 才能解压
                if (snap.message) {
                    console.log("[InstallProgressBar] 下载完成，获取到文件路径，准备切换:", snap.message);
                    handleSwitchToExtract(snap.message);
                } else {
                    console.error("[InstallProgressBar] 错误: 下载显示完成，但没有返回文件路径(message为空)。请检查 Rust 后端 finish_task 调用。");
                    dispatch({ type: 'SET_ERROR', payload: "Download finished but no file path returned. (Backend Error)" });
                }
            } else {
                // 解压/解包任务完成，或导入任务完成
                console.log("[InstallProgressBar] 流程彻底完成");
                handleClose();
            }
        } else if (snap.status === "error") {
            dispatch({type: 'SET_ERROR', payload: snap.message || "Task failed"});
        } else if (snap.status === "cancelled") {
            handleClose(true);
        }
    };

    // --------------------------------------------------------------------------------------------
    // 启动监听
    // --------------------------------------------------------------------------------------------
    const startListening = async (taskId: string) => {
        const eventName = `task-update::${taskId}`;
        console.log(`[InstallProgressBar] 准备监听: ${eventName}`);

        if (unlistenRef.current) {
            const oldUnlistenPromise = unlistenRef.current;
            unlistenRef.current = null;
            try { (await oldUnlistenPromise)(); } catch(e) {}
        }

        // 1. 设置事件监听
        const unlistenPromise = listen<TaskSnapshot>(eventName, (event) => {
            handleTaskUpdate(event.payload);
        });
        unlistenRef.current = unlistenPromise;

        // 2. [修复 Gap] 拉取一次初始状态，处理“监听前已完成”的情况
        try {
            const initialSnap = await invoke<TaskSnapshot>("get_task_status", { taskId });
            if (initialSnap) {
                console.log("[InstallProgressBar] 初始状态:", initialSnap.status, initialSnap.message);
                handleTaskUpdate(initialSnap);
            }
        } catch (e) { /* ignore */ }
    };

    // --------------------------------------------------------------------------------------------
    // 切换到解压/解包任务
    // --------------------------------------------------------------------------------------------
    const handleSwitchToExtract = async (filePath: string) => {
        isExtractingRef.current = true;

        // 停止监听旧任务
        if (unlistenRef.current) {
            const oldUnlisten = unlistenRef.current;
            unlistenRef.current = null;
            (await oldUnlisten)();
        }

        // 暂时禁用动画以重置进度条
        const dialog = dialogRef.current;
        if (dialog) dialog.classList.add('no-transition');

        dispatch({
            type: 'UPDATE_PROGRESS',
            payload: {
                stage: 'extracting',
                percent: 0,
                speed: '--',
                eta: '--',
                message: isGDK ? 'Initializing GDK Unpack...' : 'Preparing extraction...'
            }
        });

        // 下一帧恢复动画
        requestAnimationFrame(() => {
            setTimeout(() => {
                if (dialog) dialog.classList.remove('no-transition');
            }, 16);
        });

        try {
            let extractTaskId: string;

            if (isGDK) {
                // --- GDK 分支 ---
                const folderName = state.fileName
                    .replace(/\.msixvc$/i, "")
                    .replace(/\.appx$/i, "")
                    .replace(/\.zip$/i, "");

                console.log("[InstallProgressBar] GDK 分支: 调用 unpack_gdk", { inputPath: filePath, folderName });
                extractTaskId = await invoke("unpack_gdk", {
                    inputPath: filePath,
                    folderName: folderName,
                });
            } else {
                // --- UWP 分支 ---
                console.log("[InstallProgressBar] UWP 分支: 调用 extract_zip_appx");
                extractTaskId = await invoke("extract_zip_appx", {
                    fileName: state.fileName,
                    destination: filePath,
                    forceReplace: true,
                    deleteSignature: true,
                });
            }

            if (extractTaskId) {
                taskIdRef.current = extractTaskId;
                await startListening(extractTaskId);
            } else {
                throw new Error("Failed to start extraction task");
            }
        } catch (e: any) {
            console.error("解压启动失败:", e);
            dispatch({ type: 'SET_ERROR', payload: e.message || String(e) });
            isExtractingRef.current = false;
            if (dialog) dialog.classList.remove('no-transition');
        }
    };

    // --------------------------------------------------------------------------------------------
    // 初始启动逻辑
    // --------------------------------------------------------------------------------------------
    useEffect(() => {
        if (state.status === 'starting') {
            const run = async () => {
                try {
                    isExtractingRef.current = false;

                    const ext = isGDK ? '.msixvc' : '.appx';
                    let safeName = state.fileName.trim().replace(/[\\/:*?"<>|]+/g, "_") || version;

                    // 处理后缀
                    if (isImport && sourcePath?.toLowerCase().endsWith(".zip")) {
                        safeName += ".zip";
                    } else if (!safeName.toLowerCase().endsWith(ext)) {
                        safeName += ext;
                    }

                    let taskId: string;

                    if (isImport && sourcePath) {
                        // 导入逻辑
                        const isSourceGdk = sourcePath.toLowerCase().endsWith(".msixvc");
                        if (isSourceGdk) {
                            isExtractingRef.current = true;
                            const folderName = safeName.replace(/\.msixvc$/i, "");
                            console.log("[InstallProgressBar] 导入 GDK: 直接解包", sourcePath);
                            taskId = await invoke("unpack_gdk", {
                                inputPath: sourcePath,
                                folderName: folderName
                            });
                        } else {
                            console.log("[InstallProgressBar] 导入 UWP/Zip");
                            taskId = await invoke("import_appx", { sourcePath, fileName: safeName });
                        }
                    } else if (isGDK) {
                        // 下载 GDK
                        console.log("[InstallProgressBar] 下载 GDK");
                        taskId = await invoke("download_resource", {
                            url: packageId,
                            fileName: safeName,
                            md5
                        });
                    } else {
                        // 下载 UWP
                        const fullId = `${packageId}_1`;
                        console.log("[InstallProgressBar] 下载 UWP APPX");
                        taskId = await invoke("download_appx", {
                            packageId: fullId,
                            fileName: safeName,
                            md5
                        });
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
                    <ConfirmView
                        fileName={state.fileName}
                        isImport={isImport}
                        onFileNameChange={(name) => dispatch({ type: 'SET_FILENAME', payload: name })}
                        onConfirm={() => dispatch({ type: 'START_DOWNLOAD' })}
                        onCancel={() => handleClose(true)}
                    />
                )}
                {(state.status === 'starting' || state.status === 'progress') && (
                    <ProgressView
                        progress={state.progress}
                        displayPercent={animatedPercent}
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
                    className={`bm-install-dialog ${isClosing ? 'is-closing' : ''}`}
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