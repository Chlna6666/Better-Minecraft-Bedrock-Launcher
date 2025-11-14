// src/hooks/useUpdater.jsx
import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
/**
 * useUpdater - hook to interact with backend updater API
 *
 * owner/repo: GitHub repository to check, e.g. "Chlna6666" / "Better-Minecraft-Bedrock-Launcher"
 *
 * Exposed:
 * - state: { checking, error, latestStable, latestPrerelease, currentVersion }
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
    const mounted = useRef(true);
    useEffect(() => () => { mounted.current = false; }, []);
    const parseCheckResp = (resp) => {
        // resp shape matches your Rust response JSON
        if (!resp) return;
        setCurrentVersion(resp.current_version || null);
        setLatestStable(resp.latest_stable || null);
        setLatestPrerelease(resp.latest_prerelease || null);
    };
    const checkForUpdates = useCallback(async () => {
        setChecking(true);
        setError(null);
        try {
            const resp = await invoke("check_updates", { owner, repo });
            // resp is a JS object from Rust serde_json
            parseCheckResp(resp);
            setChecking(false);
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
            // optional filename hint uses asset_name
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
                // don't treat quit failure as fatal — just log and continue
                console.warn("quit_app failed:", qerr);
            }


            // resp should contain { saved_to, bytes, applied: true } or similar
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
        },
        actions: {
            checkForUpdates,
            downloadAndApply,
        },
    };
}