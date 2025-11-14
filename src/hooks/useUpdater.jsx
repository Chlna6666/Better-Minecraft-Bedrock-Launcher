// src/hooks/useUpdater.jsx
import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * useUpdater - hook to interact with backend updater API
 *
 * owner/repo: GitHub repository to check, e.g. "Chlna6666" / "Better-Minecraft-Bedrock-Launcher"
 *
 * Exposed:
 * - state: { checking, error, latestStable, latestPrerelease, currentVersion, updateAvailable }
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
    const [downloading, setDownloading] = useState(false);
    const [updateAvailable, setUpdateAvailable] = useState(false);
    const mounted = useRef(true);
    useEffect(() => () => { mounted.current = false; }, []);

    // simple semver extractor + comparator fallback (only supports X.Y.Z, ignores prerelease/build)
    const parseSemverSimple = (s) => {
        if (!s || typeof s !== "string") return null;
        // try common places: tag or version like "v1.2.3" or "1.2.3"
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

    const parseCheckResp = (resp) => {
        if (!resp) return;
        setCurrentVersion(resp.current_version || null);
        setLatestStable(resp.latest_stable || null);
        setLatestPrerelease(resp.latest_prerelease || null);

        // Prefer explicit backend flag
        if (typeof resp.update_available === "boolean") {
            setUpdateAvailable(resp.update_available);
            return;
        }

        // Fallback: do simple semver compare client-side using latest_stable.tag or latest_stable.asset_name
        try {
            const cur = resp.current_version || null;
            const curArr = parseSemverSimple(cur);
            let hasUpdate = false;
            if (resp.latest_stable) {
                const ls = resp.latest_stable;
                // prefer tag field, fall back to name or asset name
                const candidate = ls.tag || ls.name || (ls.asset_name ? ls.asset_name : null);
                const lsArr = parseSemverSimple(candidate);
                if (lsArr && curArr) {
                    hasUpdate = semverGreater(lsArr, curArr);
                }
            }
            setUpdateAvailable(Boolean(hasUpdate));
        } catch (e) {
            // on any error fallback to false
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
        if (!releaseSummary || !releaseSummary.asset_url) {
            throw new Error("releaseSummary or asset_url missing");
        }
        setDownloading(true);
        setError(null);
        try {
            const filename_hint = releaseSummary.asset_name || null;
            const resp = await invoke("download_and_apply_update", {
                args: {
                    url: releaseSummary.asset_url,
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
    }, []);

    return {
        state: {
            checking,
            error,
            currentVersion,
            latestStable,
            latestPrerelease,
            downloading,
            updateAvailable,
        },
        actions: {
            checkForUpdates,
            downloadAndApply,
        },
    };
}
