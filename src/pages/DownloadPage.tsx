import React, { useState, useMemo, useCallback, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';

// Hooks
import { useMinecraftVersions } from '../hooks/useMinecraftVersions';
import { useToast } from "../components/Toast";

// Components
import Select from "../components/Select";
import InstallProgressBar from "./Download/InstallProgressBar";
import UnifiedPageLayout from '../components/UnifiedPageLayout/UnifiedPageLayout';
import Modal from "../components/Modal";
import Button from "../components/Button";

const CurseForgeBrowser = React.lazy(() =>
    import('./Download/CurseForge/CurseForgeBrowser').then((m: any) => ({ default: m.CurseForgeBrowser }))
);

// Icons & Styles
import { Download, Upload, Box, Layers, Package, Cpu, RotateCcw, Trash2, ChevronRight } from 'lucide-react';
import './DownloadPage.css';

// Assets
import releaseIcon from "../assets/img/minecraft/Release.png";
import previewIcon from "../assets/img/minecraft/Preview.png";
import {useLocation} from "react-router-dom";

// ============================================================================
// 1. 版本行组件 (保持不变)
// ============================================================================
const VersionRow = React.memo(({ ver, activeDownloadId, isDownloading, onDownload, onStatusChange, onComplete, onCancel, t, localPathMap, forceDownloadMap }: any) => {
    const isCurrentDownloading = activeDownloadId === ver.packageId;
    const isRelease = ver.type === 0;
    const isBeta = ver.type === 1;
    const typeClass = isRelease ? 'badge-release' : 'badge-preview';
    const displayIcon = (isRelease || isBeta) ? releaseIcon : previewIcon;
    const typeLabel = ver.type === 0 ? t('common.release') : ver.type === 1 ? t('common.beta') : t('common.preview');
    const isDisabled = isDownloading || !ver.metaPresent || (ver.archivalStatus === 1 || ver.archivalStatus === 0);
    const localPath = localPathMap?.[ver.packageId] || null;
    const isLocalReady = !!localPath;
    const forceDownload = !!forceDownloadMap?.[ver.packageId];

    return (
        <div className="version-row">
            <div className="col-icon">
                <img src={displayIcon} alt="icon" className="version-icon-img" loading="lazy" />
            </div>
            <div className="col-main">
                <div className="row-header">
                    <span className="row-version-number">{ver.version}</span>
                    <span className={`mini-badge ${typeClass}`}>{typeLabel}</span>
                </div>
                <div className="row-sub-info">
                    {ver.isGDK && <span className="meta-tag tag-gdk">{t('common.gdk')}</span>}
                    <div className="meta-tag tag-cpu"><Cpu size={11} style={{ marginRight: 3 }}/> x64</div>
                    {!ver.isGDK && <span className="meta-tag tag-uwp">{t('common.uwp')}</span>}
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
                        autoExtractPath={forceDownload ? null : localPath}
                        forceDownload={forceDownload}
                        onStatusChange={onStatusChange}
                        onCompleted={onComplete}
                        onCancel={onCancel}
                    >
                        <button className="download-btn-sm" disabled style={{ width: 110, justifyContent: 'center' }}>{t('common.downloading')}</button>
                    </InstallProgressBar>
                ) : (
                    <button className="download-btn-sm" onClick={() => onDownload(ver)} disabled={isDisabled}>
                        <Download size={16} strokeWidth={2.5} />
                        {isLocalReady ? (t('common.install') || 'Install') : t('common.download')}
                    </button>
                )}
            </div>
        </div>
    );
});

