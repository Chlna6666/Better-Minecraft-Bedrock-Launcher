// src/hooks/useUpdater.jsx
import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * useUpdater - hook to interact with backend updater API
 *
 * owner/repo: GitHub repository to check, e.g. "Chlna6666" / "Better-Minecraft-Bedrock-Launcher"
 *
 * Exposed:
 * - state: { checking, error, latestStable, latestPrerelease, selectedChannel, chosenRelease, currentVersion, updateAvailable }
 * - checkForUpdates()
 * - downloadAndApply(releaseSummary) -> starts backend download and apply, resolves when done
 */
export function useUpdater({
                               owner = "Chlna6666",
                               repo = "Better-Minecraft-Bedrock-Launcher",
                               autoCheck = true,
                               autoCheckIntervalMs = 0 // >0 to poll periodically
                           } = {}) {
    const [checking, setChecking] = useState(false);
    const [error, setError] = useState(null);
    const [currentVersion, setCurrentVersion] = useState(null);
    const [latestStable, setLatestStable] = useState(null);
    const [latestPrerelease, setLatestPrerelease] = useState(null);
    const [selectedChannel, setSelectedChannel] = useState(null);
    const [selectedRelease, setSelectedRelease] = useState(null); // raw selected_release from backend
    const [chosenRelease, setChosenRelease] = useState(null); // final release to show/use (selected_release OR fallback)
    const [downloading, setDownloading] = useState(false);
    const [updateAvailable, setUpdateAvailable] = useState(false);
    const mounted = useRef(true);
    useEffect(() => () => { mounted.current = false; }, []);

    // simple semver extractor + comparator fallback (only supports X.Y.Z, ignores prerelease/build)
    const parseSemverSimple = (s) => {
        if (!s || typeof s !== "string") return null;
        const m = s.match(/(\d+)\.(\d+)\.(\d+)/);
        if (!m) return null;
        return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)];
    };
    const semverGreater = (aArr, bArr) => {
        if (!aArr || !bArr) return false;
        for (let i = 0; i < 3; i++) {
            if (aArr[i] > bArr[i]) return true;
            if (aArr[i] < bArr[i]) return false;
        }
        return false;
    };

    const buildChosenRelease = (resp) => {
        // prefer explicit selected_release from backend
        if (resp.selected_release) return resp.selected_release;
        // otherwise fallback based on selected_channel (nightly -> latest_prerelease, else latest_stable)
        if (resp.selected_channel === "nightly") {
            return resp.latest_prerelease || resp.latest_stable || null;
        }
        return resp.latest_stable || resp.latest_prerelease || null;
    };

    const parseCheckResp = (resp) => {
        if (!resp) return;
        setCurrentVersion(resp.current_version || null);
        setLatestStable(resp.latest_stable || null);
        setLatestPrerelease(resp.latest_prerelease || null);
        setSelectedChannel(resp.selected_channel || null);
        setSelectedRelease(resp.selected_release || null);

        const chosen = buildChosenRelease(resp);
        setChosenRelease(chosen || null);

        // Prefer explicit backend flag if present
        if (typeof resp.update_available === "boolean") {
            setUpdateAvailable(resp.update_available);
            return;
        }

        // Fallback: compare chosenRelease vs currentVersion using simple semver
        try {
            const cur = resp.current_version || null;
            const curArr = parseSemverSimple(cur);
            let hasUpdate = false;
            if (chosen) {
                // prefer tag then name then asset_name
                const candidate = chosen.tag || chosen.name || (chosen.asset_name ? chosen.asset_name : null);
                const candArr = parseSemverSimple(candidate);
                if (candArr && curArr) {
                    hasUpdate = semverGreater(candArr, curArr);
                } else {
                    // if can't parse semver, assume backend selection implies available update
                    hasUpdate = true;
                }
            }
            setUpdateAvailable(Boolean(hasUpdate));
        } catch (e) {
            setUpdateAvailable(false);
        }
    };

    const checkForUpdates = useCallback(async () => {
        setChecking(true);
        setError(null);
        try {
            const resp = await invoke("check_updates", { owner, repo });
            parseCheckResp(resp);
            if (mounted.current) setChecking(false);
            return resp;
        } catch (e) {
            console.error("check_updates error:", e);
            if (mounted.current) {
                setError(e?.toString?.() || String(e));
                setChecking(false);
            }
            throw e;
        }
    }, [owner, repo]);

    // optional auto-check on mount
    useEffect(() => {
        if (autoCheck) {
            checkForUpdates().catch(() => {});
        }
        if (autoCheckIntervalMs > 0) {
            const id = setInterval(() => { checkForUpdates().catch(()=>{}); }, autoCheckIntervalMs);
            return () => clearInterval(id);
        }
    }, [autoCheck, autoCheckIntervalMs, checkForUpdates]);

    const downloadAndApply = useCallback(async (releaseSummary) => {
        // if no releaseSummary passed, use chosenRelease (selected_release or fallback)
        const rs = releaseSummary || chosenRelease;
        if (!rs || !rs.asset_url) {
            throw new Error("releaseSummary or asset_url missing");
        }
        setDownloading(true);
        setError(null);
        try {
            const filename_hint = rs.asset_name || null;
            const resp = await invoke("download_and_apply_update", {
                args: {
                    url: rs.asset_url,
                    filename_hint,
                    target_exe_path: "",
                    timeout_secs: 91,
                    auto_quit: true
                }
            });

            try {
                await invoke("quit_app");
            } catch (qerr) {
                console.warn("quit_app failed:", qerr);
            }

            if (mounted.current) {
                setDownloading(false);
            }
            return resp;
        } catch (e) {
            console.error("download_and_apply_update error:", e);
            if (mounted.current) {
                setError(e?.toString?.() || String(e));
                setDownloading(false);
            }
            throw e;
        }
    }, [chosenRelease]);

    return {
        state: {
            checking,
            error,
            currentVersion,
            latestStable,
            latestPrerelease,
            selectedChannel,
            selectedRelease,
            chosenRelease,
            downloading,
            updateAvailable,
        },
        actions: {
            checkForUpdates,
            downloadAndApply,
        },
    };
}
