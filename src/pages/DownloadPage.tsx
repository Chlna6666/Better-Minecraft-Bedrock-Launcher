import React, { useState, useMemo, useRef, useEffect, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useTranslation } from 'react-i18next';
import { open } from '@tauri-apps/plugin-dialog';

// Hooks
import { useMinecraftVersions } from '../hooks/useMinecraftVersions';
import { useToast } from "../components/Toast";

// 组件

import Select from "../components/Select";

// 样式与图标
import './DownloadPage.css';
import {
    Search, Download, RefreshCw, Upload, Box, Layers, Package, ArrowUp,
    ChevronLeft, ChevronRight, Cpu, FileDigit
} from 'lucide-react';

// 图片资源
import releaseIcon from "../assets/img/minecraft/Release.png";
import previewIcon from "../assets/img/minecraft/Preview.png";
import InstallProgressBar from "./Download/InstallProgressBar.tsx";

// ============================================================================
// 1. 分页组件
// ============================================================================
// [i18n] 传入 t 函数用于翻译
const Pagination = React.memo(({ currentPage, totalPages, onPageChange, t, className = "" }) => {
    const [jumpInput, setJumpInput] = useState("");

    useEffect(() => {
        setJumpInput("");
    }, [currentPage]);

    const handleJump = (e) => {
        if (e.key === 'Enter') {
            const page = parseInt(jumpInput, 10);
            if (!isNaN(page) && page >= 1 && page <= totalPages) {
                onPageChange(page);
            }
        }
    };

    const paginationRange = useMemo(() => {
        const range = [];
        const delta = 1;
        const rangeLeft = currentPage - delta;
        const rangeRight = currentPage + delta;

        for (let i = 1; i <= totalPages; i++) {
            if (i === 1 || i === totalPages || (i >= rangeLeft && i <= rangeRight)) {
                range.push(i);
            } else if (i === rangeLeft - 1 || i === rangeRight + 1) {
                range.push("...");
            }
        }
        return range.filter((val, index, arr) => val !== arr[index - 1]);
    }, [currentPage, totalPages]);

    if (totalPages <= 1) return null;

    return (
        <div className={`pagination-container ${className}`}>
            <button
                className="page-btn"
                onClick={() => onPageChange(currentPage - 1)}
                disabled={currentPage === 1}
                title={t('DownloadPage.pagination_prev')} // [i18n]
            >
                <ChevronLeft size={18} />
            </button>

            {paginationRange.map((page, index) => {
                if (page === "...") {
                    return <span key={`dots-${index}`} className="pagination-ellipsis" style={{padding:'0 8px', opacity:0.5}}>...</span>;
                }
                return (
                    <button
                        key={`pg-btn-${page}`}
                        className={`page-btn ${currentPage === page ? 'active' : ''}`}
                        onClick={() => onPageChange(page)}
                    >
                        {page}
                    </button>
                );
            })}

            <button
                className="page-btn"
                onClick={() => onPageChange(currentPage + 1)}
                disabled={currentPage === totalPages}
                title={t('DownloadPage.pagination_next')} // [i18n]
            >
                <ChevronRight size={18} />
            </button>

            {/* 自定义跳转 */}
            <div className="pagination-jumper">
                {t('DownloadPage.pagination_goto')} {/* [i18n] */}
                <input
                    type="number"
                    className="page-input"
                    value={jumpInput}
                    onChange={(e) => setJumpInput(e.target.value)}
                    onKeyDown={handleJump}
                    placeholder={currentPage}
                    min={1}
                    max={totalPages}
                />
            </div>
        </div>
    );
});

// ============================================================================
// 2. 版本行组件
// ============================================================================
const itemVariants = {
    hidden: { opacity: 0, y: 10 },
    visible: { opacity: 1, y: 0 }
};

