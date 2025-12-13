// src/hooks/useUpdater.ts
import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
// @ts-ignore
import {ReleaseData, TaskSnapshot, UpdaterState} from "../types/updater";

interface UseUpdaterOptions {
    owner?: string;
    repo?: string;
    autoCheck?: boolean;
    autoCheckIntervalMs?: number;
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
    progress: number | null;
    progressSnapshot: null, // 初始化为 null
    taskId: string | null; // 新增：保存当前下载任务的ID
}

export function useUpdater({
                               owner = "Chlna6666",
                               repo = "Better-Minecraft-Bedrock-Launcher",
                               autoCheck = true,
                               autoCheckIntervalMs = 0
                           }: UseUpdaterOptions = {}) {
    // 状态整合
    // @ts-ignore
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
        progressSnapshot: null, // 初始化为 null
        taskId: null, // 初始化为空
    });

    const mounted = useRef(true);
    useEffect(() => {
        mounted.current = true;
        return () => { mounted.current = false; };
    }, []);

    useEffect(() => {
        let intervalId: any = null;

        if (state.downloading && state.taskId) {
            // 定义轮询函数
            const fetchStatus = async () => {
                try {
                    const snapshot = await invoke<TaskSnapshot>("get_task_status", {
                        taskId: state.taskId
                    });

                    if (mounted.current) {
                        setState(prev => ({ ...prev, progressSnapshot: snapshot }));

                        // 如果状态变成非运行中，可以在这里自动停止下载状态 (可选，取决于你的业务逻辑)
                        if (["completed", "error", "cancelled"].includes(snapshot.status)) {
                            // 通常 downloadAndApply 会处理完成状态，这里主要是为了 UI 同步
                        }
                    }
                } catch (e) {
                    console.warn("Failed to fetch task status:", e);
                }
            };

            // 立即执行一次
            fetchStatus();
            // 每 1000ms 执行一次
            intervalId = setInterval(fetchStatus, 1000);
        } else {
            // 如果不在下载，清空快照
            setState(prev => {
                if (prev.progressSnapshot) return { ...prev, progressSnapshot: null };
                return prev;
            });
        }

        return () => {
            if (intervalId) clearInterval(intervalId);
        };
    }, [state.downloading, state.taskId]);

    // 辅助函数：解析语义化版本
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

    // 监听进度
    useEffect(() => {
        let unlisten: UnlistenFn | null = null;

        const setupListener = async () => {
            unlisten = await listen<any>('update-download-progress', (event) => {
                if (!mounted.current) return;
                const p = event.payload?.progress || event.payload || 0;
                setState(prev => ({ ...prev, progress: typeof p === 'number' ? p : null }));
            });
        };

        if (state.downloading) {
            setupListener();
        }

        return () => {
            if (unlisten) unlisten();
        };
    }, [state.downloading]);

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

    // 核心修改：生成 ID 并传递给后端
    const downloadAndApply = useCallback(async (releaseSummary?: ReleaseData) => {
        const rs = releaseSummary || state.chosenRelease;
        if (!rs || !rs.asset_url) {
            throw new Error("releaseSummary or asset_url missing");
        }

        // 1. 前端生成唯一的 task ID
        const taskId = `update-task-${Date.now()}-${Math.floor(Math.random() * 1000)}`;

        setState(prev => ({
            ...prev,
            downloading: true,
            error: null,
            progressSnapshot: null,
            taskId: taskId // 保存 ID
        }));

        try {
            // 2. 将 task_id 传给 Rust
            await invoke("download_and_apply_update", {
                args: {
                    url: rs.asset_url,
                    filename_hint: rs.asset_name || null,
                    target_exe_path: "",
                    timeout_secs: 120,
                    auto_quit: true,
                    task_id: taskId // 传递 ID
                }
            });

            try {
                await invoke("quit_app");
            } catch (qerr) {
                console.warn("quit_app failed:", qerr);
            }

            if (mounted.current) {
                setState(prev => ({ ...prev, downloading: false, progress: 100, taskId: null }));
            }
        } catch (e: any) {
            // 如果是因为取消导致的报错，我们已经在 cancelDownload 处理了 UI，这里可以忽略或记录
            const errStr = String(e);
            console.error("download_and_apply_update error:", errStr);

            if (mounted.current) {
                // 如果错误包含 "cancelled"，说明是用户手动取消，不是真正错误
                if (errStr.toLowerCase().includes("cancelled") || errStr.toLowerCase().includes("取消")) {
                    setState(prev => ({ ...prev, downloading: false, taskId: null, progress: null }));
                } else {
                    setState(prev => ({ ...prev, downloading: false, error: errStr, taskId: null }));
                }
            }
        }
    }, [state.chosenRelease]);

    // 核心修改：使用保存的 ID 取消任务
    const cancelDownload = useCallback(async () => {
        const currentTaskId = state.taskId;

        if (!currentTaskId) {
            console.warn("No active task to cancel");
            return;
        }

        console.log("Cancelling task:", currentTaskId);

        try {
            // 调用 Rust 取消命令
            await invoke("cancel_task", { taskId: currentTaskId });

            // 更新 UI 状态
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