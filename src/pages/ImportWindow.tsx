import { useEffect, useState, useMemo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import { importAssets, checkImportConflict } from './Manage/api/assetApi';
import { FileUp, Loader2, CheckCircle, AlertCircle, Package, AlertTriangle, ArrowRight, Layers } from 'lucide-react';
import { AnimatePresence, motion } from 'framer-motion';
import MinecraftFormattedText from '../utils/MinecraftFormattedText'; // [引入]
import useVersions from '../hooks/useVersions';
import { useTranslation } from 'react-i18next';

import './ImportWindow.css';
import { Select } from "../components";


// [新增] 预览数据接口
interface PreviewInfo {
    name: string;
    description: string;
    icon: string | null;
    kind: string;
    version: string | null;
    size: number;
    manifest?: any; // [新增]
    sub_packs?: PreviewInfo[]; // [新增] 子包信息
    valid?: boolean; // [新增] 规范校验
    invalid_reason?: string | null; // [新增]
}

interface ConflictInfo {
    has_conflict: boolean;
    conflict_type: string | null;
    target_name: string;
    message: string;
    existing_pack_info?: PreviewInfo; // [新增]
}

const ImportWindow = () => {
    const [filePath, setFilePath] = useState<string | null>(null);
    const [preview, setPreview] = useState<PreviewInfo | null>(null); // [新增] 预览状态
    const { versions } = useVersions();
    const [selectedFolder, setSelectedFolder] = useState<string>("");
    const [isImporting, setIsImporting] = useState(false);
    const [isInspecting, setIsInspecting] = useState(false); // [新增] 检查中状态
    const [status, setStatus] = useState<'idle' | 'success' | 'error'>('idle');
    const [message, setMessage] = useState("");
    const [conflict, setConflict] = useState<ConflictInfo | null>(null); // [新增] 冲突信息
    const [showConflictDialog, setShowConflictDialog] = useState(false); // [新增] 冲突对话框
    const initOnceRef = useRef(false);
    const lastInspectedRef = useRef<string | null>(null);
    const { t, i18n } = useTranslation();

    const versionOptions = useMemo(() => {
        return versions.map(v => ({
            label: `${v.folder} (${v.version}) ${v.versionTypeLabel || ""} - ${v.kindLabel || v.kind || 'UWP'}${v.config?.enable_redirection ? ` ${t('Import.isolation')}` : ""}`,
            value: v.folder
        }));
    }, [versions, t]);

    // 获取启动参数
    useEffect(() => {
        if (initOnceRef.current) return;
        initOnceRef.current = true;
        invoke<string | null>('get_startup_import_file').then((path) => {
            if (path) setFilePath(path);
        });

        const unlistenPromise = listen<string>('import-file-requested', (event) => {
            setFilePath(event.payload);
            setStatus('idle');
            setPreview(null);
            setConflict(null);
            setShowConflictDialog(false);
        });

        return () => {
            unlistenPromise.then(unlisten => unlisten());
        };
    }, []);

    useEffect(() => {
        if (versions.length > 0) {
            if (!selectedFolder || !versions.find(v => v.folder === selectedFolder)) {
                setSelectedFolder(versions[0].folder);
            }
        }
    }, [versions, selectedFolder]);

    // [新增] 当 filePath 变化时，调用 inspect
    useEffect(() => {
        if (filePath) {
            inspectFile(filePath);
        }
    }, [filePath]);

    const inspectFile = async (path: string) => {
        if (lastInspectedRef.current === path) {
            return;
        }
        lastInspectedRef.current = path;
        setIsInspecting(true);
        setPreview(null);
        try {
            const lang = (i18n.language || "en-US").replace('_', '-');
            const info = await invoke<PreviewInfo>('inspect_import_file', { filePath: path, lang });
            setPreview(info);
        } catch (e) {
            console.error("Inspect failed:", e);
            // 即使失败也不影响主流程，只是不显示预览
        } finally {
            setIsInspecting(false);
        }
    };

    const displaySubPacks = useMemo(() => {
        if (!preview?.sub_packs) return [];
        const subs = preview.sub_packs;
        if (subs.some(s => s.kind === 'Import.worldTemplates')) {
            return subs.filter(s => s.kind === 'Import.worldTemplates' || s.kind === 'Import.minecraftWorlds');
        }
        return subs;
    }, [preview]);


    const handleImportClick = async () => {
        if (!filePath || !selectedFolder) return;
        const targetVersion = versions.find(v => v.folder === selectedFolder);
        if (!targetVersion) return;
        if (preview && preview.valid === false) {
            setStatus('error');
            setMessage(t('Import.errors.invalidPack', { reason: preview.invalid_reason || t('Import.errors.missingUuid') }));
            return;
        }

        setIsImporting(true);
        setStatus('idle');
        setMessage("");

        try {
            let enableIsolation = false;
            try {
            const config: any = await invoke('get_version_config', { folderName: targetVersion.folder });
            if (config && config.enable_redirection) enableIsolation = true;
        } catch (e) { console.warn(e); }

            // 检查冲突
            const conflictInfo: ConflictInfo = await checkImportConflict({
                kind: targetVersion.kind || 'uwp',
                folder: targetVersion.folder,
                filePath: filePath,
                enableIsolation: enableIsolation,
                edition: targetVersion.versionType,
                allowSharedFallback: false
            });

            if (conflictInfo.has_conflict) {
                setConflict(conflictInfo);
                setShowConflictDialog(true);
                setIsImporting(false);
                return;
            }

            // 无冲突直接导入
            await executeImport(false);

        } catch (e: any) {
            setStatus('error');
            setMessage(typeof e === 'string' ? e : e.message || t('Import.errors.checkConflictFailed'));
            setIsImporting(false);
        }
    };

    const executeImport = async (overwrite: boolean, allowSharedFallback: boolean = false) => {
        if (!filePath || !selectedFolder) return;
        const targetVersion = versions.find(v => v.folder === selectedFolder);
        if (!targetVersion) return;

        setIsImporting(true);
        setShowConflictDialog(false);

        try {
            let enableIsolation = false;
            try {
                const config: any = await invoke('get_version_config', { folderName: targetVersion.folder });
                if (config && config.enable_redirection) enableIsolation = true;
            } catch (e) { console.warn(e); }

            await importAssets({
                kind: targetVersion.kind || 'uwp',
                folder: targetVersion.folder,
                filePaths: [filePath],
                enableIsolation: enableIsolation,
                edition: targetVersion.versionType,
                overwrite: overwrite,
                allowSharedFallback: allowSharedFallback
            });

            setStatus('success');
            setMessage(t('Import.importSuccess'));
            setTimeout(() => getCurrentWindow().close(), 1500);
        } catch (e: any) {
            setStatus('error');
            setMessage(typeof e === 'string' ? e : e.message || t('Import.errors.importFailed'));
        } finally {
            setIsImporting(false);
        }
    };

    if (!filePath && status === 'idle') {
        return (
            <div className="loading-container">
                <Loader2 className="spin" size={32} />
                <p>{t('Import.waitingFile')}</p>
            </div>
        );
    }

    // 格式化文件大小
    const formatSize = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    return (
        <div className="import-wrapper">
            <div data-tauri-drag-region className="drag-region" />

            <div className="import-content">
                <motion.div
                    initial={{ opacity: 0, y: 20 }}
                    animate={{ opacity: 1, y: 0 }}
                    className="import-card"
                >
                    <div className="header-section">
                        <h1 className="title">{t('Import.title')}</h1>
                    </div>

                    {/* [新增] 预览卡片 */}
                    <div className="preview-card glass-panel">
                        {isInspecting ? (
                            <div className="preview-loading"><Loader2 className="spin" size={20} /> {t('Import.inspecting')}</div>
                        ) : preview ? (
                            <div className="preview-content-wrapper">
                                <div className="preview-content">
                                    <div className="preview-icon">
                                        {preview.icon ? (
                                            <img src={preview.icon} alt="icon" />
                                        ) : (
                                            <div className="preview-icon-placeholder"><Package size={24}/></div>
                                        )}
                                    </div>
                                    <div className="preview-info">
                                        <div className="preview-name">
                                            <MinecraftFormattedText text={preview.name} />
                                        </div>
                                        <div className="preview-desc">
                                            <MinecraftFormattedText text={preview.description || t('Import.noDescription')} />
                                        </div>
                                        <div className="preview-meta">
                                            <span className="meta-tag kind">{t(preview.kind)}</span>
                                            {preview.version && <span className="meta-tag ver">v{preview.version}</span>}
                                            <span className="meta-tag size">{formatSize(preview.size)}</span>
                                        </div>
                                        {preview.valid === false && (
                                            <div className="preview-warning">
                                                {t('Import.errors.invalidPack', { reason: preview.invalid_reason || t('Import.errors.missingUuid') })}
                                            </div>
                                        )}
                                    </div>
                                </div>
                                
                                {/* 子包列表 */}
                                {displaySubPacks.length > 0 && (
                                    <div className="sub-packs-list">
                                        <div className="sub-packs-header">
                                            <Layers size={14} />
                                            <span>{t('Import.subPacksCount', { count: displaySubPacks.length })}</span>
                                        </div>
                                        <div className="sub-packs-items">
                                            {displaySubPacks.map((sub, idx) => (
                                                <div key={idx} className="sub-pack-item">
                                                    <div className="sub-pack-icon">
                                                        {sub.icon ? <img src={sub.icon} alt="" /> : <Package size={16} />}
                                                    </div>
                                                    <div className="sub-pack-info">
                                                        <div className="sub-pack-name"><MinecraftFormattedText text={sub.name} /></div>
                                                        <div className="sub-pack-meta">
                                                        <span className="sub-pack-kind">{t(sub.kind)}</span>
                                                            {sub.version && <span>v{sub.version}</span>}
                                                            {sub.valid === false && (
                                                                <span className="sub-pack-invalid">{t('Import.invalidPackShort')}</span>
                                                            )}
                                                        </div>
                                                    </div>
                                                </div>
                                            ))}
                                        </div>
                                    </div>
                                )}
                            </div>
                        ) : (
                            // 如果解析失败或没有预览，显示简略路径
                            <div className="preview-fallback">
                                <FileUp size={24} style={{marginBottom:8, opacity:0.5}}/>
                                <p className="file-path">{filePath}</p>
                            </div>
                        )}
                    </div>

                    <div className="glass-panel form-panel">
                        {versions.length === 0 ? (
                            <div className="empty-tip">{t('Import.noVersions')}</div>
                        ) : (
                            <div className="form-group">
                                <label className="label">{t('Import.targetVersion')}</label>
                                <Select
                                    value={selectedFolder}
                                    onChange={(val) => setSelectedFolder(val as string)}
                                    options={versionOptions}
                                    disabled={isImporting || status === 'success'}
                                    placeholder={t('Import.selectVersion')}
                                    style={{ width: '100%' }}
                                />
                            </div>
                        )}
                    </div>

                    <AnimatePresence mode='wait'>
                        {status === 'error' && (
                            <motion.div
                                initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: 'auto' }} exit={{ opacity: 0, height: 0 }}
                                className="status-box error"
                            >
                                <AlertCircle size={18} />
                                <span>{message}</span>
                            </motion.div>
                        )}
                        {status === 'success' && (
                            <motion.div
                                initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: 'auto' }} exit={{ opacity: 0, height: 0 }}
                                className="status-box success"
                            >
                                <CheckCircle size={18} />
                                <span>{message}</span>
                            </motion.div>
                        )}
                    </AnimatePresence>

                    <button
                        onClick={handleImportClick}
                        disabled={isImporting || versions.length === 0 || status === 'success' || preview?.valid === false}
                        className="action-btn"
                    >
                        {isImporting ? (<><Loader2 className="spin" size={18} /> {t('Import.processing')}</>)
                            : status === 'success' ? (<><CheckCircle size={18} /> {t('Import.done')}</>)
                                : (t('Import.startImport'))}
                    </button>
                </motion.div>
            </div>

            {/* 冲突确认对话框 */}
            <AnimatePresence>
                {showConflictDialog && conflict && (
                    <motion.div
                        className="conflict-overlay"
                        initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
                    >
                        <motion.div
                            className="conflict-dialog"
                            initial={{ scale: 0.9, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: 0.9, opacity: 0 }}
                        >
                            <div className="conflict-header">
                                <AlertTriangle size={24} className="conflict-icon" />
                                <h3>{t('Import.conflict.title')}</h3>
                            </div>
                            <div className="conflict-body">
                                {conflict.conflict_type === 'shared_fallback' ? (
                                    <>
                                        <p className="conflict-msg-text">{conflict.message || t('Import.conflict.sharedFallbackMessage')}</p>
                                        <p className="conflict-warning">{t('Import.conflict.sharedFallbackWarning')}</p>
                                    </>
                                ) : (
                                    <>
                                        <p className="conflict-msg-text">{t('Import.conflict.uuidMessage')}</p>
                                        
                                        <div className="conflict-compare-container">
                                            {/* 旧版本 (Existing) */}
                                            <div className="conflict-item old">
                                                <div className="conflict-label">{t('Import.conflict.current')}</div>
                                                <div className="conflict-icon-wrapper">
                                                    {conflict.existing_pack_info?.icon ? (
                                                        <img src={conflict.existing_pack_info.icon} alt="old" />
                                                    ) : (
                                                        <Package size={24} opacity={0.5} />
                                                    )}
                                                </div>
                                                <div className="conflict-details">
                                                    <div className="conflict-name">
                                                        <MinecraftFormattedText text={conflict.existing_pack_info?.name || conflict.target_name} />
                                                    </div>
                                                    <div className="conflict-ver">
                                                        v{conflict.existing_pack_info?.version || t('Import.unknown')}
                                                    </div>
                                                    <div className="conflict-desc">
                                                        <MinecraftFormattedText text={conflict.existing_pack_info?.description || t('Import.noDescription')} />
                                                    </div>
                                                </div>
                                            </div>

                                            <div className="conflict-arrow">
                                                <ArrowRight size={20} />
                                            </div>

                                            {/* 新版本 (New) */}
                                            <div className="conflict-item new">
                                                <div className="conflict-label">{t('Import.conflict.new')}</div>
                                                <div className="conflict-icon-wrapper">
                                                    {preview?.icon ? (
                                                        <img src={preview.icon} alt="new" />
                                                    ) : (
                                                        <Package size={24} opacity={0.5} />
                                                    )}
                                                </div>
                                                <div className="conflict-details">
                                                    <div className="conflict-name">
                                                        <MinecraftFormattedText text={preview?.name || t('Import.unknown')} />
                                                    </div>
                                                    <div className="conflict-ver">
                                                        v{preview?.version || t('Import.unknown')}
                                                    </div>
                                                    <div className="conflict-desc">
                                                        <MinecraftFormattedText text={preview?.description || t('Import.noDescription')} />
                                                    </div>
                                                </div>
                                            </div>
                                        </div>
                                        
                                        <p className="conflict-warning">{t('Import.conflict.overwriteWarning')}</p>
                                    </>
                                )}
                            </div>
                            <div className="conflict-actions">
                                <button className="btn-cancel" onClick={() => setShowConflictDialog(false)}>{t('common.cancel')}</button>
                                {conflict.conflict_type === 'shared_fallback' ? (
                                    <button className="btn-confirm" onClick={() => executeImport(false, true)}>{t('Import.conflict.importToShared')}</button>
                                ) : (
                                    <button className="btn-confirm" onClick={() => executeImport(true)}>{t('Import.conflict.overwriteImport')}</button>
                                )}
                            </div>
                        </motion.div>
                    </motion.div>
                )}
            </AnimatePresence>
        </div>
    );
};

export default ImportWindow;