const VersionRow = React.memo(({
                                   ver, activeDownloadId, isDownloading, onDownload,
                                   onStatusChange, onComplete, onCancel, t
                               }) => {
    const isCurrentDownloading = activeDownloadId === ver.packageId;
    const isRelease = ver.type === 0;
    const isBeta = ver.type === 1;
    const typeClass = isRelease ? 'badge-release' : isBeta ? 'badge-beta' : 'badge-preview';
    const displayIcon = (isRelease || isBeta) ? releaseIcon : previewIcon;

    const safeId = String(ver.packageId || "");
    const displayId = safeId.length > 12 ? `${safeId.substring(0, 12)}...` : (safeId || "Unknown ID");

    // [i18n] 动态获取类型标签
    const typeLabel = (() => {
        switch (ver.type) {
            case 0: return t('common.release');
            case 1: return t('common.beta');
            case 2: return t('common.preview');
            default: return ver.typeStr;
        }
    })();

    // [i18n] 禁用提示信息
    let disabledReason = "";
    if (!ver.metaPresent) disabledReason = t('DownloadPage.no_metadata');
    else if (ver.archivalStatus === 1 || ver.archivalStatus === 0) disabledReason = t('DownloadPage.archival_not_available');

    const isDisabled = isDownloading || !ver.metaPresent || (ver.archivalStatus === 1 || ver.archivalStatus === 0);

    return (
        <motion.div
            variants={itemVariants}
            className="version-row"
        >
            <div className="col-icon">
                <img src={displayIcon} alt="icon" className="version-icon-img" loading="lazy" />
            </div>

            <div className="col-main">
                <div className="row-header">
                    <span className="row-version-number">{ver.version}</span>
                    <span className={`mini-badge ${typeClass}`}>{typeLabel}</span>
                </div>
                <div className="row-sub-info">
                    {/* [i18n] 标签 */}
                    {ver.isGDK ? (
                        <span className="meta-tag tag-gdk">{t('DownloadPage.gdk_build')}</span>
                    ) : (
                        <span className="meta-tag tag-uwp">{t('DownloadPage.uwp_build')}</span>
                    )}
                    <div className="meta-tag" title={t('DownloadPage.architecture')}>
                        <Cpu size={12} /> x64
                    </div>
                    {(ver.archivalStatus === 2) && (
                        <span className="meta-tag" style={{color:'#f59e0b', background: 'rgba(245, 158, 11, 0.1)'}}>
                            {t('DownloadPage.may_unavailable')}
                        </span>
                    )}
                </div>
            </div>

            <div className="col-action">
                {isCurrentDownloading ? (
                    <InstallProgressBar
                        version={ver.version}
                        packageId={ver.packageId}
                        versionType={ver.type}
                        md5={ver.md5}
                        isGDK={ver.isGDK}
                        onStatusChange={onStatusChange}
                        onCompleted={onComplete}
                        onCancel={onCancel}
                    >
                        <button className="download-btn-sm" disabled style={{ width: 110, justifyContent: 'center' }}>
                            {t('common.downloading')}
                        </button>
                    </InstallProgressBar>
                ) : (
                    <button
                        className="download-btn-sm"
                        onClick={() => onDownload(ver)}
                        disabled={isDisabled}
                        title={isDisabled ? disabledReason : t('common.download')}
                    >
                        <Download size={15} />
                        {t('common.download')}
                    </button>
                )}
            </div>
        </motion.div>
    );
});

// ============================================================================
// 3. 主页面组件
// ============================================================================
const containerVariants = {
    hidden: { opacity: 0 },
    visible: {
        opacity: 1,
        transition: {
            staggerChildren: 0.03,
            delayChildren: 0.05
        }
    },
    exit: { opacity: 0 }
};

