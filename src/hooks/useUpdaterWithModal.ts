// src/hooks/useUpdaterWithModal.ts
import { useState, useEffect, useCallback } from "react";
import { useUpdater } from "./useUpdater";
import { ReleaseData } from "../types/updater.ts";

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

    // 使用核心 Hook
    const { state, actions } = useUpdater({
        owner,
        repo,
        autoCheck,
    });

    const [modalOpen, setModalOpen] = useState(false);
    const [lastSeenVersion, setLastSeenVersion] = useState<string>("");

    // 当检测到新版本时，自动打开弹窗
    useEffect(() => {
        if (!autoCheck) return;

        // 获取当前应该展示的版本
        const release = state.chosenRelease;

        if (state.updateAvailable && release) {
            // 生成唯一标识，防止重复弹窗
            const versionId = release.tag || release.name || release.asset_name || "";

            // 如果这个版本还没弹过窗，且当前不在下载中
            if (versionId && versionId !== lastSeenVersion && !state.downloading) {
                setLastSeenVersion(versionId);
                setModalOpen(true);
            }
        }
    }, [state.updateAvailable, state.chosenRelease, state.downloading, lastSeenVersion, autoCheck]);

    // 封装关闭逻辑
    const closeModal = useCallback(() => {
        // 如果正在下载，不允许通过常规方式关闭（弹窗内部会处理）
        if (!state.downloading) {
            setModalOpen(false);
        }
    }, [state.downloading]);

    // 封装下载逻辑
    const startDownload = useCallback((release: ReleaseData) => {
        actions.downloadAndApply(release);
    }, [actions]);

    // 返回扁平化结构，符合 App.tsx 的解构需求
    return {
        modalOpen,
        closeModal,
        newRelease: state.chosenRelease, // 重命名以匹配 App.tsx
        downloading: state.downloading,
        progressSnapshot: state.progressSnapshot,
        startDownload,
        cancelDownload: actions.cancelDownload
    };
}