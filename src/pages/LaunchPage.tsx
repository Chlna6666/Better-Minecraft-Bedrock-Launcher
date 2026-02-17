import React, { useEffect, useState, useRef, useCallback, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from 'react-i18next';
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
// 引入 Framer Motion 用于丝滑动画
import {
    Play, ChevronDown, Box, CheckCircle2,
    DownloadCloud
} from "lucide-react";

// @ts-ignore
import useVersions from "../hooks/useVersions.jsx";
import './LaunchPage.css';

import { LaunchStatusModal } from "../components/LaunchStatusModal.tsx";

// --- 导出类型 ---
export interface VersionData {
    folder: string;
    name: string;
    version: string;
    path: string;
    kind: string;
    kindLabel: string;
    versionType: string;
    versionTypeLabel: string;
    icon?: string;
}

export interface LaunchError {
    code: string;
    message: string;
    raw?: string;
}

// --- [导出 Hook]：复用启动逻辑 ---
export const useLauncher = (t: (key: string, options?: any) => string) => {
    const [isLaunching, setIsLaunching] = useState(false);
    const [launchLogs, setLaunchLogs] = useState<string[]>([]);
    const [launchError, setLaunchError] = useState<LaunchError | null>(null);
    const unlistenRef = useRef<UnlistenFn | null>(null);

    const startListening = useCallback(async (onSuccess?: () => void) => {
        if (unlistenRef.current) return;
        try {
            const unlisten = await listen("launch-progress", (e: any) => {
                const payload = e.payload || {};
                const now = new Date().toLocaleTimeString([], { hour12: false });
                const msg = payload.message || payload.status || t("LaunchPage.processing");

                setLaunchLogs(prev => [...prev, `[${now}] ${msg}`].slice(-50));

                if (payload.status === "error") {
                    const msgStr = payload.message || "";
                    const codeMatch = msgStr.match(/HRESULT\((0x[0-9A-Fa-f]+)\)/) || msgStr.match(/code:\s*(-?\d+)/);
                    setLaunchError({
                        code: payload.code || (codeMatch ? codeMatch[1] : t("LaunchPage.unknown_error")),
                        message: msgStr || t("LaunchPage.launch_failed"),
                        raw: JSON.stringify(payload)
                    });
                }

                if (payload.stage === "done" && payload.status === "ok") {
                    setLaunchLogs(prev => [...prev, t("LaunchPage.launch_success")]);
                    if (onSuccess) onSuccess();
                }
            });
            unlistenRef.current = unlisten;
        } catch (err) { console.error(err); }
    }, [t]);

    const launch = async (folderName: string, args: string | null = null, onSuccess?: () => void) => {
        setIsLaunching(true);
        setLaunchError(null);
        setLaunchLogs([
            t("LaunchPage.launching", { folder: folderName }),
            ...(args ? [t("LaunchPage.args", { args })] : [])
        ]);

        await startListening(onSuccess);

        try {
            await invoke("launch_appx", {
                fileName: folderName,
                autoStart: true,
                launchArgs: args
            });
        } catch (err: any) {
            const errStr = String(err);
            const codeMatch = errStr.match(/HRESULT\((0x[0-9A-Fa-f]+)\)/);
            setLaunchError({
                code: codeMatch ? codeMatch[1] : t("LaunchPage.invoke_error"),
                message: errStr
            });
        }
    };

    const close = () => {
        if (unlistenRef.current) { unlistenRef.current(); unlistenRef.current = null; }
        setIsLaunching(false);
        setLaunchError(null);
    };

    useEffect(() => {
        return () => {
            if (unlistenRef.current) unlistenRef.current();
        };
    }, []);

    return { isLaunching, launchLogs, launchError, launch, close };
};

// --- LaunchPage 组件 ---
export const LaunchPage = () => {
    const { t } = useTranslation();
    const navigate = useNavigate();
    // @ts-ignore
    const { versions, counts, reload } = useVersions();

    const [selectedFolder, setSelectedFolder] = useState<string>("");
    const [isDropdownOpen, setIsDropdownOpen] = useState(false);

    const { isLaunching, launchLogs, launchError, launch, close } = useLauncher(t);
    const dropdownRef = useRef<HTMLDivElement>(null);

    // ... (LaunchCounts 逻辑保持不变)
    const loadLaunchCounts = useCallback(() => {
        try { return JSON.parse(localStorage.getItem("launchCounts") || "{}"); } catch { return {}; }
    }, []);

    const saveLaunchCounts = useCallback((obj: any) => {
        localStorage.setItem("launchCounts", JSON.stringify(obj));
    }, []);

    const sortedVersions = useMemo(() => {
        if (!versions) return [];
        return [...versions].sort((a: VersionData, b: VersionData) => {
            const localCounts = counts || loadLaunchCounts();
            const countA = localCounts[a.folder] || 0;
            const countB = localCounts[b.folder] || 0;
            if (countB !== countA) return countB - countA;
            if (a.versionType !== b.versionType) return a.versionType === 'release' ? -1 : 1;
            return a.folder.localeCompare(b.folder);
        });
    }, [versions, counts, loadLaunchCounts]);

    useEffect(() => {
        if (!sortedVersions || sortedVersions.length === 0) { setSelectedFolder(""); return; }
        const last = localStorage.getItem("lastSelectedVersion");
        const exists = sortedVersions.find((v: VersionData) => v.folder === last);
        setSelectedFolder(exists ? last! : sortedVersions[0].folder);
    }, [sortedVersions]);

    useEffect(() => {
        const handleClickOutside = (e: MouseEvent) => {
            if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) setIsDropdownOpen(false);
        };
        document.addEventListener("mousedown", handleClickOutside);
        return () => document.removeEventListener("mousedown", handleClickOutside);
    }, []);

    const handleLaunchClick = () => {
        if (!versions || versions.length === 0) { navigate('/download'); return; }
        if (!selectedFolder) return;

        launch(selectedFolder, null, () => {
            setTimeout(() => {
                close();
                localStorage.setItem("lastSelectedVersion", selectedFolder);
                const nc = loadLaunchCounts();
                nc[selectedFolder] = (nc[selectedFolder] || 0) + 1;
                saveLaunchCounts(nc);
                if (reload) reload();
            }, 1500);
        });
    };

    const currentVer = versions?.find((v: VersionData) => v.folder === selectedFolder);
    const isEmpty = !versions || versions.length === 0;

    return (
        <div className="launch-page-root">
            <div className="launch-floater-wrapper fade-in-up" ref={dropdownRef}>
                {!isEmpty && (
                    <div
                        className={`version-list-card glass ${isDropdownOpen ? 'is-open' : ''}`}
                        aria-hidden={!isDropdownOpen}
                    >
                        <div className="list-scroll-area">
                            {sortedVersions.map((v: VersionData, index: number) => (
                                <div
                                    key={v.folder}
                                    className={`version-item ${selectedFolder === v.folder ? 'selected' : ''}`}
                                    style={{ ["--bm-item-i" as any]: index }}
                                    onClick={() => { setSelectedFolder(v.folder); setIsDropdownOpen(false); }}
                                >
                                    <div className="item-icon">
                                        {v.icon ? <img src={v.icon} alt="icon" /> : <Box size={20} />}
                                    </div>
                                    <div className="item-info">
                                        <div className="item-title">{v.folder}</div>
                                        <div className="item-meta">
                                            <span className="ver-num">{v.version}</span>
                                            <span className={`ver-tag kind ${v.kind?.toLowerCase() || ''}`}>{v.kindLabel}</span>
                                        </div>
                                    </div>
                                    {selectedFolder === v.folder && <CheckCircle2 size={18} className="check-icon" />}
                                </div>
                            ))}
                        </div>
                    </div>
                )}

                <div className={`main-launch-btn ${isEmpty ? 'empty-state' : ''}`}>
                    <div className="btn-left ripple-effect" onClick={handleLaunchClick}>
                        <div className="btn-content">
                            <div className="btn-label">{isEmpty ? t("LaunchPage.not_installed") : t("LaunchPage.start_game")}</div>
                            <div className="btn-sub">{isEmpty ? t("LaunchPage.go_download") : (currentVer ? currentVer.version : t("LaunchPage.select_version"))}</div>
                        </div>
                        <div className="btn-bg-icon"><Play /></div>
                    </div>
                    <div className="btn-right" onClick={(e) => {
                        if (isEmpty) { handleLaunchClick(); }
                        else { e.stopPropagation(); setIsDropdownOpen((v) => !v); }
                    }}>
                        {isEmpty ? (
                            <DownloadCloud size={20} />
                        ) : (
                            <span className={`launch-chevron ${isDropdownOpen ? 'is-open' : ''}`}>
                                <ChevronDown size={20} />
                            </span>
                        )}
                    </div>
                </div>
            </div>

            <LaunchStatusModal
                isOpen={isLaunching}
                logs={launchLogs}
                error={launchError}
                onClose={close}
                onRetry={handleLaunchClick}
            />
        </div>
    );
};