const DownloadPage = () => {
    const { t } = useTranslation();
    const toast = useToast();

    // Data Hooks
    const { versions: rawVersions, loading, error, reload } = useMinecraftVersions();

    // UI States
    const [activeTab, setActiveTab] = useState('game');
    const [searchTerm, setSearchTerm] = useState("");
    const [filterType, setFilterType] = useState('all');
    const [currentPage, setCurrentPage] = useState(1);
    const PAGE_SIZE = 12;

    // Scroll States
    const [showScrollTop, setShowScrollTop] = useState(false);
    const contentRef = useRef(null);

    // Download / Import States
    const [isDownloading, setIsDownloading] = useState(false);
    const [activeDownloadId, setActiveDownloadId] = useState(null);
    const [isImporting, setIsImporting] = useState(false);
    const [sourcePath, setSourcePath] = useState(null);

    // --- 滚动监听 ---
    useEffect(() => {
        const el = contentRef.current;
        if (!el) return;
        let ticking = false;
        const handleScroll = () => {
            if (!ticking) {
                requestAnimationFrame(() => {
                    setShowScrollTop(el.scrollTop > 300);
                    ticking = false;
                });
                ticking = true;
            }
        };
        el.addEventListener('scroll', handleScroll, { passive: true });
        return () => el.removeEventListener('scroll', handleScroll);
    }, []);

    const scrollToTop = useCallback(() => {
        contentRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
    }, []);

    // --- 数据过滤 ---
    const filteredVersions = useMemo(() => {
        if (!rawVersions) return [];
        return rawVersions.map(v => ({
            version: v[0],
            packageId: v[1],
            type: v[2],
            typeStr: v[3],
            buildType: v[4],
            archivalStatus: v[5],
            metaPresent: v[6],
            md5: v[7],
            isGDK: v[4] === "GDK"
        })).filter(item => {
            if (filterType !== 'all') {
                if (filterType === 'release' && item.type !== 0) return false;
                if (filterType === 'beta' && item.type !== 1) return false;
                if (filterType === 'preview' && item.type !== 2) return false;
            }
            if (searchTerm) {
                const s = searchTerm.toLowerCase();
                const vStr = String(item.version || "").toLowerCase();
                const idStr = String(item.packageId || "").toLowerCase();
                if (!vStr.includes(s) && !idStr.includes(s)) return false;
            }
            return true;
        });
    }, [rawVersions, filterType, searchTerm]);

    // --- 分页切片 ---
    const totalPages = Math.ceil(filteredVersions.length / PAGE_SIZE);

    useEffect(() => { setCurrentPage(1); }, [filterType, searchTerm, activeTab]);

    const paginatedVersions = useMemo(() => {
        const start = (currentPage - 1) * PAGE_SIZE;
        return filteredVersions.slice(start, start + PAGE_SIZE);
    }, [filteredVersions, currentPage]);

    const handlePageChange = useCallback((newPage) => {
        setCurrentPage(newPage);
        scrollToTop();
    }, [scrollToTop]);

    // --- 交互处理器 ---
    const handleDownload = useCallback((ver) => {
        if (isDownloading) return;
        if (!ver.metaPresent) return toast.error(t('DownloadPage.no_metadata'));
        setActiveDownloadId(ver.packageId);
        setIsDownloading(true);
    }, [isDownloading, toast, t]);

    const handleComplete = useCallback(() => { setActiveDownloadId(null); setIsDownloading(false); }, []);

    const handleImport = async () => {
        if (isDownloading) return;
        try {
            const selected = await open({ filters: [{ name: 'Packages', extensions: ['appx', 'zip'] }], multiple: false });
            if (selected) { setSourcePath(selected); setIsImporting(true); setActiveDownloadId('import'); setIsDownloading(true); }
        } catch(e) {}
    };

    // [i18n] 动态筛选器文本
    const filterOptions = useMemo(() => [
        { value: 'all', label: t('common.all_versions') },
        { value: 'release', label: t('common.release') },
        { value: 'beta', label: t('common.beta') },
        { value: 'preview', label: t('common.preview') },
    ], [t]);

    // [i18n] 动态搜索提示
    const searchPlaceholder = useMemo(() => {
        switch (activeTab) {
            case 'game': return t('DownloadPage.search_game');
            case 'mods': return t('DownloadPage.search_mods');
            case 'resource': return t('DownloadPage.search_resource');
            default: return t('DownloadPage.search_default');
        }
    }, [activeTab, t]);

    // 切换 Tab 时重置状态
    useEffect(() => {
        setSearchTerm("");
        setFilterType('all');
        setCurrentPage(1);
        if (contentRef.current) contentRef.current.scrollTo({ top: 0 });
    }, [activeTab]);

    return (
        <div className="download-page-container">
            <div className="bg-shape shape-1" />
            <div className="bg-shape shape-2" />

            {isImporting && activeDownloadId === 'import' && (
                <InstallProgressBar
                    version="Local Import" packageId={null} versionType={-1} isImport={true}
                    sourcePath={sourcePath} onStatusChange={setIsDownloading}
                    onCompleted={handleComplete} onCancel={handleComplete}
                />
            )}

            <div className="unified-glass-panel">
                <div className="panel-header">
                    <div className="tab-switcher">
                        <button className={`tab-btn ${activeTab === 'game' ? 'active' : ''}`} onClick={() => setActiveTab('game')}>
                            <Box size={16} style={{marginRight: 6}}/> {t('DownloadPage.tab_game')}
                        </button>
                        <button className={`tab-btn ${activeTab === 'mods' ? 'active' : ''}`} onClick={() => setActiveTab('mods')}>
                            <Layers size={16} style={{marginRight: 6}}/> {t('DownloadPage.tab_mods')}
                        </button>
                        <button className={`tab-btn ${activeTab === 'resource' ? 'active' : ''}`} onClick={() => setActiveTab('resource')}>
                            <Package size={16} style={{marginRight: 6}}/> {t('DownloadPage.tab_resource')}
                        </button>
                    </div>

                    <div className="actions-group">
                        <div className="search-input-wrapper">
                            <Search className="search-icon" />
                            <input
                                type="text" className="modern-input"
                                placeholder={searchPlaceholder}
                                value={searchTerm} onChange={(e) => setSearchTerm(e.target.value)}
                            />
                        </div>
                        <div style={{width: 130}}>
                            <Select
                                value={filterType} onChange={setFilterType} options={filterOptions}
                                size="md" className="glass-select" dropdownMatchButton={true}
                            />
                        </div>
                        <button className="action-icon-btn" onClick={() => reload()} title={t('DownloadPage.refresh_tooltip')}>
                            <RefreshCw size={18} className={loading ? "spin" : ""} />
                        </button>
                        <button className="action-icon-btn" onClick={handleImport} title={t('DownloadPage.import_tooltip')}>
                            <Upload size={18} />
                        </button>
                    </div>
                </div>

                <div className="panel-content" ref={contentRef}>
                    <AnimatePresence mode="wait">
                        {activeTab === 'game' ? (
                            loading && (!rawVersions || rawVersions.length === 0) ? (
                                <div className="version-list-container">
                                    {[...Array(8)].map((_, i) => (
                                        <div key={i} className="skeleton-row">
                                            <div className="skeleton sk-icon"/>
                                            <div><div className="skeleton sk-line-1"/><div className="skeleton sk-line-2"/></div>
                                            <div className="skeleton sk-meta"/>
                                            <div className="skeleton sk-btn"/>
                                        </div>
                                    ))}
                                </div>
                            ) : (
                                <motion.div
                                    key={currentPage}
                                    variants={containerVariants}
                                    initial="hidden" animate="visible" exit="exit"
                                    className="version-list-container"
                                >
                                    {paginatedVersions.map((ver, idx) => (
                                        <VersionRow
                                            key={`${ver.version}-${ver.packageId || 'no-id'}-${idx}`}
                                            ver={ver}
                                            activeDownloadId={activeDownloadId}
                                            isDownloading={isDownloading}
                                            onDownload={handleDownload}
                                            onStatusChange={setIsDownloading}
                                            onComplete={handleComplete}
                                            onCancel={handleComplete}
                                            t={t}
                                        />
                                    ))}
                                    {filteredVersions.length === 0 && !loading && (
                                        <div style={{padding: 60, textAlign:'center', opacity:0.6}}>
                                            {error ? t('DownloadPage.load_error', {error}) : t('DownloadPage.no_data')}
                                        </div>
                                    )}
                                </motion.div>
                            )
                        ) : (
                            <motion.div initial={{opacity:0}} animate={{opacity:1}} style={{flex:1, display:'flex', flexDirection:'column', alignItems:'center', justifyContent:'center', color:'var(--text-sub)'}}>
                                <Package size={64} style={{opacity:0.3, marginBottom:16}}/>
                                <p>{t('DownloadPage.developing')}</p>
                            </motion.div>
                        )}
                    </AnimatePresence>

                    <AnimatePresence>
                        {showScrollTop && (
                            <motion.button
                                initial={{ opacity: 0, scale: 0.5, y: 20 }}
                                animate={{ opacity: 1, scale: 1, y: 0 }}
                                exit={{ opacity: 0, scale: 0.5, y: 20 }}
                                className="scroll-top-btn"
                                onClick={scrollToTop}
                            >
                                <ArrowUp size={22} />
                            </motion.button>
                        )}
                    </AnimatePresence>
                </div>

                {activeTab === 'game' && filteredVersions.length > 0 && (
                    <Pagination
                        currentPage={currentPage}
                        totalPages={totalPages}
                        onPageChange={handlePageChange}
                        t={t}
                    />
                )}
            </div>
        </div>
    );
};
export default DownloadPage;