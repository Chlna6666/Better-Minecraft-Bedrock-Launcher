import { useState, useEffect, useCallback } from "react";
import { useUpdater } from "./useUpdater";

interface UseUpdaterWithModalOptions {
    owner: string;
    repo: string;
    autoCheck?: boolean;
    autoOpen?: boolean;
}

export function useUpdaterWithModal({
                                        owner,
                                        repo,
                                        autoCheck = true,
                                        autoOpen = false,
                                    }: UseUpdaterWithModalOptions) {

    // 1. 使用核心 Hook 获取状态和操作
    const { state, actions } = useUpdater({
        owner,
        repo,
        autoCheck,
    });

    // 2. 本地弹窗状态管理
    const [modalOpen, setModalOpen] = useState(false);
    // 记录上一次自动弹窗的版本，防止刷新页面或组件重绘时重复弹窗
    const [lastSeenVersion, setLastSeenVersion] = useState<string>("");

    // 3. [自动弹窗逻辑] 监听 updateAvailable 变化
    useEffect(() => {
        if (!autoOpen) return;
        const release = state.chosenRelease;

        // 只有当明确有更新，且存在版本信息时才处理
        if (state.updateAvailable && release) {
            // 获取版本唯一标识 (tag 或 name)
            const versionId = release.tag || release.name || "unknown_version";

            // 逻辑：
            // 1. 版本号有效
            // 2. 该版本尚未自动弹过窗
            // 3. 当前没有正在下载 (下载中不需要重新弹)
            if (versionId && versionId !== lastSeenVersion && !state.downloading) {
                console.log(`[Updater] Auto-opening modal for new version: ${versionId}`);
                setLastSeenVersion(versionId);
                setModalOpen(true);
            }
        }
    }, [autoOpen, state.updateAvailable, state.chosenRelease, state.downloading, lastSeenVersion]);

    // 4. 封装关闭逻辑
    const closeModal = useCallback(() => {
        // 下载中禁止通过点击背景关闭 (除非点击取消按钮)
        if (!state.downloading) {
            setModalOpen(false);
        }
    }, [state.downloading]);

    // 5. 封装下载逻辑
    const startDownload = useCallback(() => {
        // 如果 UI 传了 release 参数进来也可以用，但这里直接用 state.chosenRelease 更稳妥
        if (state.chosenRelease) {
            actions.downloadAndApply(state.chosenRelease);
        }
    }, [actions, state.chosenRelease]);

    // 6. 封装取消逻辑
    const cancelDownload = useCallback(() => {
        actions.cancelDownload();
        setModalOpen(false);
    }, [actions]);

    return {
        modalOpen,
        setModalOpen,
        closeModal,
        newRelease: state.updateAvailable ? state.chosenRelease : null,
        downloading: state.downloading,
        progressPercent: state.progress ?? (state.progressSnapshot?.done && state.progressSnapshot?.total
            ? Math.round((state.progressSnapshot.done / state.progressSnapshot.total) * 100)
            : 0),
        progressSnapshot: state.progressSnapshot,
        taskStatus: state.progressSnapshot?.status || null,
        startDownload,
        cancelDownload,
        checkForUpdates: actions.checkForUpdates,
        checking: state.checking,
        error: state.error
    };
}
