import { useState, useEffect } from "react";
import { useUpdater } from "./useUpdater";

export function useUpdaterWithModal({
                                        owner,
                                        repo,
                                        autoCheck = true,
                                        autoCheckIntervalMs = 0,
                                    }) {
    const { state: updState, actions: updActions } = useUpdater({
        owner,
        repo,
        autoCheck,
        autoCheckIntervalMs,
    });

    const [modalOpen, setModalOpen] = useState(false);
    const [selectedRelease, setSelectedRelease] = useState(null);
    const [seenTag, setSeenTag] = useState("");

    // 监听更新状态，自动弹窗
    useEffect(() => {
        if (!autoCheck) {
            if (modalOpen) setModalOpen(false);
            return;
        }

        // 优先稳定版，没有则预发布
        const latest = updState.latestStable ?? updState.latestPrerelease ?? null;
        if (
            latest &&
            latest.tag &&
            latest.tag !== seenTag &&
            updState.updateAvailable
        ) {
            setSeenTag(latest.tag);
            setSelectedRelease(latest);
            setModalOpen(true);
        }
    }, [
        updState.updateAvailable,
        updState.latestStable,
        updState.latestPrerelease,
        seenTag,
        modalOpen,
        autoCheck,
    ]);

    return {
        state: updState,
        actions: updActions,
        modalOpen,
        setModalOpen,
        selectedRelease,
    };
}
