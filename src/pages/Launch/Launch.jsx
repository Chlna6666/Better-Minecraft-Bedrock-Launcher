import React, { useEffect, useState, useRef, useCallback } from "react";
import { useTranslation } from 'react-i18next';
import "./Launch.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import useVersions from "../../hooks/useVersions.jsx"; // <- 使用 hook
import unknownIcon from "../../assets/feather/box.svg";

import LaunchModal from "./LaunchModal.jsx";

function Launch() {
    const { t } = useTranslation();

    // useVersions 提供 versions, counts, reload
    const { versions, counts, reload } = useVersions();

    // ---- 状态 ----
    const [selectedVersion, setSelectedVersion] = useState("");
    const [isOpen, setIsOpen] = useState(false);
    const [visible, setVisible] = useState(false);
    const [isClosing, setIsClosing] = useState(false);

    const [launchModalOpen, setLaunchModalOpen] = useState(false);
    const [launching, setLaunching] = useState(false);
    const [launchError, setLaunchError] = useState(null); // { code, message }
    const [launchDetails, setLaunchDetails] = useState(""); // **只保留最新一行**（string）

    // unlisten ref 保存当前监听器取消函数
    const unlistenRef = useRef(null);
    const containerRef = useRef(null);

    // ---- 本地存储管理（操作 launchCounts） ----
    const loadLaunchCounts = useCallback(() => {
        try {
            const raw = localStorage.getItem("launchCounts");
            return raw ? JSON.parse(raw) : {};
        } catch {
            return {};
        }
    }, []);
    const saveLaunchCounts = useCallback((countsObj) => {
        try {
            localStorage.setItem("launchCounts", JSON.stringify(countsObj));
        } catch {
            // ignore
        }
    }, []);

    // ---- 当 versions 更新时恢复上次选择或设置默认 ----
    useEffect(() => {
        if (!versions || versions.length === 0) {
            setSelectedVersion("");
            return;
        }
        let last = localStorage.getItem("lastSelectedVersion");
        // If last doesn't exist in current list, clear it and local counts for it
        if (last && !versions.find(v => v.folder === last)) {
            localStorage.removeItem("lastSelectedVersion");
            const lc = loadLaunchCounts();
            if (lc[last]) {
                delete lc[last];
                saveLaunchCounts(lc);
            }
            last = null;
        }
        if (last) {
            setSelectedVersion(last);
        } else {
            setSelectedVersion(versions[0].folder);
        }
    }, [versions, loadLaunchCounts, saveLaunchCounts]);

    // ---- 切换下拉 ----
    const toggleDropdown = () => {
        if (!visible) {
            // 刷新最新启动次数（从 localStorage），并显示
            setVisible(true);
            setIsOpen(true);
        } else if (isOpen) {
            setIsOpen(false);
            setIsClosing(true);
            setTimeout(() => {
                setVisible(false);
                setIsClosing(false);
            }, 400);
        } else {
            setIsOpen(true);
        }
    };

    // ---- 选中版本 ----
    const handleVersionSelect = (folder) => {
        setSelectedVersion(folder);
        setIsOpen(false);
        setIsClosing(true);
        setTimeout(() => {
            setVisible(false);
            setIsClosing(false);
        }, 400);
    };

    // helper：格式化事件负载为展示行（只生成单行文本）
    const formatLaunchPayload = (payload) => {
        try {
            const now = new Date().toLocaleTimeString();
            const stage = payload.stage ?? "unknown";
            const status = payload.status ?? "";
            const msg = payload.message ?? "";
            const code = payload.code ?? "";
            let line = `[${now}] [${stage}] ${status}`;
            if (msg) line += ` - ${msg}`;
            if (code) line += ` (${code})`;
            return line;
        } catch {
            return JSON.stringify(payload);
        }
    };

    // 监听器：建立监听并把最新事件替换到 launchDetails（保留最新一行）
    const startListeningLaunchProgress = useCallback(async () => {
        // 防止重复 listener
        if (unlistenRef.current) return;

        try {
            const off = await listen("launch-progress", (e) => {
                const payload = e.payload || {};
                const line = formatLaunchPayload(payload);
                // **只保留最新一行**
                setLaunchDetails(line);

                const status = payload.status ?? "";
                const stage = payload.stage ?? "";
                const message = payload.message ?? "";
                const code = payload.code ?? undefined;

                // 若后端报告错误 -> show error
                if (status === "error") {
                    setLaunching(false);
                    setLaunchError({ code: code ?? "未知", message: message ?? String(payload) });
                    // keep listener so user can retry or close
                }

                // 若后端报告完成 -> 关闭 modal 并进行成功后的本地处理
                if (stage === "done" && status === "ok") {
                    setLaunching(false);
                    // small delay for UX
                    setTimeout(async () => {
                        // stop listening
                        try { if (unlistenRef.current) { await unlistenRef.current(); } } catch(_) {}
                        unlistenRef.current = null;

                        // 关闭 modal
                        setLaunchModalOpen(false);
                        setLaunchError(null);

                        // 成功：记录启动次数 & 更新 UI（使用 reload 更新版本列表）
                        try {
                            if (selectedVersion) {
                                localStorage.setItem("lastSelectedVersion", selectedVersion);

                                const newCounts = loadLaunchCounts();
                                newCounts[selectedVersion] = (newCounts[selectedVersion] || 0) + 1;
                                saveLaunchCounts(newCounts);

                                // 触发 useVersions 重新加载（它会读 launchCounts 并重新排序）
                                if (typeof reload === 'function') await reload();
                            }
                        } catch (e) {
                            console.error("更新启动计数失败:", e);
                        }

                        // 清理临时信息（也可保留最新一行或清空）
                        setLaunchDetails("");
                    }, 600);
                }
            });

            unlistenRef.current = off;
        } catch (e) {
            console.error("监听 launch-progress 失败:", e);
        }
    }, [loadLaunchCounts, reload, saveLaunchCounts, selectedVersion]);

    // 停止监听并清理
    const stopListeningLaunchProgress = useCallback(async () => {
        if (unlistenRef.current) {
            try { await unlistenRef.current(); } catch (e) { /* ignore */ }
            unlistenRef.current = null;
        }
    }, []);

    // ---- 启动 ----
    const handleLaunch = useCallback(async () => {
        if (!selectedVersion) return;

        // open modal and start listening
        setLaunchModalOpen(true);
        setLaunching(true);
        setLaunchError(null);
        setLaunchDetails(`[${new Date().toLocaleTimeString()}] 开始启动 ${selectedVersion}`); // 最新一行

        await startListeningLaunchProgress();

        try {
            // 发起后端启动（后端会通过事件流反馈进度/结果）
            await invoke("launch_appx", { fileName: selectedVersion, autoStart: true });
            // 不在这里直接关闭 modal，等待后端的 "done" event
        } catch (err) {
            // 若 invoke 本身报错，则展示错误（并保留监听以接收更多日志）
            console.error("launch_appx invoke 失败:", err);
            let code = "未知";
            let message = "";

            try {
                if (err && typeof err === "object") {
                    if (err.code) code = String(err.code);
                    if (err.message) message = String(err.message);
                    if (!message && err.toString) message = err.toString();
                } else {
                    message = String(err);
                }
            } catch {
                message = String(err);
            }

            const hrMatch = (message || "").match(/HRESULT\([^)]+\)/);
            if (hrMatch) code = hrMatch[0];

            setLaunching(false);
            setLaunchError({ code, message });
            setLaunchDetails(`[${new Date().toLocaleTimeString()}] 错误: ${message}`); // 只显示最新一行错误
            // keep listener so backend logs (if any) can arrive; user can retry
        }
    }, [selectedVersion, startListeningLaunchProgress]);

    // 用户在 modal 点击关闭
    const handleModalClose = useCallback(async () => {
        // stop listening and reset modal state
        await stopListeningLaunchProgress();
        setLaunchModalOpen(false);
        setLaunching(false);
        setLaunchError(null);
        setLaunchDetails("");
    }, [stopListeningLaunchProgress]);

    // 用户点重试：关闭旧 listener，清理状态，然后重新启动
    const handleModalRetry = useCallback(async () => {
        await stopListeningLaunchProgress();
        setLaunchError(null);
        setLaunchDetails("");
        setLaunching(true);
        // small delay to let state settle
        setTimeout(() => {
            handleLaunch();
        }, 120);
    }, [handleLaunch, stopListeningLaunchProgress]);

    // 清理：组件卸载时确保取消监听
    useEffect(() => {
        return () => {
            if (unlistenRef.current) {
                unlistenRef.current().catch(()=>{});
                unlistenRef.current = null;
            }
        };
    }, []);

    // helper to safely get icon (versions contain icon but fallback if missing)
    const getIconForVersion = (v) => v?.icon || unknownIcon;

    return (
        <div className="launch-section">
            <div className={`version-selector-container ${isOpen ? "bounce-in" : ""}`}>
                <button
                    className="start-button"
                    disabled={versions.length === 0}
                    onClick={handleLaunch}
                >
                    <div className="button-content">
                        <span>{t('Launch.start_game')}</span>
                        <span className="version">
                            {selectedVersion || t('Launch.not_installed')}
                        </span>
                    </div>
                </button>
                <button
                    className="arrow-button"
                    onClick={toggleDropdown}
                    disabled={versions.length === 0}
                >
                    <div className={`arrow ${isOpen ? "open" : ""}`}>▲</div>
                </button>

                {visible && (
                    <div
                        className={`version-list ${
                            isOpen ? "slide-down" : isClosing ? "slide-up" : ""
                        }`}
                        ref={containerRef}
                        style={{ maxHeight: 300, overflowY: 'auto' }}
                    >
                        {versions.map(({ folder, name, version, type, icon }) => {
                            const count = counts[folder] || 0;
                            return (
                                <div
                                    key={folder}
                                    className="version-item"
                                    onClick={() => handleVersionSelect(folder)}
                                >
                                    <div style={{ display: "flex", alignItems: "center", width: "100%" }}>
                                        <img
                                            src={getIconForVersion({ icon })}
                                            alt="icon"
                                            className="version-icon"
                                        />
                                        <div className="version-info">
                                            <div className="version-title">{folder}</div>
                                            <div className="version-sub">
                                                {version} {type}
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            );
                        })}
                    </div>
                )}
            </div>

            <LaunchModal
                open={launchModalOpen}
                launching={launching}
                error={launchError}
                details={launchDetails} // 传入最新一行（string）
                onClose={handleModalClose}
                onRetry={handleModalRetry}
            />
        </div>
    );
}

export default Launch;
