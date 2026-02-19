import React, { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { AnimatePresence, motion } from 'framer-motion';
import { AlertCircle, AlertTriangle, ArrowRight, CheckCircle, Layers, Loader2, Package, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import MinecraftFormattedText from '../../../utils/MinecraftFormattedText';
import { checkImportConflict, importAssets } from '../api/assetApi';

import './AssetImportModal.css';

interface PreviewInfo {
    name: string;
    description: string;
    icon: string | null;
    kind: string;
    version: string | null;
    size: number;
    manifest?: any;
    sub_packs?: PreviewInfo[];
    valid?: boolean;
    invalid_reason?: string | null;
}

interface ConflictInfo {
    has_conflict: boolean;
    conflict_type: string | null;
    target_name: string;
    message: string;
    existing_pack_info?: PreviewInfo;
}

type Status = 'idle' | 'success' | 'error';

export type AssetImportModalType = 'resource_pack' | 'map';

interface AssetImportModalProps {
    isOpen: boolean;
    type: AssetImportModalType;
    version: any;
    enableIsolation: boolean;
    edition: string;
    userId: string | null;
    filePaths: string[];
    onClose: () => void;
    onImported: () => void;
}

export const AssetImportModal: React.FC<AssetImportModalProps> = ({
    isOpen,
    type,
    version,
    enableIsolation,
    edition,
    userId,
    filePaths,
    onClose,
    onImported
}) => {
    const { t, i18n } = useTranslation();
    const [activePath, setActivePath] = useState<string | null>(null);
    const [previewByPath, setPreviewByPath] = useState<Record<string, PreviewInfo | null>>({});
    const [inspectingPaths, setInspectingPaths] = useState<Record<string, boolean>>({});
    const [inspectErrors, setInspectErrors] = useState<Record<string, boolean>>({});
    const [isImporting, setIsImporting] = useState(false);
    const [status, setStatus] = useState<Status>('idle');
    const [message, setMessage] = useState('');

    const [conflict, setConflict] = useState<ConflictInfo | null>(null);
    const [showConflictDialog, setShowConflictDialog] = useState(false);
    const conflictIndexRef = useRef<number | null>(null);

    // Prevent duplicate inspection requests for the same path.
    const inspectInFlightRef = useRef<Record<string, Promise<void>>>({});
    // Avoid stale state closures (especially during the open/reset effect).
    const previewCacheRef = useRef<Record<string, PreviewInfo | null | undefined>>({});
    const inspectErrorRef = useRef<Record<string, boolean>>({});

    const buildType = useMemo(() => String(version?.kind || 'uwp').toLowerCase(), [version]);
    const versionFolder = useMemo(() => String(version?.folder || ''), [version]);

    const canClose = !isImporting;

    const getInspectLimit = () => {
        const hc = (globalThis as any)?.navigator?.hardwareConcurrency;
        const n = typeof hc === 'number' && hc > 0 ? hc : 4;
        // Spawn-blocking on the backend already uses a threadpool; avoid oversubscription.
        return Math.max(2, Math.min(6, Math.floor(n / 2)));
    };

    const runWithConcurrency = async <T,>(
        items: T[],
        limit: number,
        worker: (item: T, index: number) => Promise<void>
    ) => {
        if (items.length === 0) return;
        const concurrency = Math.max(1, Math.min(limit, items.length));
        let nextIndex = 0;

        const runners = new Array(concurrency).fill(0).map(async () => {
            while (true) {
                const idx = nextIndex;
                nextIndex += 1;
                if (idx >= items.length) return;
                await worker(items[idx], idx);
            }
        });

        await Promise.all(runners);
    };

    useEffect(() => {
        if (!isOpen) return;
        // Reset when opened to avoid leaking previous state across tabs.
        setActivePath(filePaths[0] || null);
        previewCacheRef.current = {};
        inspectErrorRef.current = {};
        setPreviewByPath({});
        setInspectingPaths({});
        setInspectErrors({});
        setIsImporting(false);
        setStatus('idle');
        setMessage('');
        setConflict(null);
        setShowConflictDialog(false);
        conflictIndexRef.current = null;
        inspectInFlightRef.current = {};
        // Trigger background inspection for all files as soon as the modal shows.
        // This ensures the list populates quickly even when the first file is slow.
        void inspectAll(filePaths);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [isOpen, filePaths.join('|')]);

    const preview = activePath ? (previewByPath[activePath] ?? null) : null;
    const isInspectingActive = !!(activePath && inspectingPaths[activePath]);

    const displaySubPacks = useMemo(() => {
        if (!preview?.sub_packs) return [];
        const subs = preview.sub_packs;
        if (subs.some(s => s.kind === 'Import.worldTemplates')) {
            return subs.filter(s => s.kind === 'Import.worldTemplates' || s.kind === 'Import.minecraftWorlds');
        }
        return subs;
    }, [preview]);

    const formatSize = (bytes: number) => {
        if (!bytes) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const inspectFile = async (path: string) => {
        if (!path) return;
        const cached = previewCacheRef.current[path];
        const hadError = !!inspectErrorRef.current[path];
        // If we already have a preview and it wasn't from a failed attempt, skip.
        // If it previously failed (cached=null + hadError=true), allow retry.
        if (cached !== undefined && !(cached === null && hadError)) return;

        if (inspectInFlightRef.current[path]) {
            await inspectInFlightRef.current[path];
            return;
        }

        setInspectingPaths(prev => ({ ...prev, [path]: true }));
        try {
            const task = (async () => {
                const lang = (i18n.language || 'en-US').replace('_', '-');
                const info = await invoke<PreviewInfo>('inspect_import_file', { filePath: path, lang });
                setPreviewByPath(prev => {
                    const next = { ...prev, [path]: info };
                    previewCacheRef.current = next;
                    return next;
                });
                setInspectErrors(prev => {
                    const next = { ...prev };
                    delete next[path];
                    inspectErrorRef.current = next;
                    return next;
                });
            })();
            inspectInFlightRef.current[path] = task;
            await task;
        } catch (_e) {
            setPreviewByPath(prev => {
                const next = { ...prev, [path]: null };
                previewCacheRef.current = next;
                return next;
            });
            setInspectErrors(prev => {
                const next = { ...prev, [path]: true };
                inspectErrorRef.current = next;
                return next;
            });
        } finally {
            delete inspectInFlightRef.current[path];
            setInspectingPaths(prev => {
                const next = { ...prev };
                delete next[path];
                return next;
            });
        }
    };

    const inspectAll = async (paths: string[]) => {
        const unique = Array.from(new Set(paths.filter(Boolean)));
        // Always enqueue; inspectFile() itself de-dupes via refs and in-flight tracking.
        const todo = unique.filter(p => !inspectInFlightRef.current[p]);
        if (todo.length === 0) return;
        const limit = getInspectLimit();
        await runWithConcurrency(todo, limit, async (p) => {
            await inspectFile(p);
        });
    };

    const doImportOne = async (filePath: string, overwrite: boolean, allowSharedFallback: boolean) => {
        await importAssets({
            kind: version?.kind || 'uwp',
            folder: versionFolder,
            filePaths: [filePath],
            enableIsolation: !!enableIsolation,
            edition,
            userId: userId || null,
            overwrite,
            allowSharedFallback
        });
    };

    const checkConflictOne = async (filePath: string): Promise<ConflictInfo> => {
        return await checkImportConflict({
            kind: version?.kind || 'uwp',
            folder: versionFolder,
            filePath,
            enableIsolation: !!enableIsolation,
            edition,
            userId: userId || null,
            allowSharedFallback: false
        });
    };

    const stopImport = (errMessage?: string) => {
        setIsImporting(false);
        conflictIndexRef.current = null;
        if (errMessage) {
            setStatus('error');
            setMessage(errMessage);
        }
    };

    const importSequential = async (startIndex: number) => {
        for (let idx = startIndex; idx < filePaths.length; idx++) {
            const p = filePaths[idx];
            setActivePath(p);
            await inspectFile(p);

            let conflictInfo: ConflictInfo;
            try {
                conflictInfo = await checkConflictOne(p);
            } catch (e: any) {
                stopImport(typeof e === 'string' ? e : e?.message || t('Import.errors.checkConflictFailed'));
                return;
            }

            if (conflictInfo?.has_conflict) {
                setConflict(conflictInfo);
                setShowConflictDialog(true);
                conflictIndexRef.current = idx;
                setIsImporting(false);
                return;
            }

            try {
                await doImportOne(p, false, false);
            } catch (e: any) {
                stopImport(typeof e === 'string' ? e : e?.message || t('Import.errors.importFailed'));
                return;
            }
        }

        setIsImporting(false);
        setStatus('success');
        setMessage(t('Import.importSuccess'));
        onImported();
        setTimeout(() => onClose(), 600);
    };

    const handleStartImport = async () => {
        if (!filePaths.length || isImporting) return;

        if (!versionFolder) {
            setStatus('error');
            setMessage(t('Import.noVersions'));
            return;
        }

        // Match list gating: GDK maps require a user unless isolation is enabled.
        if (buildType === 'gdk' && type === 'map' && !userId && !enableIsolation) {
            setStatus('error');
            setMessage(t('GDKUserSelect.select_user'));
            return;
        }

        setIsImporting(true);
        setStatus('idle');
        setMessage('');
        setConflict(null);
        setShowConflictDialog(false);
        conflictIndexRef.current = null;

        await importSequential(0);
    };

    const resolveConflictAndContinue = async (overwrite: boolean, allowSharedFallback: boolean) => {
        const idx = conflictIndexRef.current;
        const p = (typeof idx === 'number' && idx >= 0) ? filePaths[idx] : null;
        if (!p) {
            setShowConflictDialog(false);
            setConflict(null);
            conflictIndexRef.current = null;
            return;
        }

        setIsImporting(true);
        setShowConflictDialog(false);
        setConflict(null);

        try {
            await doImportOne(p, overwrite, allowSharedFallback);
        } catch (e: any) {
            stopImport(typeof e === 'string' ? e : e?.message || t('Import.errors.importFailed'));
            return;
        }

        const nextIndex = idx + 1;
        conflictIndexRef.current = null;
        await importSequential(nextIndex);
    };

    if (!isOpen) return null;

    return createPortal(
        <div className="am-import-overlay" onClick={canClose ? onClose : undefined}>
            <motion.div
                className="am-import-modal"
                onClick={(e) => e.stopPropagation()}
                initial={{ opacity: 0, y: 10, scale: 0.98 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: 10, scale: 0.98 }}
            >
                <div className="am-import-header">
                    <div className="am-import-title-row">
                        <div className="am-import-title">
                            {type === 'map' ? t('ManageImport.title_maps') : t('ManageImport.title_resource')}
                        </div>
                        <button className="am-import-close" onClick={onClose} disabled={!canClose} aria-label="Close">
                            <X size={16} />
                        </button>
                    </div>
                    <div className="am-import-subtitle">
                        <span className="tag">{versionFolder || t('common.unknown')}</span>
                        <span className="tag">{buildType.toUpperCase()}</span>
                        {enableIsolation && <span className="tag">{t('Import.isolation')}</span>}
                    </div>
                </div>

                <div className="am-import-body">
                    <div className="am-import-pick-meta">
                        {filePaths.length ? t('ManageImport.selected_count', { count: filePaths.length }) : t('ManageImport.no_files')}
                    </div>

                    <div className="am-import-main">
                        <div className="am-import-pane files">
                            {filePaths.length > 0 && (
                                <div className="am-import-files">
                                    <div className="am-import-files-inner">
                                        {filePaths.map((p) => (
                                            (() => {
                                                const rowPreview = previewByPath[p] ?? null;
                                                const rowName = rowPreview?.name || (p.split(/[/\\\\]/).pop() || p);
                                                const rowKind = rowPreview?.kind ? t(rowPreview.kind) : null;
                                                const rowVer = rowPreview?.version ? `v${rowPreview.version}` : null;
                                                const rowSize = (rowPreview && typeof rowPreview.size === 'number') ? formatSize(rowPreview.size) : null;

                                                return (
                                                    <button
                                                        key={p}
                                                        className={`am-import-file ${p === activePath ? 'active' : ''}`}
                                                        onClick={() => { setActivePath(p); void inspectFile(p); }}
                                                        disabled={isImporting}
                                                    >
                                                        <div className="row">
                                                            <div className="icon">
                                                                {rowPreview?.icon ? (
                                                                    <img src={rowPreview.icon} alt="" />
                                                                ) : (
                                                                    <Package size={16} opacity={0.75} />
                                                                )}
                                                            </div>
                                                            <div className="info">
                                                                <div className="name">
                                                                    <MinecraftFormattedText text={rowName} />
                                                                    {inspectingPaths[p] && <span className="badge loading"><Loader2 className="spin" size={12} /> {t('Import.inspecting')}</span>}
                                                                    {rowPreview?.valid === false && <span className="badge bad">{t('Import.invalidPackShort')}</span>}
                                                                </div>
                                                                <div className="meta">
                                                                    {rowKind && <span className="chip">{rowKind}</span>}
                                                                    {rowVer && <span className="chip">{rowVer}</span>}
                                                                    {rowSize && <span className="chip">{rowSize}</span>}
                                                                </div>
                                                            </div>
                                                        </div>
                                                        <div className="path">{p}</div>
                                                    </button>
                                                );
                                            })()
                                        ))}
                                    </div>
                                </div>
                            )}
                        </div>

                        <div className="am-import-pane preview">
                            <div className="am-import-preview">
                                <div className="am-import-preview-inner">
                                    {activePath ? (
                                        <div className="am-import-preview-content">
                                            <div className="am-import-preview-main">
                                                <div className="am-import-preview-icon">
                                                    {preview?.icon ? (
                                                        <img src={preview.icon} alt="icon" />
                                                    ) : (
                                                        <div className="placeholder"><Package size={22} /></div>
                                                    )}
                                                </div>
                                                <div className="am-import-preview-info">
                                                    <div className="name">
                                                        <MinecraftFormattedText text={preview?.name || (activePath.split(/[/\\\\]/).pop() || activePath)} />
                                                        {isInspectingActive && <span className="badge loading"><Loader2 className="spin" size={12} /> {t('Import.inspecting')}</span>}
                                                        {preview?.valid === false && <span className="badge bad">{t('Import.invalidPackShort')}</span>}
                                                    </div>
                                                    <div className="path">{activePath}</div>
                                                    {preview && (
                                                        <>
                                                            <div className="meta">
                                                                <span className="meta-tag kind">{t(preview.kind)}</span>
                                                                {preview.version && <span className="meta-tag ver">v{preview.version}</span>}
                                                                <span className="meta-tag size">{formatSize(preview.size)}</span>
                                                            </div>
                                                            <div className="desc"><MinecraftFormattedText text={preview.description || t('Import.noDescription')} /></div>
                                                        </>
                                                    )}
                                                    {!isInspectingActive && !preview && (
                                                        <div className="warn">
                                                            {t('ManageImport.preview_unavailable')}
                                                        </div>
                                                    )}
                                                    {preview?.valid === false && (
                                                        <div className="warn">
                                                            {t('Import.errors.invalidPack', { reason: preview.invalid_reason || t('Import.errors.missingUuid') })}
                                                        </div>
                                                    )}
                                                </div>
                                            </div>

                                            {displaySubPacks.length > 0 && (
                                                <div className="am-import-subpacks">
                                                    <div className="head"><Layers size={14} /> {t('Import.subPacksCount', { count: displaySubPacks.length })}</div>
                                                    <div className="items">
                                                        {displaySubPacks.map((sub, idx) => (
                                                            <div key={idx} className="item">
                                                                <div className="icon">{sub.icon ? <img src={sub.icon} alt="" /> : <Package size={16} />}</div>
                                                                <div className="info">
                                                                    <div className="n"><MinecraftFormattedText text={sub.name} /></div>
                                                                    <div className="m">
                                                                        <span>{t(sub.kind)}</span>
                                                                        {sub.version && <span>v{sub.version}</span>}
                                                                        {sub.valid === false && <span className="bad">{t('Import.invalidPackShort')}</span>}
                                                                    </div>
                                                                </div>
                                                            </div>
                                                        ))}
                                                    </div>
                                                </div>
                                            )}
                                        </div>
                                    ) : (
                                        <div className="am-import-preview-fallback">
                                            <Package size={18} />
                                            <span>{t('ManageImport.no_files')}</span>
                                        </div>
                                    )}
                                </div>
                            </div>
                        </div>
                    </div>

                    <AnimatePresence mode="wait">
                        {status === 'error' && (
                            <motion.div
                                className="am-import-status error"
                                initial={{ opacity: 0, height: 0 }}
                                animate={{ opacity: 1, height: 'auto' }}
                                exit={{ opacity: 0, height: 0 }}
                            >
                                <AlertCircle size={16} />
                                <span>{message}</span>
                            </motion.div>
                        )}
                        {status === 'success' && (
                            <motion.div
                                className="am-import-status success"
                                initial={{ opacity: 0, height: 0 }}
                                animate={{ opacity: 1, height: 'auto' }}
                                exit={{ opacity: 0, height: 0 }}
                            >
                                <CheckCircle size={16} />
                                <span>{message}</span>
                            </motion.div>
                        )}
                    </AnimatePresence>
                </div>

                <div className="am-import-footer">
                    <button className="btn secondary" onClick={onClose} disabled={!canClose}>{t('common.cancel')}</button>
                    <button
                        className="btn primary"
                        onClick={handleStartImport}
                        disabled={isImporting || filePaths.length === 0 || status === 'success' || (filePaths.length === 1 && preview?.valid === false)}
                    >
                        {isImporting ? (<><Loader2 className="spin" size={16} /> {t('Import.processing')}</>)
                            : status === 'success' ? (<><CheckCircle size={16} /> {t('Import.done')}</>)
                                : t('ManageImport.start')}
                    </button>
                </div>

                <AnimatePresence>
                    {showConflictDialog && conflict && (
                        <motion.div
                            className="am-import-conflict-overlay"
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                        >
                            <motion.div
                                className="am-import-conflict-dialog"
                                initial={{ scale: 0.95, opacity: 0 }}
                                animate={{ scale: 1, opacity: 1 }}
                                exit={{ scale: 0.95, opacity: 0 }}
                            >
                                <div className="head">
                                    <AlertTriangle size={22} />
                                    <div className="title">{t('Import.conflict.title')}</div>
                                </div>

                                <div className="body">
                                    {conflict.conflict_type === 'shared_fallback' ? (
                                        <>
                                            <p className="msg">{conflict.message || t('Import.conflict.sharedFallbackMessage')}</p>
                                            <p className="warn">{t('Import.conflict.sharedFallbackWarning')}</p>
                                        </>
                                    ) : (
                                        <>
                                            <p className="msg">{t('Import.conflict.uuidMessage')}</p>
                                            <div className="compare">
                                                <div className="item old">
                                                    <div className="label">{t('Import.conflict.current')}</div>
                                                    <div className="icon">
                                                        {conflict.existing_pack_info?.icon ? (
                                                            <img src={conflict.existing_pack_info.icon} alt="old" />
                                                        ) : (
                                                            <Package size={22} opacity={0.6} />
                                                        )}
                                                    </div>
                                                    <div className="details">
                                                        <div className="name">
                                                            <MinecraftFormattedText text={conflict.existing_pack_info?.name || conflict.target_name} />
                                                        </div>
                                                        <div className="ver">v{conflict.existing_pack_info?.version || t('Import.unknown')}</div>
                                                        <div className="desc">
                                                            <MinecraftFormattedText text={conflict.existing_pack_info?.description || t('Import.noDescription')} />
                                                        </div>
                                                    </div>
                                                </div>
                                                <div className="arrow"><ArrowRight size={18} /></div>
                                                <div className="item new">
                                                    <div className="label">{t('Import.conflict.new')}</div>
                                                    <div className="icon">
                                                        {preview?.icon ? <img src={preview.icon} alt="new" /> : <Package size={22} opacity={0.6} />}
                                                    </div>
                                                    <div className="details">
                                                        <div className="name"><MinecraftFormattedText text={preview?.name || t('Import.unknown')} /></div>
                                                        <div className="ver">v{preview?.version || t('Import.unknown')}</div>
                                                        <div className="desc"><MinecraftFormattedText text={preview?.description || t('Import.noDescription')} /></div>
                                                    </div>
                                                </div>
                                            </div>
                                            <p className="warn">{t('Import.conflict.overwriteWarning')}</p>
                                        </>
                                    )}
                                </div>

                                <div className="actions">
                                    <button
                                        className="btn secondary"
                                        onClick={() => { setShowConflictDialog(false); setConflict(null); conflictIndexRef.current = null; }}
                                    >
                                        {t('common.cancel')}
                                    </button>
                                    {conflict.conflict_type === 'shared_fallback' ? (
                                        <button className="btn primary" onClick={() => resolveConflictAndContinue(false, true)}>
                                            {t('Import.conflict.importToShared')}
                                        </button>
                                    ) : (
                                        <button className="btn danger" onClick={() => resolveConflictAndContinue(true, false)}>
                                            {t('Import.conflict.overwriteImport')}
                                        </button>
                                    )}
                                </div>
                            </motion.div>
                        </motion.div>
                    )}
                </AnimatePresence>
            </motion.div>
        </div>,
        document.body
    );
};
