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

    useEffect(() => {
        if (!autoCheck) {
            if (modalOpen) setModalOpen(false);
            return;
        }

        const chosen = updState.chosenRelease ?? (updState.selectedChannel === "nightly"
                ? (updState.latestPrerelease ?? updState.latestStable)
                : (updState.latestStable ?? updState.latestPrerelease)
        );

        if (!chosen) {
            setSelectedRelease(null);
            return;
        }

        const tag = chosen.tag || chosen.name || (chosen.asset_name ?? "");

        if (updState.updateAvailable && tag && tag !== seenTag) {
            setSeenTag(tag);
            setSelectedRelease(chosen);
            setModalOpen(true);
        }
    }, [
        autoCheck,
        modalOpen,
        seenTag,
        updState.updateAvailable,
        updState.chosenRelease,
        updState.selectedChannel,
        updState.latestStable,
        updState.latestPrerelease,
    ]);

    return {
        state: updState,
        actions: updActions,
        modalOpen,
        setModalOpen,
        selectedRelease,
    };
}
