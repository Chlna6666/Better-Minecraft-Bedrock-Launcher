import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from 'react-i18next';
import { open } from '@tauri-apps/plugin-dialog';
import { listen } from "@tauri-apps/api/event";
import searchIcon from "../../assets/feather/search.svg";
import refreshIcon from "../../assets/feather/rotate-cw.svg";
import closeIcon from "../../assets/feather/x.svg";
import releaseIcon from "../../assets/img/minecraft/Release.png";
import previewIcon from "../../assets/img/minecraft/Preview.png";
import unknownIcon from "../../assets/feather/box.svg";
import downloadIcon from "../../assets/feather/download.svg";
import uploadIcon from "../../assets/feather/upload.svg"; // 假设存在此图标，如果没有，可以替换为其他或使用文本按钮

import "./DownloadMinecraft.css";
import InstallProgressBar from "./InstallProgressBar.jsx";
import { getConfig } from "../../utils/config.jsx";

// 比较两个版本号，返回 -1, 0, 1
function compareVersion(v1, v2) {
    const parts1 = String(v1).split(".").map(Number);
    const parts2 = String(v2).split(".").map(Number);
    const len = Math.max(parts1.length, parts2.length);
    for (let i = 0; i < len; i++) {
        const num1 = parts1[i] || 0;
        const num2 = parts2[i] || 0;
        if (num1 < num2) return -1;
        if (num1 > num2) return 1;
    }
    return 0;
}