// ============================================================================
// 主页面组件
// ============================================================================
const DownloadPage = () => {
    const { t } = useTranslation();
    const toast = useToast();

    const location = useLocation(); // 获取路由状态

    // Data Hooks
    const { versions: rawVersions, loading: gameLoading, reload: reloadGame } = useMinecraftVersions();

    // UI States
    const [activeTab, setActiveTab] = useState(() => {
        return (location.state as any)?.initialTab || 'game';
    });
    const [searchTerm, setSearchTerm] = useState(() => {
        return (location.state as any)?.searchTerm || "";
    });

    // Game Tab States
    const [filterType, setFilterType] = useState('release');
    const [currentPage, setCurrentPage] = useState(1);
    const PAGE_SIZE = 8;
    const [gameRefreshNonce, setGameRefreshNonce] = useState(0);
    const [gameRefreshing, setGameRefreshing] = useState(false);

    // Resource Tab States
    const [cfRefreshNonce, setCfRefreshNonce] = useState(0);
    const [cfLoading, setCfLoading] = useState(false);

    // Download States
    const [isDownloading, setIsDownloading] = useState(false);
    const [activeDownloadId, setActiveDownloadId] = useState<string | null>(null);
    const [isImporting, setIsImporting] = useState(false);
    const [sourcePath, setSourcePath] = useState<string | null>(null);
    const [localPathMap, setLocalPathMap] = useState<Record<string, string | null>>({});
    const [forceDownloadMap, setForceDownloadMap] = useState<Record<string, boolean>>({});
    const [localActionsOpen, setLocalActionsOpen] = useState(false);
    const [localActionsVer, setLocalActionsVer] = useState<any | null>(null);
    const [localActionsBusy, setLocalActionsBusy] = useState(false);
    const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

    // Filter Logic for Game Tab
    const filteredVersions = useMemo(() => {
        if (!rawVersions) return [];
        return rawVersions.map((v: any) => ({
            version: v[0], packageId: v[1], type: v[2], typeStr: v[3], buildType: v[4],
            archivalStatus: v[5], metaPresent: v[6], md5: v[7], isGDK: String(v[4] || "").toLowerCase() === "gdk"
        })).filter((item: any) => {
            if (filterType !== 'all') {
                if (filterType === 'release' && item.type !== 0) return false;
                if (filterType === 'beta' && item.type !== 1) return false;
                if (filterType === 'preview' && item.type !== 2) return false;
            }
            if (searchTerm && activeTab === 'game') {
                const s = searchTerm.toLowerCase();
                if (!String(item.version).toLowerCase().includes(s) && !String(item.packageId).toLowerCase().includes(s)) return false;
            }
            return true;
        });
    }, [rawVersions, filterType, searchTerm, activeTab]);

    const gameTotalPages = Math.ceil(filteredVersions.length / PAGE_SIZE);
    const paginatedVersions = useMemo(() => {
        const start = (currentPage - 1) * PAGE_SIZE;
        return filteredVersions.slice(start, start + PAGE_SIZE);
    }, [filteredVersions, currentPage]);

    const getLocalPackageFileName = useCallback((ver: any) => {
        const ext = ver?.isGDK ? ".msixvc" : ".appx";
        return `${ver.version}${ext}`;
    }, []);

    useEffect(() => {
        let cancelled = false;
        const run = async () => {
            if (activeTab !== 'game') return;
            if (!paginatedVersions || paginatedVersions.length === 0) return;

            const updates: Record<string, string | null> = {};
            for (const ver of paginatedVersions) {
                try {
                    const fileName = getLocalPackageFileName(ver);
                    // Fast path: check existence only (no MD5 hashing) so the UI can show "Install" immediately.
                    const p = await invoke<string | null>("local_download_path", { fileName, md5: null });
                    updates[ver.packageId] = p;
                } catch {
                    updates[ver.packageId] = null;
                }
            }

            if (cancelled) return;
            setLocalPathMap((prev) => ({ ...prev, ...updates }));
        };
        run();
        return () => { cancelled = true; };
    }, [activeTab, paginatedVersions, getLocalPackageFileName]);

    useEffect(() => {
        if (activeTab === 'game') setCurrentPage(1);
        setSearchTerm("");
    }, [activeTab]);

    useEffect(() => { setCurrentPage(1); }, [filterType, searchTerm]);

    useEffect(() => {
        if (!gameLoading) setGameRefreshing(false);
    }, [gameLoading]);

    const handleGameDownload = useCallback((ver: any) => {
        if (isDownloading) return;
        if (!ver.metaPresent) return toast.error(t('DownloadPage.no_metadata'));
        setActiveDownloadId(ver.packageId);
        setIsDownloading(true);
    }, [isDownloading, toast, t]);

    const handleGameAction = useCallback((ver: any) => {
        if (isDownloading) return;
        if (!ver.metaPresent) return toast.error(t('DownloadPage.no_metadata'));
        const localPath = localPathMap?.[ver.packageId] || null;
        if (localPath) {
            setLocalActionsVer(ver);
            setLocalActionsOpen(true);
            return;
        }
        setActiveDownloadId(ver.packageId);
        setIsDownloading(true);
    }, [isDownloading, toast, t, localPathMap]);

    const handleComplete = useCallback((packageId?: string | null) => {
        setActiveDownloadId(null);
        setIsDownloading(false);
        if (packageId) {
            setForceDownloadMap((prev) => {
                if (!prev[packageId]) return prev;
                const next = { ...prev };
                delete next[packageId];
                return next;
            });
        }
    }, []);

    const handleImport = async () => {
        if (isDownloading) return;
        try {
            const selected = await open({ filters: [{ name: 'Packages', extensions: ['appx', 'zip', 'msixvc'] }], multiple: false });
            if (selected) { setSourcePath(selected as string); setIsImporting(true); setActiveDownloadId('import'); setIsDownloading(true); }
        } catch (e) { }
    };

    const handleRefresh = useCallback(() => {
        if (activeTab === 'game') {
            setGameRefreshNonce((v) => v + 1);
            setGameRefreshing(true);
            reloadGame();
            return;
        }
        if (activeTab === 'resource') {
            setCfRefreshNonce((v) => v + 1);
        }
    }, [activeTab, reloadGame]);

    // [优化] 使用 useMemo 缓存配置对象
    const tabs = useMemo(() => [
        { id: 'game', label: t('DownloadPage.tab_game'), icon: <Box size={18} /> },
        { id: 'resource', label: t('DownloadPage.tab_resource'), icon: <Package size={18} /> },
        { id: 'mods', label: t('DownloadPage.tab_mods'), icon: <Layers size={18} /> },
    ], [t]);

    const filterOptions = useMemo(() => [
        { value: 'all', label: t('common.all_versions') },
        { value: 'release', label: t('common.release') },
        { value: 'beta', label: t('common.beta') },
        { value: 'preview', label: t('common.preview') },
    ], [t]);

    // [布局关键] 顶部操作区配置
    const getHeaderActions = () => {
        if (activeTab === 'game') {
            return (
                <div style={{ display: 'flex', gap: 10, alignItems: 'center' }}>
                    <div style={{ width: 120 }}>
                        <Select
                            value={filterType}
                            onChange={setFilterType}
                            options={filterOptions}
                            size="md"
                            className="glass-select"
                            dropdownMatchButton={true}
                        />
                    </div>
                    <button className="upl-action-icon-btn" onClick={handleImport} data-bm-title={t('DownloadPage.import_tooltip')}>
                        <Upload size={18} />
                    </button>
                </div>
            );
        } else if (activeTab === 'resource') {
            return (
                <div style={{ display: 'flex', alignItems: 'center', height: '100%' }}>
                    <div style={{
                        width: 1,
                        height: 24,
                        background: 'var(--text-sub)',
                        opacity: 0.2,
                        marginRight: 16
                    }}></div>
                    <div id="cf-header-slot" style={{ display: 'flex', gap: 12, alignItems: 'center' }}></div>
                </div>
            );
        }
        return null;
    };

    const getPaginationConfig = () => {
        if (activeTab === 'game' && filteredVersions.length > 0) {
            return { currentPage, totalPages: gameTotalPages, onPageChange: setCurrentPage, t };
        }
        return undefined;
    };

    const localActionsPath = localActionsVer?.packageId ? (localPathMap?.[localActionsVer.packageId] || null) : null;
    const localActionsFileName = localActionsVer ? getLocalPackageFileName(localActionsVer) : null;

    const closeLocalActions = useCallback(() => {
        if (localActionsBusy) return;
        setLocalActionsOpen(false);
        setLocalActionsVer(null);
    }, [localActionsBusy]);

    const startInstall = useCallback((ver: any) => {
        setLocalActionsOpen(false);
        setLocalActionsVer(null);
        setActiveDownloadId(ver.packageId);
        setIsDownloading(true);
    }, []);

    const handleLocalInstall = useCallback(() => {
        if (!localActionsVer || !localActionsPath) return;
        startInstall(localActionsVer);
    }, [localActionsVer, localActionsPath, startInstall]);

    const handleRedownload = useCallback(async () => {
        if (!localActionsVer || !localActionsFileName) return;
        setForceDownloadMap((prev) => ({ ...prev, [localActionsVer.packageId]: true }));
        startInstall(localActionsVer);
    }, [localActionsVer, localActionsFileName, startInstall, toast]);

    const handleDeleteLocal = useCallback(async () => {
        if (!localActionsVer || !localActionsFileName) return;
        setLocalActionsOpen(false);
        setDeleteConfirmOpen(true);
    }, [localActionsVer, localActionsFileName]);

    const handleConfirmDeleteLocal = useCallback(async () => {
        if (!localActionsVer || !localActionsFileName) return;
        setLocalActionsBusy(true);
        try {
            await invoke("delete_local_download", { fileName: localActionsFileName });
            setLocalPathMap((prev) => ({ ...prev, [localActionsVer.packageId]: null }));
            setDeleteConfirmOpen(false);
            setLocalActionsVer(null);
        } catch (e: any) {
            toast?.error(e?.message || String(e));
        } finally {
            setLocalActionsBusy(false);
        }
    }, [localActionsVer, localActionsFileName, toast]);

    return (
        <>
            {isImporting && activeDownloadId === 'import' && (
                <InstallProgressBar version={t('DownloadPage.local_import')} packageId={null} versionType={-1} isImport={true} sourcePath={sourcePath} onStatusChange={setIsDownloading} onCompleted={handleComplete} onCancel={handleComplete}><></></InstallProgressBar>
            )}

            <UnifiedPageLayout
                activeTab={activeTab}
                onTabChange={setActiveTab}
                tabs={tabs}
                searchConfig={{
                    value: searchTerm,
                    onChange: setSearchTerm,
                    placeholder: activeTab === 'resource' ? t('DownloadPage.search_curseforge') : t('DownloadPage.search_game')
                }}
                refreshConfig={{
                    onRefresh: handleRefresh,
                    loading: activeTab === 'game' ? gameLoading : activeTab === 'resource' ? cfLoading : false,
                    title: t('DownloadPage.refresh_tooltip')
                }}
                enableScrollTop={true}
                headerActions={getHeaderActions()}
                useInnerContainer={false}
                pagination={getPaginationConfig()}
                hideScrollbar={activeTab === 'resource'}
            >
                <div style={{ display: 'flex', flexDirection: 'column', height: '100%', overflow: 'hidden' }}>
                    {/* Game Tab */}
                    {activeTab === 'game' && (
                        <div key={`game-tab-${gameRefreshNonce}`} style={{ flex: 1, overflowY: 'auto', paddingBottom: 20 }} className="custom-scrollbar bm-anim-page-in">
                            {gameLoading && (gameRefreshing || !rawVersions || rawVersions.length === 0) ? (
                                <div className="version-list-container" style={{ padding: 20 }}>
                                    {[...Array(PAGE_SIZE)].map((_, i) => (
                                        <div key={i} className="version-row skeleton-row">
                                            <div className="col-icon">
                                                <div className="skeleton sk-icon" />
                                            </div>
                                            <div className="col-main">
                                                <div className="row-header">
                                                    <div className="skeleton sk-ver" />
                                                    <div className="skeleton sk-badge" />
                                                </div>
                                                <div className="row-sub-info">
                                                    <div className="skeleton sk-tag sk-tag-lg" />
                                                    <div className="skeleton sk-tag" />
                                                    <div className="skeleton sk-tag" />
                                                </div>
                                            </div>
                                            <div className="col-action">
                                                <div className="skeleton sk-btn" />
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            ) : (
                                <div className="version-list-container">
                                    {paginatedVersions.map((ver: any, idx: number) => (
                                        <VersionRow key={`${ver.version}-${idx}`} ver={ver} activeDownloadId={activeDownloadId} isDownloading={isDownloading} onDownload={handleGameAction} onStatusChange={setIsDownloading} onComplete={handleComplete} onCancel={handleComplete} t={t} localPathMap={localPathMap} forceDownloadMap={forceDownloadMap} />
                                    ))}
                                </div>
                            )}
                        </div>
                    )}

                    {/* Resource Tab */}
                    {activeTab === 'resource' && (
                        <div key="resource-tab" style={{ flex: 1, height: '100%' }} className="bm-anim-page-in">
                            <React.Suspense fallback={<div style={{ height: '100%' }} />}>
                                <CurseForgeBrowser
                                    searchQuery={searchTerm}
                                    onClearSearch={() => setSearchTerm("")}
                                    refreshNonce={cfRefreshNonce}
                                    onLoadingChange={setCfLoading}
                                />
                            </React.Suspense>
                        </div>
                    )}

                    {/* Mods Tab */}
                    {activeTab === 'mods' && (
                        <div key="mods-tab" style={{ flex: 1, height: '100%', display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', color: 'var(--text-sub)' }} className="bm-anim-page-in">
                            <Package size={64} style={{ opacity: 0.2, marginBottom: 16 }} />
                            <p style={{ opacity: 0.6 }}>{t('DownloadPage.developing')}</p>
                        </div>
                    )}
                </div>
            </UnifiedPageLayout>

            <Modal
                open={localActionsOpen}
                title={t("DownloadPage.local_package_actions_title")}
                onClose={closeLocalActions}
                width="560px"
            >
                <div className="dp-local-actions">
                    <div className="dp-local-actions-desc">{t("DownloadPage.local_package_actions_desc")}</div>

                    <div className="dp-local-actions-meta">
                        <div className="dp-local-actions-meta-left">
                            <div className="dp-local-actions-name">{localActionsVer?.version || "-"}</div>
                            {localActionsPath ? <div className="dp-local-actions-path" title={localActionsPath}>{localActionsPath}</div> : null}
                        </div>
                        <div className="dp-local-actions-meta-right">
                            <span className={`mini-badge ${localActionsVer?.type === 0 ? "badge-release" : (localActionsVer?.type === 1 ? "badge-beta" : "badge-preview")}`}>
                                {localActionsVer?.type === 0 ? t("common.release") : (localActionsVer?.type === 1 ? t("common.beta") : t("common.preview"))}
                            </span>
                            {localActionsVer?.isGDK && <span className="meta-tag tag-gdk">{t("common.gdk")}</span>}
                        </div>
                    </div>

                    <div className="dp-local-actions-list" role="list">
                        <button
                            type="button"
                            className={`dp-action-item ${(!localActionsPath || localActionsBusy) ? "is-disabled" : ""}`}
                            onClick={(!localActionsPath || localActionsBusy) ? undefined : handleLocalInstall}
                        >
                            <div className="dp-action-icon is-primary"><Box size={18} /></div>
                            <div className="dp-action-main">
                                <div className="dp-action-title">{t("DownloadPage.local_install")}</div>
                                <div className="dp-action-sub">{t("DownloadPage.local_install_desc")}</div>
                            </div>
                            <div className="dp-action-end"><ChevronRight size={18} /></div>
                        </button>

                        <button
                            type="button"
                            className={`dp-action-item ${(localActionsBusy) ? "is-disabled" : ""}`}
                            onClick={localActionsBusy ? undefined : handleRedownload}
                        >
                            <div className="dp-action-icon"><RotateCcw size={18} /></div>
                            <div className="dp-action-main">
                                <div className="dp-action-title">{t("DownloadPage.redownload")}</div>
                                <div className="dp-action-sub">{t("DownloadPage.redownload_desc")}</div>
                            </div>
                            <div className="dp-action-end"><ChevronRight size={18} /></div>
                        </button>

                        <button
                            type="button"
                            className={`dp-action-item is-danger ${(localActionsBusy) ? "is-disabled" : ""}`}
                            onClick={localActionsBusy ? undefined : handleDeleteLocal}
                        >
                            <div className="dp-action-icon is-danger"><Trash2 size={18} /></div>
                            <div className="dp-action-main">
                                <div className="dp-action-title">{t("DownloadPage.delete_local_package")}</div>
                                <div className="dp-action-sub">{t("DownloadPage.delete_local_package_desc")}</div>
                            </div>
                            <div className="dp-action-end"><ChevronRight size={18} /></div>
                        </button>
                    </div>
                </div>
            </Modal>

            <Modal
                open={deleteConfirmOpen}
                title={t("DownloadPage.delete_local_confirm_title")}
                onClose={() => (localActionsBusy ? undefined : setDeleteConfirmOpen(false))}
                width="520px"
                footer={(
                    <div style={{ display: "flex", justifyContent: "flex-end", gap: 12 }}>
                        <Button variant="ghost" disabled={localActionsBusy} onClick={() => setDeleteConfirmOpen(false)}>{t("common.cancel")}</Button>
                        <Button variant="danger" loading={localActionsBusy} onClick={handleConfirmDeleteLocal}>{t("DownloadPage.delete_local_confirm")}</Button>
                    </div>
                )}
            >
                <div className="dp-local-actions">
                    <div className="dp-local-actions-desc">{t("DownloadPage.delete_local_confirm_desc")}</div>
                    <div className="dp-local-actions-meta">
                        <div className="dp-local-actions-meta-left">
                            <div className="dp-local-actions-name">{localActionsVer?.version || "-"}</div>
                            {localActionsPath ? <div className="dp-local-actions-path" title={localActionsPath}>{localActionsPath}</div> : null}
                        </div>
                        <div className="dp-local-actions-meta-right">
                            {localActionsVer?.isGDK && <span className="meta-tag tag-gdk">{t("common.gdk")}</span>}
                        </div>
                    </div>
                </div>
            </Modal>
        </>
    );
};

export default DownloadPage;
