import React, { useEffect, useState, useRef, useCallback, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from 'react-i18next';
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { motion, AnimatePresence } from "framer-motion";
import {
    Play, ChevronDown, Box, Loader2, AlertCircle, RotateCcw, X, CheckCircle2,
    DownloadCloud, Copy, Search, Flame // 引入 Flame 图标可选，用于标记热门
} from "lucide-react";

import useVersions from "../hooks/useVersions";
import './LaunchPage.css';

interface VersionData {
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

interface LaunchError {
    code: string;
    message: string;
    raw?: string;
}

export const LaunchPage = () => {
    const { t } = useTranslation();
    const navigate = useNavigate();

    // 假设 useVersions 已经返回了 counts 对象 { "FolderName": 5, ... }
    // @ts-ignore
    const { versions, counts, reload } = useVersions();

    const [selectedFolder, setSelectedFolder] = useState<string>("");
    const [isDropdownOpen, setIsDropdownOpen] = useState(false);

    // 启动状态
    const [isLaunching, setIsLaunching] = useState(false);
    const [launchLogs, setLaunchLogs] = useState<string[]>([]);
    const [launchError, setLaunchError] = useState<LaunchError | null>(null);

    const unlistenRef = useRef<UnlistenFn | null>(null);
    const dropdownRef = useRef<HTMLDivElement>(null);
    const logsEndRef = useRef<HTMLDivElement>(null);
    const listScrollRef = useRef<HTMLDivElement>(null);

    // --- 本地存储辅助函数 ---
    const loadLaunchCounts = useCallback(() => {
        try { return JSON.parse(localStorage.getItem("launchCounts") || "{}"); } catch { return {}; }
    }, []);

    const saveLaunchCounts = useCallback((obj: any) => {
        localStorage.setItem("launchCounts", JSON.stringify(obj));
    }, []);

    // --- [核心功能]：根据启动次数对版本列表进行排序 ---
    const sortedVersions = useMemo(() => {
        if (!versions) return [];
        // 创建浅拷贝以避免修改原数组
        return [...versions].sort((a: VersionData, b: VersionData) => {
            // 获取启动次数，默认为 0
            // 优先使用 hook 返回的 counts，如果没有则尝试从 localStorage 读取（防抖底）
            const localCounts = counts || loadLaunchCounts();
            const countA = localCounts[a.folder] || 0;
            const countB = localCounts[b.folder] || 0;

            // 1. 优先按启动次数降序排列 (次数多的在上面)
            if (countB !== countA) {
                return countB - countA;
            }

            // 2. 次数相同时，Release (正式版) 优先于 Preview
            if (a.versionType !== b.versionType) {
                return a.versionType === 'release' ? -1 : 1;
            }

            // 3. 最后按文件夹名称字母顺序
            return a.folder.localeCompare(b.folder);
        });
    }, [versions, counts, loadLaunchCounts]);

    // --- 初始化选中逻辑 ---
    useEffect(() => {
        if (!sortedVersions || sortedVersions.length === 0) {
            setSelectedFolder("");
            return;
        }

        const last = localStorage.getItem("lastSelectedVersion");
        const exists = sortedVersions.find((v: VersionData) => v.folder === last);

        if (last && exists) {
            setSelectedFolder(last);
        } else {
            // 如果没有上次记录，默认选中排序后的第一个（即启动次数最多的）
            setSelectedFolder(sortedVersions[0].folder);
        }
    }, [sortedVersions]);

    // 点击外部关闭下拉菜单
    useEffect(() => {
        const handleClickOutside = (e: MouseEvent) => {
            if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) setIsDropdownOpen(false);
        };
        document.addEventListener("mousedown", handleClickOutside);
        return () => document.removeEventListener("mousedown", handleClickOutside);
    }, []);

    // 下拉菜单打开时重置滚动条
    useEffect(() => {
        if (isDropdownOpen && listScrollRef.current) {
            listScrollRef.current.scrollTop = 0;
        }
    }, [isDropdownOpen]);

    // 日志自动滚动
    useEffect(() => {
        if (logsEndRef.current) {
            logsEndRef.current.scrollIntoView({ behavior: "smooth" });
        }
    }, [launchLogs, isLaunching]);

    const copyErrorToClipboard = () => {
        if (launchError) {
            navigator.clipboard.writeText(JSON.stringify(launchError, null, 2));
        }
    };

    const searchErrorOnline = () => {
        if (launchError) {
            const query = `Minecraft Bedrock Error ${launchError.code}`;
            window.open(`https://www.bing.com/search?q=${encodeURIComponent(query)}`, '_blank');
        }
    };

    // --- 监听启动进度 ---
    const startListening = useCallback(async () => {
        if (unlistenRef.current) return;
        try {
            const unlisten = await listen("launch-progress", (e: any) => {
                const payload = e.payload || {};
                const now = new Date().toLocaleTimeString([], { hour12: false });
                const msg = payload.message || payload.status || "Processing...";

                setLaunchLogs(prev => {
                    const newLog = `[${now}] ${msg}`;
                    // 保持最近 50 条日志
                    return [...prev, newLog].slice(-50);
                });

                // 处理错误
                if (payload.status === "error") {
                    const msgStr = payload.message || "";
                    const codeMatch = msgStr.match(/HRESULT\((0x[0-9A-Fa-f]+)\)/) || msgStr.match(/code:\s*(-?\d+)/);
                    const code = payload.code || (codeMatch ? codeMatch[1] : "Unknown Error");

                    setLaunchError({
                        code: code,
                        message: msgStr || "Launch failed with unknown error.",
                        raw: JSON.stringify(payload)
                    });
                }

                // 处理成功完成
                if (payload.stage === "done" && payload.status === "ok") {
                    setLaunchLogs(prev => [...prev, "启动成功！游戏窗口即将出现..."]);

                    setTimeout(() => {
                        handleClose();

                        // [核心功能]：更新启动次数
                        if (selectedFolder) {
                            // 1. 保存上次选择
                            localStorage.setItem("lastSelectedVersion", selectedFolder);

                            // 2. 更新计数
                            const nc = loadLaunchCounts();
                            nc[selectedFolder] = (nc[selectedFolder] || 0) + 1;
                            saveLaunchCounts(nc);

                            // 3. 触发 Hook 重新加载，这将更新 `counts` 状态并触发 `sortedVersions` 重新排序
                            if (reload) reload();
                        }
                    }, 1500);
                }
            });
            unlistenRef.current = unlisten;
        } catch (err) { console.error(err); }
    }, [selectedFolder, loadLaunchCounts, saveLaunchCounts, reload]);

    // --- 触发启动 ---
    const handleLaunch = async () => {
        if (!versions || versions.length === 0) { navigate('/download'); return; }
        if (!selectedFolder) return;

        setIsLaunching(true);
        setLaunchError(null);
        setLaunchLogs(["正在准备启动环境..."]);
        await startListening();

        try {
            await invoke("launch_appx", { fileName: selectedFolder, autoStart: true });
        } catch (err: any) {
            const errStr = String(err);
            const codeMatch = errStr.match(/HRESULT\((0x[0-9A-Fa-f]+)\)/);
            setLaunchError({
                code: codeMatch ? codeMatch[1] : "Invoke Error",
                message: errStr
            });
        }
    };

    const handleClose = async () => {
        if (unlistenRef.current) { unlistenRef.current(); unlistenRef.current = null; }
        setIsLaunching(false);
        setLaunchError(null);
    };

    const handleRetry = async () => {
        if (unlistenRef.current) { unlistenRef.current(); unlistenRef.current = null; }
        handleLaunch();
    };

    // 获取当前选中的版本数据（从 sortedVersions 中查找性能更好，或者直接 find 原始数据皆可）
    const currentVer = versions?.find((v: VersionData) => v.folder === selectedFolder);
    const isEmpty = !versions || versions.length === 0;

    return (
        <div className="launch-page-root">
            <div className="launch-floater-wrapper fade-in-up" ref={dropdownRef}>
                {/* 1. 版本列表 */}
                <div className={`version-list-card glass ${isDropdownOpen && !isEmpty ? 'active' : ''}`}>
                    <div className="list-scroll-area" ref={listScrollRef}>
                        {/* 使用 sortedVersions 渲染列表 */}
                        {sortedVersions.map((v: VersionData, index: number) => {
                            const count = counts ? (counts[v.folder] || 0) : 0;
                            return (
                                <div
                                    key={v.folder}
                                    className={`version-item ${selectedFolder === v.folder ? 'selected' : ''}`}
                                    style={{ '--i': index } as React.CSSProperties}
                                    onClick={() => { setSelectedFolder(v.folder); setIsDropdownOpen(false); }}
                                >
                                    <div className="item-icon">
                                        {v.icon ? <img src={v.icon} alt="icon" /> : <Box size={20} />}
                                    </div>
                                    <div className="item-info">
                                        <div className="item-title">
                                            {v.folder}
                                            {/* 可选：显示启动次数非常多的版本为热门 */}
                                            {count > 10 && <Flame size={12} className="hot-icon" style={{marginLeft: 6, color: '#fb923c', display:'inline'}} />}
                                        </div>
                                        <div className="item-meta">
                                            <span className="ver-num">{v.version}</span>
                                            {/* 显示启动次数 (可选) */}
                                            {/* <span style={{opacity:0.5, fontSize:10, marginRight:4}}>({count})</span> */}

                                            <span className={`ver-tag kind ${v.kind?.toLowerCase() || ''}`}>{v.kindLabel}</span>
                                            <span className={`ver-tag ${v.versionType === 'release' ? 'release' : 'preview'}`}>
                                                {v.versionTypeLabel}
                                            </span>
                                        </div>
                                    </div>
                                    {selectedFolder === v.folder && <CheckCircle2 size={18} className="check-icon" />}
                                </div>
                            );
                        })}
                    </div>
                </div>

                {/* 2. 主按钮 */}
                <div className={`main-launch-btn ${isEmpty ? 'empty-state' : ''}`}>
                    <div className="btn-left ripple-effect" onClick={handleLaunch}>
                        <div className="btn-content">
                            <div className="btn-label">{isEmpty ? "未安装游戏" : "启动游戏"}</div>
                            <div className="btn-sub">{isEmpty ? "点击前往下载" : (currentVer ? currentVer.version : "请选择版本")}</div>
                        </div>
                        <div className="btn-bg-icon"><Play /></div>
                    </div>
                    <div
                        className="btn-right"
                        onClick={(e) => {
                            if (isEmpty) { handleLaunch(); }
                            else { e.stopPropagation(); setIsDropdownOpen(!isDropdownOpen); }
                        }}
                    >
                        {isEmpty ? <DownloadCloud size={20} /> : <ChevronDown className={`arrow ${isDropdownOpen ? 'open' : ''}`} size={20} />}
                    </div>
                </div>
            </div>

            {/* 3. 启动状态弹窗 */}
            <AnimatePresence>
                {isLaunching && (
                    <div className="launch-overlay-fixed">
                        <motion.div
                            className="overlay-backdrop"
                            initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
                        />

                        <motion.div
                            className={`launch-modal-fixed glass ${launchError ? 'error-mode' : ''}`}
                            initial={{ opacity: 0, scale: 0.9, y: 30 }}
                            animate={{ opacity: 1, scale: 1, y: 0 }}
                            exit={{ opacity: 0, scale: 0.9, y: 30 }}
                            transition={{ type: "spring", stiffness: 400, damping: 25 }}
                        >
                            {launchError && (
                                <button className="close-icon-btn" onClick={handleClose}>
                                    <X size={18} />
                                </button>
                            )}

                            <div className="modal-header">
                                <div className={`status-icon-ring ${launchError ? 'error' : 'loading'}`}>
                                    {launchError ? <AlertCircle size={32} /> : <Loader2 size={32} className="spin" />}
                                </div>
                                <div className="header-text">
                                    <h3>{launchError ? "Launch Failed" : "Starting Game"}</h3>
                                    <span className="sub-status">
                                        {launchError ? `Error Code: ${launchError.code}` : "正在加载游戏资源..."}
                                    </span>
                                </div>
                            </div>

                            <div className="modal-body">
                                {launchError ? (
                                    <div className="error-detail-box">
                                        <p>{launchError.message}</p>
                                    </div>
                                ) : (
                                    <div className="log-output-box">
                                        {launchLogs.map((log, idx) => (
                                            <span key={idx} className="log-line">{log}</span>
                                        ))}
                                        <div ref={logsEndRef} style={{ float: "left", clear: "both" }}></div>
                                    </div>
                                )}
                            </div>

                            <div className="modal-footer">
                                {launchError ? (
                                    <>
                                        <div className="footer-tools">
                                            <button onClick={copyErrorToClipboard} className="tool-link">
                                                <Copy size={14} /> 复制
                                            </button>
                                            <button onClick={searchErrorOnline} className="tool-link">
                                                <Search size={14} /> 帮助
                                            </button>
                                        </div>
                                        <button onClick={handleRetry} className="action-btn primary">
                                            <RotateCcw size={16} /> 重试
                                        </button>
                                    </>
                                ) : (
                                    <button onClick={handleClose} className="action-btn ghost">
                                        取消启动
                                    </button>
                                )}
                            </div>
                        </motion.div>
                    </div>
                )}
            </AnimatePresence>
        </div>
    );
};