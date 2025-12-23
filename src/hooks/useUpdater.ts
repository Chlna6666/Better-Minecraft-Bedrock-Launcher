import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";

export interface TaskSnapshot {
    id: string;
    stage: string;
    total: number | null;
    done: number;
    speedBytesPerSec: number;
    eta: string;
    percent: number | null;
    status: string; // 'ready' | 'starting' | 'progress' | 'completed' | 'error' | 'cancelled'
    message: string | null;
}

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

interface UseUpdaterOptions {
    owner?: string;
    repo?: string;
    autoCheck?: boolean;
    autoCheckIntervalMs?: number;
}

export interface UpdaterState {
    checking: boolean;
    error: string | null;
    currentVersion: string | null;
    latestStable: ReleaseData | null;
    latestPrerelease: ReleaseData | null;
    selectedChannel: string | null;
    selectedRelease: ReleaseData | null;
    chosenRelease: ReleaseData | null;
    downloading: boolean;
    updateAvailable: boolean;
    progress: number | null;       // 简易进度 (0-100)
    progressSnapshot: TaskSnapshot | null; // 详细快照 (包含速度、ETA等)
    taskId: string | null;
}

export function useUpdater({
                               owner = "Chlna6666",
                               repo = "Better-Minecraft-Bedrock-Launcher",
                               autoCheck = true,
                               autoCheckIntervalMs = 0
                           }: UseUpdaterOptions = {}) {
    const [state, setState] = useState<UpdaterState>({
        checking: false,
        error: null,
        currentVersion: null,
        latestStable: null,
        latestPrerelease: null,
        selectedChannel: null,
        selectedRelease: null,
        chosenRelease: null,
        downloading: false,
        updateAvailable: false,
        progress: null,
        progressSnapshot: null,
        taskId: null,
    });

    const mounted = useRef(true);
    useEffect(() => {
        mounted.current = true;
        return () => { mounted.current = false; };
    }, []);


    useEffect(() => {
        let unlistenFn: UnlistenFn | null = null;

        const startListening = async () => {
            // 只有当正在下载且有 Task ID 时才开始监听
            if (!state.downloading || !state.taskId) {
                // 如果之前有监听器，清理掉
                if (unlistenFn) {
                    unlistenFn();
                    unlistenFn = null;
                }
                return;
            }

            const currentTaskId = state.taskId;
            const eventName = `task-update::${currentTaskId}`;
            console.log(`[useUpdater] 开始监听更新进度: ${eventName}`);

            // 1. 设置事件监听器
            unlistenFn = await listen<TaskSnapshot>(eventName, (event) => {
                if (!mounted.current) return;
                const snap = event.payload;

                setState(prev => {
                    // 只有当 ID 匹配时才更新 (防止竞态)
                    if (prev.taskId !== currentTaskId) return prev;
                    return {
                        ...prev,
                        progressSnapshot: snap,
                        progress: snap.percent ?? 0
                    };
                });
            });

            // 2. 初始状态拉取 (防止监听建立前的 GAP)
            try {
                const initialSnap = await invoke<TaskSnapshot>("get_task_status", { taskId: currentTaskId });
                if (mounted.current && initialSnap && state.taskId === currentTaskId) {
                    setState(prev => ({
                        ...prev,
                        progressSnapshot: initialSnap,
                        progress: initialSnap.percent ?? 0
                    }));
                }
            } catch (e) {
                console.warn("[useUpdater] 获取初始任务状态失败 (可能任务刚开始):", e);
            }
        };

        startListening();

        return () => {
            if (unlistenFn) unlistenFn();
        };
    }, [state.downloading, state.taskId]);

    const parseSemverSimple = (s: string | null | undefined) => {
        if (!s || typeof s !== "string") return null;
        const m = s.match(/(\d+)\.(\d+)\.(\d+)/);
        if (!m) return null;
        return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)];
    };

    const semverGreater = (aArr: number[], bArr: number[]) => {
        for (let i = 0; i < 3; i++) {
            if (aArr[i] > bArr[i]) return true;
            if (aArr[i] < bArr[i]) return false;
        }
        return false;
    };

    const checkForUpdates = useCallback(async () => {
        setState(prev => ({ ...prev, checking: true, error: null }));
        try {
            const resp = await invoke<any>("check_updates", { owner, repo });

            if (!mounted.current) return resp;

            const latestStable = resp.latest_stable || null;
            const latestPrerelease = resp.latest_prerelease || null;
            const selectedChannel = resp.selected_channel || null;

            let chosen: ReleaseData | null = null;
            if (resp.selected_release) {
                chosen = resp.selected_release;
            } else if (selectedChannel === "nightly") {
                chosen = latestPrerelease || latestStable || null;
            } else {
                chosen = latestStable || latestPrerelease || null;
            }

            let hasUpdate = false;
            if (typeof resp.update_available === "boolean") {
                hasUpdate = resp.update_available;
            } else {
                try {
                    const curArr = parseSemverSimple(resp.current_version);
                    const tag = chosen?.tag || chosen?.name || chosen?.asset_name;
                    const newArr = parseSemverSimple(tag);
                    if (curArr && newArr) {
                        hasUpdate = semverGreater(newArr, curArr);
                    } else {
                        hasUpdate = !!chosen;
                    }
                } catch { hasUpdate = false; }
            }

            setState(prev => ({
                ...prev,
                checking: false,
                currentVersion: resp.current_version || null,
                latestStable,
                latestPrerelease,
                selectedChannel,
                selectedRelease: resp.selected_release || null,
                chosenRelease: chosen,
                updateAvailable: hasUpdate
            }));

            return resp;
        } catch (e: any) {
            console.error("check_updates error:", e);
            if (mounted.current) {
                setState(prev => ({ ...prev, checking: false, error: String(e) }));
            }
            throw e;
        }
    }, [owner, repo]);

    useEffect(() => {
        if (autoCheck) {
            checkForUpdates().catch(() => {});
        }
        if (autoCheckIntervalMs > 0) {
            const id = setInterval(() => { checkForUpdates().catch(()=>{}); }, autoCheckIntervalMs);
            return () => clearInterval(id);
        }
    }, [autoCheck, autoCheckIntervalMs, checkForUpdates]);

    const downloadAndApply = useCallback(async (releaseSummary?: ReleaseData) => {
        const rs = releaseSummary || state.chosenRelease;
        if (!rs || !rs.asset_url) {
            throw new Error("releaseSummary or asset_url missing");
        }

        // 1. 生成 Task ID
        const taskId = `update-task-${Date.now()}-${Math.floor(Math.random() * 1000)}`;

        setState(prev => ({
            ...prev,
            downloading: true,
            error: null,
            progressSnapshot: null,
            progress: 0,
            taskId: taskId
        }));

        try {
            // 2. 调用 Rust 命令 (注意: 这个命令是阻塞的，直到下载完成或出错)
            // 但我们的 useEffect 会在后台并发处理事件监听
            await invoke("download_and_apply_update", {
                args: {
                    url: rs.asset_url,
                    filename_hint: rs.asset_name || null,
                    target_exe_path: "",
                    timeout_secs: 120,
                    auto_quit: true,
                    task_id: taskId
                }
            });

            // 下载成功，尝试退出
            try {
                await invoke("quit_app");
            } catch (qerr) {
                console.warn("quit_app failed:", qerr);
            }

            if (mounted.current) {
                setState(prev => ({ ...prev, downloading: false, progress: 100, taskId: null }));
            }
        } catch (e: any) {
            const errStr = String(e);
            console.error("download_and_apply_update error:", errStr);

            if (mounted.current) {
                if (errStr.toLowerCase().includes("cancelled") || errStr.toLowerCase().includes("取消")) {
                    setState(prev => ({ ...prev, downloading: false, taskId: null, progress: null }));
                } else {
                    setState(prev => ({ ...prev, downloading: false, error: errStr, taskId: null }));
                }
            }
        }
    }, [state.chosenRelease]);

    const cancelDownload = useCallback(async () => {
        const currentTaskId = state.taskId;
        if (!currentTaskId) {
            console.warn("No active task to cancel");
            return;
        }

        console.log("Cancelling task:", currentTaskId);
        try {
            await invoke("cancel_task", { taskId: currentTaskId });
            if (mounted.current) {
                setState(prev => ({
                    ...prev,
                    downloading: false,
                    progress: null,
                    taskId: null,
                    error: "Update cancelled by user"
                }));
            }
        } catch (e) {
            console.error("Failed to cancel task:", e);
        }
    }, [state.taskId]);

    return {
        state,
        actions: {
            checkForUpdates,
            downloadAndApply,
            cancelDownload
        },
    };
}