function DownloadMinecraft({ onStatusChange }) {
    const { t } = useTranslation();
    const [versions, setVersions] = useState([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState(null);
    const [searchTerm, setSearchTerm] = useState("");
    const [isSearchOpen, setIsSearchOpen] = useState(false);
    const [showRelease, setShowRelease] = useState(true);
    const [showBeta, setShowBeta] = useState(true);
    const [showPreview, setShowPreview] = useState(true);
    const [displayCount, setDisplayCount] = useState(10);

    const [isDownloading, setIsDownloading] = useState(false);
    const [activeDownload, setActiveDownload] = useState(null);
    const [isImporting, setIsImporting] = useState(false);
    const [sourcePath, setSourcePath] = useState(null);
    const [dragOver, setDragOver] = useState(false);

    const cachedVersions = useRef(null);
    const containerRef = useRef(null);
    const fetchLockRef = useRef(false);

    // 记录取消的 packageId（短时间冷却，防止立即重试）
    const cancelledRef = useRef(new Map());
    const CANCEL_BLOCK_MS = 1500; // ms

    const ROW_HEIGHT = 45;
    const OVERSCAN = 6;
    const [scrollTop, setScrollTop] = useState(0);

    const searchInputRef = useRef(null);

    useEffect(() => {
        fetchVersions(false);
    }, []);

    useEffect(() => {
        onStatusChange && onStatusChange(isDownloading);
    }, [isDownloading, onStatusChange]);

    useEffect(() => {
        if (isSearchOpen && searchInputRef.current) {
            // 等待展开动画后 focus
            const id = setTimeout(() => searchInputRef.current.focus(), 120);
            return () => clearTimeout(id);
        }
    }, [isSearchOpen]);

    // 点击外部关闭搜索
    useEffect(() => {
        const handleDocClick = (e) => {
            if (!isSearchOpen) return;
            if (!searchInputRef.current) return;
            const container = searchInputRef.current.closest('.search-wrapper');
            if (container && !container.contains(e.target)) {
                setIsSearchOpen(false);
            }
        };
        document.addEventListener('mousedown', handleDocClick);
        return () => document.removeEventListener('mousedown', handleDocClick);
    }, [isSearchOpen]);

    const fetchVersions = async (forceRefresh = false) => {
        if (!forceRefresh && cachedVersions.current) {
            setVersions(cachedVersions.current);
            return;
        }
        if (fetchLockRef.current) {
            console.debug("[fetchVersions] 正在进行中，忽略并发请求");
            return;
        }
        fetchLockRef.current = true;
        setLoading(true);
        setError(null);
        try {
            const config = await getConfig();
            const api = config.launcher.custom_appx_api;
            const res = await fetch(api);
            if (!res.ok) throw new Error(res.statusText);
            const data = await res.json();
            setVersions(data || []);
            cachedVersions.current = data || [];
            console.debug("[fetchVersions] 已更新缓存，共有版本数：", (data && data.length) || 0);
        } catch (e) {
            console.error("[fetchVersions] 拉取版本数据失败：", e);
            setError(e.message || String(e));
        } finally {
            setLoading(false);
            fetchLockRef.current = false;
        }
    };

    // 无限滚动：接近底部增加 displayCount
    useEffect(() => {
        const c = containerRef.current;
        if (!c) return;
        const onScrollBottom = () => {
            if (isDownloading) return;
            if (c.scrollTop + c.clientHeight >= c.scrollHeight - 50) {
                setDisplayCount((prev) => prev + 20);
            }
        };
        c.addEventListener("scroll", onScrollBottom);
        return () => c.removeEventListener("scroll", onScrollBottom);
    }, [isDownloading]);

    // 记录 scrollTop 用于虚拟列表
    useEffect(() => {
        const c = containerRef.current;
        if (!c) return;
        const onScroll = () => {
            if (isDownloading) return;
            setScrollTop(c.scrollTop);
        };
        c.addEventListener("scroll", onScroll);
        return () => c.removeEventListener("scroll", onScroll);
    }, [isDownloading]);

    // 修复 passive listener 错误：给容器添加非被动 wheel listener
    useEffect(() => {
        const el = containerRef.current;
        if (!el) return;
        const onWheel = (e) => {
            if (isDownloading) e.preventDefault();
        };
        el.addEventListener("wheel", onWheel, { passive: false });
        return () => el.removeEventListener("wheel", onWheel, { passive: false });
    }, [isDownloading]);

    // 排序与过滤
    const sorted = React.useMemo(
        () => [...versions].sort((a, b) => compareVersion(b[0], a[0])),
        [versions]
    );

    const filtered = React.useMemo(
        () =>
            sorted
                .filter(
                    ([v, , t]) =>
                        (t === 0 ? showRelease : t === 1 ? showBeta : showPreview) &&
                        (!searchTerm || String(v).includes(searchTerm.trim()))
                )
                .slice(0, displayCount),
        [sorted, showRelease, showBeta, showPreview, searchTerm, displayCount]
    );

    // 虚拟列表计算
    const containerHeight = containerRef.current ? containerRef.current.clientHeight : 0;
    const totalCount = filtered.length;
    const visibleCount = containerHeight ? Math.ceil(containerHeight / ROW_HEIGHT) : 10;
    const startIndex = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
    const endIndex = Math.min(totalCount, startIndex + visibleCount + OVERSCAN * 2);
    const topPadding = startIndex * ROW_HEIGHT;
    const bottomPadding = (totalCount - endIndex) * ROW_HEIGHT;
    const visibleList = filtered.slice(startIndex, endIndex);

    const mapVersionTypeToLabel = (type) => {
        switch (type) {
            case 0:
                return t('common.release');
            case 1:
                return t('common.beta');
            case 2:
                return t('common.preview');
            default:
                return t('common.unknown');
        }
    };

    const getVersionIconByType = (type) => {
        if (type === 0 || type === 1) return releaseIcon;
        if (type === 2) return previewIcon;
        return unknownIcon;
    };

    // 当 child 通知取消（带 packageId）
    const handleChildCancel = (pkgId) => {
        if (pkgId) {
            cancelledRef.current.set(pkgId, Date.now());
        }
        setActiveDownload(null);
        setIsDownloading(false);
    };

    // 当 child 通知完成（带 packageId）
    const handleChildCompleted = (pkgId) => {
        if (pkgId && cancelledRef.current.has(pkgId)) {
            cancelledRef.current.delete(pkgId);
        }
        setActiveDownload(null);
        setIsDownloading(false);
    };

    // 点击下载：检查冷却区（刚取消的包短期内不允许立即重试）
    const handleDownloadClick = (pkgId) => {
        if (isDownloading || activeDownload) return;

        const ts = cancelledRef.current.get(pkgId);
        if (ts && Date.now() - ts < CANCEL_BLOCK_MS) {
            // 冷却期内忽略点击（你也可以在这里提示用户）
            console.debug("[handleDownloadClick] 点击被忽略：刚被取消，等待冷却", pkgId);
            return;
        }
        if (ts) cancelledRef.current.delete(pkgId);

        setActiveDownload(pkgId);
        // InstallProgressBar 会通过 onStatusChange(true) 通知父组件正在下载
    };

    const handleImportClick = async () => {
        if (isDownloading || activeDownload) return;
        const selected = await open({
            filters: [{ name: 'Packages', extensions: ['appx', 'zip'] }],
            multiple: false,
        });
        if (selected) {
            setSourcePath(selected);
            setIsImporting(true);
            setActiveDownload('import'); // 使用特殊值禁用其他操作
        }
    };

    const handleDragOver = (e) => {
        if (isDownloading || activeDownload) return;
        e.preventDefault();
        setDragOver(true);
    };

    const handleDragLeave = () => {
        setDragOver(false);
    };

    //
    const handleDrop = (e) => {
        if (isDownloading || activeDownload) return;
        e.preventDefault();
        setDragOver(false);
        const files = e.dataTransfer.files;
        if (files.length > 0) {
            const file = files[0];
            const ext = file.name.toLowerCase().split('.').pop();
            if (ext === 'appx' || ext === 'zip') {
                setSourcePath(file.path || file.name);
                setIsImporting(true);
                setActiveDownload('import');
            } else {
                console.warn("Unsupported file type for import");
            }
        }
    };

    return (
        <div
            className={`container ${dragOver ? 'drag-over' : ''}`}
            ref={containerRef}
            onDragOver={handleDragOver}
            onDragLeave={handleDragLeave}
            onDrop={handleDrop}
        >
            {/* 操作区域：使用相对定位，右侧放置 Refresh（始终固定），搜索采用绝对定位展开到左侧，不影响其他元素布局 */}
            <div
                className="control-bar"
                style={{
                    pointerEvents: (isDownloading || !!activeDownload) ? "none" : "auto",
                    opacity: (isDownloading || !!activeDownload) ? 0.5 : 1,
                }}
            >
                <div className="filter-container">
                    <label className="checkbox-label">
                        <input
                            type="checkbox"
                            checked={showRelease}
                            onChange={() => setShowRelease(!showRelease)}
                            className="checkbox"
                        />
                        {t('common.release')}
                    </label>
                    <label className="checkbox-label">
                        <input
                            type="checkbox"
                            checked={showBeta}
                            onChange={() => setShowBeta(!showBeta)}
                            className="checkbox"
                        />
                        {t('common.beta')}
                    </label>
                    <label className="checkbox-label">
                        <input
                            type="checkbox"
                            checked={showPreview}
                            onChange={() => setShowPreview(!showPreview)}
                            className="checkbox"
                        />
                        {t('common.preview')}
                    </label>
                </div>

                {/* 中间保留占位（防止布局被覆盖时看起来乱），但搜索实际使用绝对定位展开 */}
                <div className="middle-placeholder" aria-hidden />

                <div className="actions">
                    {/* 搜索 */}
                    <div className={`search-wrapper ${isSearchOpen ? 'open' : 'closed'}`}>
                        {!isSearchOpen && (
                            <img
                                src={searchIcon}
                                alt="Search"
                                className="search-icon-btn"
                                onClick={() => setIsSearchOpen(true)}
                                title={t('DownloadMinecraft.search')}
                            />
                        )}
                        <div className="search-container">
                            <input
                                ref={searchInputRef}
                                type="text"
                                placeholder={t('DownloadMinecraft.search_placeholder')}
                                value={searchTerm}
                                onChange={(e) => setSearchTerm(e.target.value)}
                                className="search-input"
                                onKeyDown={(e) => {
                                    if (e.key === 'Escape') {
                                        setIsSearchOpen(false);
                                    }
                                }}
                            />
                            <button
                                className="close-search-btn"
                                onClick={() => { setIsSearchOpen(false); setSearchTerm(''); }}
                                aria-label="Close search"
                            >
                                <img src={closeIcon} alt="Close" />
                            </button>
                        </div>
                    </div>

                    {/* 导入 */}
                    <div className="action-btn" onClick={handleImportClick} title={t('DownloadMinecraft.import')}>
                        <img src={uploadIcon} alt="Import" />
                    </div>

                    {/* 刷新 */}
                    <div
                        className="action-btn"
                        onClick={() => { if (!loading) fetchVersions(true); }}
                        title={loading ? t('DownloadMinecraft.refresh_loading') : t('DownloadMinecraft.refresh')}
                        style={{ opacity: loading ? 0.6 : 1 }}
                    >
                        <img src={refreshIcon} alt="Refresh" />
                    </div>
                </div>

            </div>

            {/* 版本列表 */}
            <div className="table">
                <div style={{ paddingTop: topPadding, paddingBottom: bottomPadding }}>
                    {visibleList.map(([version, pkgId, type], idx) => {
                        const key = pkgId || version || idx;
                        return (
                            <div key={key} className="table-row" style={{ height: ROW_HEIGHT }}>
                                <div className="table-icon-cell">
                                    <img src={getVersionIconByType(type)} alt="Version" className="version-icon" />
                                </div>
                                <div className="table-cell">
                                    <div className="table-version-number">{version}</div>
                                    <div className="table-version-type">{mapVersionTypeToLabel(type)}</div>
                                </div>
                                <div className="table-download-cell">
                                    {activeDownload === pkgId ? (
                                        <InstallProgressBar
                                            key={pkgId}
                                            version={version}
                                            packageId={pkgId}
                                            versionType={type}
                                            onStatusChange={setIsDownloading}
                                            onCompleted={(id) => handleChildCompleted(id)}
                                            onCancel={(id) => handleChildCancel(id)}
                                        >
                                            <button className="download-button" disabled>
                                                <img src={downloadIcon} alt="Download" className="download-icon" />
                                            </button>
                                        </InstallProgressBar>
                                    ) : (
                                        <button
                                            className="download-button"
                                            onClick={() => handleDownloadClick(pkgId)}
                                            disabled={isDownloading || !!activeDownload}
                                        >
                                            <img src={downloadIcon} alt="Download" className="download-icon" />
                                        </button>
                                    )}
                                </div>
                            </div>
                        );
                    })}
                </div>

                {/* 空数据或错误提示 */}
                {filtered.length === 0 && (
                    <div className="no-data">
                        {t('DownloadMinecraft.no_data')}
                        {loading && (
                            <div className="info">
                                {t('DownloadMinecraft.info')}
                            </div>
                        )}
                        {error && (
                            <div className="info error">
                                {t('DownloadMinecraft.info_error', { error })}
                            </div>
                        )}
                    </div>
                )}
            </div>

            {/* 导入进度条（独立于列表） */}
            {isImporting && (
                <InstallProgressBar
                    version="Imported"
                    packageId={null}
                    versionType={-1}
                    isImport={true}
                    sourcePath={sourcePath}
                    onStatusChange={setIsDownloading}
                    onCompleted={() => {
                        setIsImporting(false);
                        setActiveDownload(null);
                        setSourcePath(null);
                    }}
                    onCancel={() => {
                        setIsImporting(false);
                        setActiveDownload(null);
                        setSourcePath(null);
                    }}
                />
            )}
        </div>
    );
}

export default DownloadMinecraft;