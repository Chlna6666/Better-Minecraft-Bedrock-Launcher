import React, { useState, useMemo, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';
import useVersions from '../../hooks/useVersions';
import { AssetManager } from './components/AssetManager';
import { ConfirmModal } from './components/ConfirmModal';
import { VersionSettingsModal } from './components/VersionSettingsModal';
import "../Download/InstallProgressBar.css";
import InstallProgressBar from "../Download/InstallProgressBar";
import {
    Layers, Map, Package, Search, Box, Loader2,
    FolderOpen, Play, RefreshCw, Trash2, Plus, Settings, ShieldCheck, DownloadCloud
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { useToast } from '../../components/Toast';
import './ManagePage.css';

import { useLauncher } from '../../hooks/useLauncher';
import { LaunchStatusModal } from '../../components/LaunchStatusModal';

type TabType = 'mods' | 'resource' | 'maps';

const ManagePage = () => {
    // 1. 获取版本列表数据
    const { versions, reload } = useVersions();
    const isDataLoading = versions === undefined || versions === null;
    const toast = useToast();
    const { t } = useTranslation();
    const navigate = useNavigate();

    // 2. 状态管理
    const [selectedFolder, setSelectedFolder] = useState<string | null>(null);
    const [activeTab, setActiveTab] = useState<TabType>('mods');
    const [searchQuery, setSearchQuery] = useState('');

    // [新增] 隔离状态
    const [isIsolated, setIsIsolated] = useState(false);

    // 弹窗状态
    const [isDeleteModalOpen, setDeleteModalOpen] = useState(false);
    const [isDeleting, setIsDeleting] = useState(false);
    const [isListRefreshing, setIsListRefreshing] = useState(false);

    const [isSettingsOpen, setSettingsOpen] = useState(false);

    // 导入相关状态
    const [isImporting, setIsImporting] = useState(false);
    const [importPath, setImportPath] = useState<string | null>(null);

    // 使用启动器 Hook
    const { isLaunching, launchLogs, launchError, launch, close } = useLauncher();

    // 3. 默认选中逻辑
    useEffect(() => {
        if (!isDataLoading && versions && versions.length > 0 && !selectedFolder) {
            setSelectedFolder(versions[0].folder);
        }
    }, [versions, selectedFolder, isDataLoading]);

    // [新增] 当选中的版本变化时，检查其配置是否开启了隔离
    useEffect(() => {
        if (selectedFolder) {
            invoke('get_version_config', { folderName: selectedFolder })
                .then((res: any) => {
                    setIsIsolated(!!res?.enable_redirection);
                })
                .catch(() => {
                    setIsIsolated(false);
                });
        } else {
            setIsIsolated(false);
        }
    }, [selectedFolder, versions]); // versions 变化也需要重新检查（例如刚保存设置）

    const showLoading = isDataLoading || (!selectedFolder && versions?.length > 0);

    // 过滤左侧列表
    const filteredVersions = useMemo(() => {
        if (isDataLoading || !versions) return [];
        if (!searchQuery) return versions;
        const q = searchQuery.toLowerCase();
        return versions.filter((v: any) =>
            (v.name && v.name.toLowerCase().includes(q)) ||
            (v.folder && v.folder.toLowerCase().includes(q)) ||
            (v.version && v.version.toLowerCase().includes(q))
        );
    }, [versions, searchQuery, isDataLoading]);

    const selectedVersion = useMemo(() =>
            (versions || []).find((v: any) => v.folder === selectedFolder),
        [versions, selectedFolder]);

    // --- Actions ---

    const handleListRefresh = async () => {
        setIsListRefreshing(true);
        try {
            await reload();
            toast.success(t("ManagePage.list_refreshed"));
        } finally {
            setTimeout(() => setIsListRefreshing(false), 500);
        }
    };

    const handleGoDownload = () => {
        navigate('/download');
    };

    const handleImportVersion = async () => {
        if (isImporting) return;
        try {
            const selected = await open({ filters: [{ name: 'Packages', extensions: ['appx', 'zip', 'msixvc'] }], multiple: false });
            if (selected) { setImportPath(selected as string); setIsImporting(true); }
        } catch (e: any) { toast.error(t("ManagePage.select_file_failed")); }
    };
    const handleImportComplete = () => { setIsImporting(false); setImportPath(null); toast.success(t("ManagePage.import_done")); reload(); };
    const handleImportCancel = () => { setIsImporting(false); setImportPath(null); };

    const handleOpenFolder = async () => {
        if (!selectedVersion?.path) return;
        try { await invoke('open_path', { path: selectedVersion.path }); }
        catch (e: any) { toast.error(t("ManagePage.open_failed", { error: e.message })); }
    };

    const handleSettingsClick = () => {
        if (selectedVersion) setSettingsOpen(true);
    };

    const handleSettingsSaved = () => {
        reload(); // 重新加载版本列表以刷新状态
    };

    const handleLaunchMap = (folderName: string, mapName: string) => {
        if (!selectedVersion) return;
        launch(selectedVersion.folder, `minecraft://?load=${folderName}`, () => {
            setTimeout(close, 1500);
        });
    };

    const handleLaunchNormal = () => {
        if (!selectedVersion) return;
        launch(selectedVersion.folder, null, () => {
            setTimeout(close, 1500);
        });
    };

    const handleDeleteClick = () => { if (selectedVersion) setDeleteModalOpen(true); };

    const handleConfirmDelete = async () => {
        if (!selectedVersion) return;
        setIsDeleting(true);
        try {
            await invoke('delete_version', { folderName: selectedVersion?.folder });
            toast.success(t("ManagePage.version_deleted"));
            setDeleteModalOpen(false);
            await reload();
            setSelectedFolder(null);
        } catch (e: any) { toast.error(t("ManagePage.delete_failed", { error: e.message })); }
        finally { setIsDeleting(false); }
    };

    const renderListContent = () => {
        if (showLoading && !versions) return <SkeletonSidebar />;
        if (filteredVersions.length === 0) {
            return (
                <div className="list-empty anim-content-entry">
                    {(!versions || versions.length === 0) ? (
                        <>
                            <p>{t("ManagePage.no_versions")}</p>
                            <div className="list-empty-actions">
                                <button className="list-empty-btn primary" type="button" onClick={handleGoDownload}>
                                    <DownloadCloud size={14} />
                                    {t("common.go_download")}
                                </button>
                                <button className="list-empty-btn" type="button" onClick={handleListRefresh} disabled={isListRefreshing}>
                                    <RefreshCw size={14} className={isListRefreshing ? 'spin' : ''} />
                                    {t("common.refresh")}
                                </button>
                            </div>
                        </>
                    ) : ( <p>{t("common.no_result")}</p> )}
                </div>
            );
        }
        return filteredVersions.map((v: any, index: number) => {
            const shouldAnimate = index < 15;
            const delayStyle = shouldAnimate ? { animationDelay: `${index * 0.03}s` } : {};
            const animateClass = shouldAnimate ? 'animate-in' : '';
            return (
                <div key={v.folder} onClick={() => setSelectedFolder(v.folder)} style={delayStyle} className={`version-item ${animateClass} ${selectedFolder === v.folder ? 'active' : ''}`}>
                    <div className="v-icon">{v.icon ? <img src={v.icon} alt="" width="32" height="32" /> : <Box size={24} style={{ opacity: 0.5 }} />}</div>
                    <div className="v-info">
                        <div className="v-name" title={v.folder || v.name}>{v.folder || v.name}</div>
                        <div className="v-meta">
                            <span className="v-ver">{v.version}</span>
                            {v.kindLabel && <span className={`v-tag ${String(v.kind || '').toLowerCase()}`}>{v.kindLabel}</span>}
                        </div>
                    </div>
                </div>
            );
        });
    };

    const renderRightContent = () => {
        if (showLoading) {
            return (
                <div className="manage-empty-state anim-content-entry" key="loading">
                    <Loader2 size={32} opacity={0.5} className="loader-spin" />
                    <p style={{ marginTop: 16, fontSize: 13, opacity: 0.7 }}>{t("ManagePage.loading_versions")}</p>
                </div>
            );
        }

        if (selectedVersion) {
            return (
                <div className="manage-main-panel anim-content-entry" key={selectedVersion.folder}>
                    <div className="manage-compact-header">
                        <div className="header-top-row">
                            <div className="header-info">
                                <h2 title={selectedVersion.folder || selectedVersion.name}>{selectedVersion.folder || selectedVersion.name}</h2>
                                <div className="header-badges">
                                    <span className="v-badge primary">{selectedVersion.version}</span>
                                    {selectedVersion.versionTypeLabel && <span className="v-badge secondary">{selectedVersion.versionTypeLabel}</span>}
                                    {/* [新增] 隔离模式状态显示 */}
                                    {isIsolated && (
                                        <span className="v-badge isolation">
                                            <ShieldCheck size={10} style={{marginRight:3, marginBottom:-1}}/>{t("ManagePage.isolation")}
                                        </span>
                                    )}
                                </div>
                            </div>
                            <div className="header-actions">
                                <div className="icon-group">
                                    <button className="icon-btn-ghost" title={t("ManagePage.open_version_folder")} onClick={handleOpenFolder}><FolderOpen size={18} /></button>
                                    <button className="icon-btn-ghost" title={t("ManagePage.version_settings")} onClick={handleSettingsClick}><Settings size={18} /></button>
                                    <button className="icon-btn-ghost danger" title={t("ManagePage.delete_version")} onClick={handleDeleteClick}><Trash2 size={18} /></button>
                                </div>
                                <div className="divider-vertical" />
                                <button className="launch-btn" onClick={handleLaunchNormal}>
                                    <Play size={16} fill="currentColor" style={{marginRight: 6}} /> {t("ManagePage.launch_instance")}
                                </button>
                            </div>
                        </div>
                        <div className="header-tabs-row">
                            <TabButton active={activeTab === 'mods'} onClick={() => setActiveTab('mods')} icon={<Layers size={15}/>} label={t("ManagePage.tabs.mods")} />
                            <TabButton active={activeTab === 'resource'} onClick={() => setActiveTab('resource')} icon={<Package size={15}/>} label={t("ManagePage.tabs.resource")} />
                            <TabButton active={activeTab === 'maps'} onClick={() => setActiveTab('maps')} icon={<Map size={15}/>} label={t("ManagePage.tabs.maps")} />
                        </div>
                    </div>

                    <div className="manage-body-content" style={{ display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
                        {activeTab === 'mods' && <AssetManager key="mods" version={selectedVersion} type="dll_mod" fileExtensions={['dll']} />}
                        {activeTab === 'resource' && <AssetManager key="resource" version={selectedVersion} type="resource_pack" fileExtensions={['mcpack', 'zip']} />}
                        {activeTab === 'maps' && (
                            <AssetManager
                                key="maps"
                                version={selectedVersion}
                                type="map"
                                fileExtensions={['mcworld', 'zip']}
                                onLaunchMap={handleLaunchMap}
                            />
                        )}
                    </div>
                </div>
            );
        }

        return <div className="manage-empty-state anim-content-entry" key="empty"><Box size={48} opacity={0.3} /><p>{t("ManagePage.select_version_tip")}</p></div>;
    };

    return (
        <div className="manage-page-container">
            <div className="manage-layout">
                <div className="manage-sidebar anim-sidebar-bg">
                    <div className="sidebar-header">
                        <div className="sidebar-tools">
                            <div className="search-box"><Search size={14} className="search-icon" /><input type="text" placeholder={t("ManagePage.search_placeholder")} value={searchQuery} onChange={(e) => setSearchQuery(e.target.value)} /></div>
                            <div className="sidebar-actions">
                                <button className="sidebar-icon-btn" onClick={handleImportVersion} disabled={isImporting}><Plus size={16} /></button>
                                <button className="sidebar-icon-btn" onClick={handleListRefresh} disabled={isListRefreshing}><RefreshCw size={14} className={isListRefreshing ? 'spin' : ''} /></button>
                            </div>
                        </div>
                    </div>
                    <div className="version-list custom-scrollbar">{renderListContent()}</div>
                </div>
                <div className="manage-content">{renderRightContent()}</div>
            </div>

            {isImporting && <InstallProgressBar version={t("ManagePage.local_import")} packageId={null} versionType={-1} isImport={true} sourcePath={importPath} onStatusChange={() => {}} onCompleted={handleImportComplete} onCancel={handleImportCancel}><></></InstallProgressBar>}

            <ConfirmModal
                isOpen={isDeleteModalOpen} title={t("ManagePage.delete_version_title")} isDanger={true} isLoading={isDeleting}
                onConfirm={handleConfirmDelete} onCancel={() => setDeleteModalOpen(false)}
                content={<div><p>{t("ManagePage.delete_version_confirm", { folder: selectedVersion?.folder })}</p><p style={{marginTop: 8, color: '#ef4444', fontSize: '13px'}}>{t("ManagePage.delete_version_warning")}</p></div>}
            />

            <VersionSettingsModal
                isOpen={isSettingsOpen}
                onClose={() => setSettingsOpen(false)}
                version={selectedVersion}
                onSaved={handleSettingsSaved}
            />

            <LaunchStatusModal
                isOpen={isLaunching}
                logs={launchLogs}
                error={launchError}
                onClose={close}
                onRetry={handleLaunchNormal}
            />
        </div>
    );
};

const SkeletonSidebar = () => (<div className="skeleton-container">{[1, 2, 3, 4, 5].map((i) => (<div key={i} className="skeleton-item"><div className="skeleton-bg sk-icon" /><div className="sk-text"><div className="skeleton-bg sk-line-1" /><div className="skeleton-bg sk-line-2" /></div></div>))}</div>);
const TabButton = ({ active, onClick, icon, label }: { active: boolean, onClick: () => void, icon: React.ReactNode, label: string }) => (<button className={`compact-tab-btn ${active ? 'active' : ''}`} onClick={onClick}>{icon}<span>{label}</span>{active && <div className="tab-indicator" />}</button>);

export default ManagePage;
