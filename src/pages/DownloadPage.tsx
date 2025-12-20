import React, { useState, useMemo, useCallback, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useTranslation } from 'react-i18next';
import { open } from '@tauri-apps/plugin-dialog';

// Hooks
import { useMinecraftVersions } from '../hooks/useMinecraftVersions';
import { useToast } from "../components/Toast";

// Components
import Select from "../components/Select";
import InstallProgressBar from "./Download/InstallProgressBar.tsx";
import UnifiedPageLayout from '../components/UnifiedPageLayout/UnifiedPageLayout';

// Icons & Styles
import { Download, Upload, Box, Layers, Package, Cpu } from 'lucide-react';
import './DownloadPage.css';

// Assets
import releaseIcon from "../assets/img/minecraft/Release.png";
import previewIcon from "../assets/img/minecraft/Preview.png";

// ============================================================================
// 1. 版本行组件 (VersionRow) - 保持不变
// ============================================================================
const itemVariants = {
    hidden: { opacity: 0, y: 10 },
    visible: { opacity: 1, y: 0 }
};

const VersionRow = React.memo(({
                                   ver, activeDownloadId, isDownloading, onDownload,
                                   onStatusChange, onComplete, onCancel, t
                               }: any) => {
    const isCurrentDownloading = activeDownloadId === ver.packageId;
    const isRelease = ver.type === 0;
    const isBeta = ver.type === 1;
    const typeClass = isRelease ? 'badge-release' : isBeta ? 'badge-beta' : 'badge-preview';
    const displayIcon = (isRelease || isBeta) ? releaseIcon : previewIcon;

    const typeLabel = (() => {
        switch (ver.type) {
            case 0: return t('common.release');
            case 1: return t('common.beta');
            case 2: return t('common.preview');
            default: return ver.typeStr;
        }
    })();

    let disabledReason = "";
    if (!ver.metaPresent) disabledReason = t('DownloadPage.no_metadata');
    else if (ver.archivalStatus === 1 || ver.archivalStatus === 0) disabledReason = t('DownloadPage.archival_not_available');

    const isDisabled = isDownloading || !ver.metaPresent || (ver.archivalStatus === 1 || ver.archivalStatus === 0);

    return (
        <motion.div variants={itemVariants} className="version-row">
            <div className="col-icon">
                <img src={displayIcon} alt="icon" className="version-icon-img" loading="lazy" />
            </div>

            <div className="col-main">
                <div className="row-header">
                    <span className="row-version-number">{ver.version}</span>
                    <span className={`mini-badge ${typeClass}`}>{typeLabel}</span>
                </div>
                <div className="row-sub-info">
                    {ver.isGDK ? (
                        <span className="meta-tag tag-gdk">{t('DownloadPage.gdk_build')}</span>
                    ) : (
                        <span className="meta-tag tag-uwp">{t('DownloadPage.uwp_build')}</span>
                    )}
                    <div className="meta-tag" title={t('DownloadPage.architecture')}>
                        <Cpu size={12} /> x64
                    </div>
                    {(ver.archivalStatus === 2) && (
                        <span className="meta-tag" style={{ color: '#f59e0b', background: 'rgba(245, 158, 11, 0.1)' }}>
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
// 2. 主页面组件 (DownloadPage)
// ============================================================================
const containerVariants = {
    hidden: { opacity: 0 },
    visible: {
        opacity: 1,
        transition: { staggerChildren: 0.03, delayChildren: 0.05 }
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

    // Download / Import States
    const [isDownloading, setIsDownloading] = useState(false);
    const [activeDownloadId, setActiveDownloadId] = useState<string | null>(null);
    const [isImporting, setIsImporting] = useState(false);
    const [sourcePath, setSourcePath] = useState<string | null>(null);

    // --- 数据处理逻辑 ---
    const filteredVersions = useMemo(() => {
        if (!rawVersions) return [];
        return rawVersions.map((v: any) => ({
            version: v[0],
            packageId: v[1],
            type: v[2],
            typeStr: v[3],
            buildType: v[4],
            archivalStatus: v[5],
            metaPresent: v[6],
            md5: v[7],
            isGDK: v[4] === "GDK"
        })).filter((item: any) => {
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

    const totalPages = Math.ceil(filteredVersions.length / PAGE_SIZE);

    useEffect(() => { setCurrentPage(1); }, [filterType, searchTerm, activeTab]);

    const paginatedVersions = useMemo(() => {
        const start = (currentPage - 1) * PAGE_SIZE;
        return filteredVersions.slice(start, start + PAGE_SIZE);
    }, [filteredVersions, currentPage]);

    const handlePageChange = useCallback((newPage: number) => {
        setCurrentPage(newPage);
        // Layout组件会自动处理滚动回顶部，这里只需要更新页码
    }, []);

    const handleDownload = useCallback((ver: any) => {
        if (isDownloading) return;
        if (!ver.metaPresent) return toast.error(t('DownloadPage.no_metadata'));
        setActiveDownloadId(ver.packageId);
        setIsDownloading(true);
    }, [isDownloading, toast, t]);

    const handleComplete = useCallback(() => { setActiveDownloadId(null); setIsDownloading(false); }, []);

    const handleImport = async () => {
        if (isDownloading) return;
        try {
            const selected = await open({ filters: [{ name: 'Packages', extensions: ['appx', 'zip', 'msixvc'] }], multiple: false });
            if (selected) { setSourcePath(selected as string); setIsImporting(true); setActiveDownloadId('import'); setIsDownloading(true); }
        } catch (e) { }
    };

    // --- 配置 UnifiedPageLayout Props ---

    const tabs = [
        { id: 'game', label: t('DownloadPage.tab_game'), icon: <Box size={16} /> },
        { id: 'mods', label: t('DownloadPage.tab_mods'), icon: <Layers size={16} /> },
        { id: 'resource', label: t('DownloadPage.tab_resource'), icon: <Package size={16} /> },
    ];

    const filterOptions = useMemo(() => [
        { value: 'all', label: t('common.all_versions') },
        { value: 'release', label: t('common.release') },
        { value: 'beta', label: t('common.beta') },
        { value: 'preview', label: t('common.preview') },
    ], [t]);

    const searchPlaceholder = useMemo(() => {
        switch (activeTab) {
            case 'game': return t('DownloadPage.search_game');
            case 'mods': return t('DownloadPage.search_mods');
            case 'resource': return t('DownloadPage.search_resource');
            default: return t('DownloadPage.search_default');
        }
    }, [activeTab, t]);

    // 1. 搜索配置
    const searchConfig = {
        value: searchTerm,
        onChange: (v: string) => setSearchTerm(v),
        placeholder: searchPlaceholder
    };

    // 2. 刷新配置
    const refreshConfig = {
        onRefresh: reload,
        loading: loading,
        title: t('DownloadPage.refresh_tooltip')
    };

    // 3. 额外的 Header 动作 (筛选 + 导入)
    const headerActions = (
        <>
            <div style={{ width: 130 }}>
                <Select
                    value={filterType} onChange={setFilterType} options={filterOptions}
                    size="md" className="glass-select" dropdownMatchButton={true}
                />
            </div>
            <button className="upl-action-icon-btn" onClick={handleImport} title={t('DownloadPage.import_tooltip')}>
                <Upload size={18} />
            </button>
        </>
    );

    // 4. 分页配置
    const paginationConfig = (activeTab === 'game' && filteredVersions.length > 0) ? {
        currentPage,
        totalPages,
        onPageChange: handlePageChange,
        t
    } : undefined;

    return (
        <>
            {/* 导入进度条 (独立于布局) */}
            {isImporting && activeDownloadId === 'import' && (
                <InstallProgressBar
                    version="Local Import" packageId={null} versionType={-1} isImport={true}
                    sourcePath={sourcePath} onStatusChange={setIsDownloading}
                    onCompleted={handleComplete} onCancel={handleComplete}
                >
                    <></>
                </InstallProgressBar>
            )}

            <UnifiedPageLayout
                activeTab={activeTab}
                onTabChange={setActiveTab}
                tabs={tabs}
                // 传入新配置
                searchConfig={searchConfig}
                refreshConfig={refreshConfig}
                enableScrollTop={true} // 启用回到顶部
                headerActions={headerActions}
                useInnerContainer={false} // 我们自己控制列表容器
                pagination={paginationConfig}
            >
                <div style={{ display: 'flex', flexDirection: 'column', minHeight: '100%' }}>
                    <AnimatePresence mode="wait">
                        {activeTab === 'game' ? (
                            loading && (!rawVersions || rawVersions.length === 0) ? (
                                // Loading Skeleton
                                <div className="version-list-container" style={{ padding: 20 }}>
                                    {[...Array(8)].map((_, i) => (
                                        <div key={i} className="skeleton-row" style={{
                                            display: 'flex', alignItems: 'center', height: 76,
                                            borderBottom: '1px solid rgba(0,0,0,0.05)', padding: '0 24px'
                                        }}>
                                            <div style={{ width: 42, height: 42, background: 'rgba(0,0,0,0.05)', borderRadius: 10, marginRight: 24 }} />
                                            <div style={{ flex: 1 }}>
                                                <div style={{ width: 120, height: 20, background: 'rgba(0,0,0,0.05)', borderRadius: 4, marginBottom: 8 }} />
                                                <div style={{ width: 80, height: 14, background: 'rgba(0,0,0,0.05)', borderRadius: 4 }} />
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            ) : (
                                // Data List
                                <motion.div
                                    key={currentPage}
                                    variants={containerVariants}
                                    initial="hidden" animate="visible" exit="exit"
                                    className="version-list-container"
                                    style={{ paddingBottom: 20 }}
                                >
                                    {paginatedVersions.map((ver: any, idx: number) => (
                                        <VersionRow
                                            key={`${ver.version}-${ver.packageId}-${idx}`}
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
                                        <div style={{ padding: 60, textAlign: 'center', opacity: 0.6 }}>
                                            {error ? t('DownloadPage.load_error', { error }) : t('DownloadPage.no_data')}
                                        </div>
                                    )}
                                </motion.div>
                            )
                        ) : (
                            // Other Tabs
                            <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} style={{ flex: 1, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', color: 'var(--upl-text-sub)', paddingTop: 100 }}>
                                <Package size={64} style={{ opacity: 0.3, marginBottom: 16 }} />
                                <p>{t('DownloadPage.developing')}</p>
                            </motion.div>
                        )}
                    </AnimatePresence>
                </div>
            </UnifiedPageLayout>
        </>
    );
};

export default DownloadPage;