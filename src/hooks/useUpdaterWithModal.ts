import { useState, useEffect, useCallback } from "react";
import { useUpdater } from "./useUpdater";

interface UseUpdaterWithModalOptions {
    owner: string;
    repo: string;
    autoCheck?: boolean;
}

export function useUpdaterWithModal({
                                        owner,
                                        repo,
                                        autoCheck = true,
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
    }, [state.updateAvailable, state.chosenRelease, state.downloading, lastSeenVersion]);

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
        // --- 弹窗控制 ---
        modalOpen,
        setModalOpen, // [关键] 暴露给 Navbar 手动点击打开
        closeModal,

        // --- 数据状态 ---
        // 只有当确实有更新时才返回 newRelease，避免 Navbar 错误显示红点
        newRelease: state.updateAvailable ? state.chosenRelease : null,

        downloading: state.downloading,

        // 映射进度：优先使用事件监听到的数字进度 (0-100)，如果没有则尝试从快照获取
        progressSnapshot: state.progress ?? (state.progressSnapshot?.processed_bytes && state.progressSnapshot?.total_bytes
            ? Math.round((state.progressSnapshot.processed_bytes / state.progressSnapshot.total_bytes) * 100)
            : 0),

        // 详细任务状态 (如果 UI 需要显示 "解压中..." 等状态)
        taskStatus: state.progressSnapshot?.status || null,

        // --- 操作 ---
        startDownload,
        cancelDownload,
        checkForUpdates: actions.checkForUpdates,

        // --- 调试信息 ---
        checking: state.checking,
        error: state.error
    };
}