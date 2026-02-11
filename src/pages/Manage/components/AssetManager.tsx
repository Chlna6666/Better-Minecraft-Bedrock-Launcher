import React, { useEffect, useState, useCallback, useMemo, useRef } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { open as openFileDialog, save as saveFileDialog } from '@tauri-apps/plugin-dialog';
import { openFolder } from '../../../utils/fileActions';

import { motion, AnimatePresence } from 'framer-motion';
import {
    Trash2, Plus, Package, Clock, Layers, Box, Search, X,
    FilePenLine, FolderOpen, Archive, Share2, Loader2, HardDrive,
    ToggleLeft, ToggleRight, CheckSquare, MoreHorizontal, UploadCloud,
    ArrowUp, ArrowDown, Type, Play
} from 'lucide-react';

import { useToast } from '../../../components/Toast';
import { Button, Select } from '../../../components';
import { deleteAsset } from '../api/assetApi';
import { GDKUserSelect } from './GDKUserSelect';
import { ConfirmModal } from './ConfirmModal';
import { LevelDatEditor } from './LevelDatEditor';
import { AssetContextMenu, ContextMenuItem } from './AssetContextMenu';
import MinecraftFormattedText from '../../../utils/MinecraftFormattedText';
import { useTranslation } from 'react-i18next';
import Slider from '../../../components/Slider/Slider';
import './AssetManager.css';

export type AssetType = 'dll_mod' | 'map' | 'resource_pack';
type PackSubtype = 'resource' | 'behavior';
type SortKey = 'name' | 'date' | 'size';

interface AssetManagerProps {
    version: any;
    type: AssetType;
    fileExtensions: string[];
    onLaunchMap?: (folderName: string, mapName: string) => void;
}

interface AssetItem {
    id: string;
    uniqueKey: string;
    name: string;
    folderName: string;
    path?: string;
    image?: string;
    enabled?: boolean;
    modType?: string;
    injectDelayMs?: number;
    size?: string;
    sizeBytes?: number;
    lastPlayed?: number;
    modified?: string;
    description?: string;
    source?: string;
    edition?: string;
}

export const AssetManager: React.FC<AssetManagerProps> = ({
                                                              version,
                                                              type,
                                                              fileExtensions,
                                                              onLaunchMap
                                                          }) => {
    // --- State ---
    const [assets, setAssets] = useState<AssetItem[]>([]);
    const [loading, setLoading] = useState(false);
    const [gdkUserId, setGdkUserId] = useState<string | null>(null);
    const [packSubtype, setPackSubtype] = useState<PackSubtype>('resource');
    const [searchQuery, setSearchQuery] = useState('');

    const toast = useToast();
    const { t, i18n } = useTranslation();

    const [sortConfig, setSortConfig] = useState<{ key: SortKey; order: 'asc' | 'desc' }>(() => {
        if (type === 'map') return { key: 'date', order: 'desc' };
        return { key: 'name', order: 'asc' };
    });

    useEffect(() => {
        if (type === 'map') setSortConfig({ key: 'date', order: 'desc' });
        else setSortConfig({ key: 'name', order: 'asc' });
    }, [type]);

    const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
    const lastSelectedRef = useRef<string | null>(null);

    // 用于存储当前是否处于隔离模式 (Redirection)
    const isIsolationActiveRef = useRef<boolean>(false);

    const [contextMenu, setContextMenu] = useState<{ x: number, y: number, items: ContextMenuItem[] } | null>(null);

    const [isDragging, setIsDragging] = useState(false);

    const [confirmModal, setConfirmModal] = useState<{
        isOpen: boolean,
        title: string,
        content: React.ReactNode,
        onConfirm: () => void,
        isDanger?: boolean
    } | null>(null);

    const [injectDelayEditing, setInjectDelayEditing] = useState<{
        item: AssetItem;
        draft: string;
    } | null>(null);

    const [modTypeEditing, setModTypeEditing] = useState<{
        item: AssetItem;
        draftType: string;
        draftDelay: string;
    } | null>(null);

    const delayMax = 60000;
    const delayStep = 100;
    const delayPresets = [0, 100, 250, 500, 1000, 2000, 3000, 5000, 10000, 15000, 30000, 60000];
    const modTypeOptions = useMemo(() => ([
        { value: 'preload-native', label: t("AssetManager.mod_type_preload_native") },
        { value: 'hot-inject', label: t("AssetManager.mod_type_hot_inject") },
        { value: 'native', label: t("AssetManager.mod_type_native") },
        { value: 'lse-quickjs', label: t("AssetManager.mod_type_lse_quickjs") },
    ]), [t]);

    const parseDelay = (raw: string) => {
        const n = Number(String(raw ?? '').trim());
        if (!Number.isFinite(n) || n < 0) return null;
        return Math.floor(n);
    };

    const clampDelay = (v: number) => Math.min(delayMax, Math.max(0, v));

    const renderDelayEditor = (draft: string, onDraftChange: (next: string) => void) => {
        const parsed = parseDelay(draft);
        const safe = clampDelay(parsed ?? 0);
        return (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <Slider
                    min={0}
                    max={delayMax}
                    step={delayStep}
                    value={safe}
                    onChange={(v) => onDraftChange(String(v))}
                />
                <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                    <input
                        type="number"
                        min={0}
                        step={delayStep}
                        value={draft}
                        onChange={(e) => onDraftChange(e.target.value)}
                        style={{
                            flex: 1,
                            padding: '10px 12px',
                            borderRadius: 10,
                            border: '1px solid rgba(255,255,255,0.12)',
                            background: 'rgba(0,0,0,0.2)',
                            color: 'var(--text-color, #fff)',
                            outline: 'none',
                        }}
                    />
                    <span style={{ fontSize: 13, opacity: 0.75 }}>ms</span>
                </div>
                <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                    {[
                        { label: t("AssetManager.delay_reset"), value: 0 },
                        { label: '+100', delta: 100 },
                        { label: '+1000', delta: 1000 },
                        { label: '+5000', delta: 5000 },
                    ].map((b) => (
                        <button
                            key={b.label}
                            type="button"
                            onClick={() => {
                                if ('value' in b) {
                                    onDraftChange(String(clampDelay(b.value)));
                                } else {
                                    const base = parseDelay(draft) ?? 0;
                                    onDraftChange(String(clampDelay(base + b.delta)));
                                }
                            }}
                            style={{
                                padding: '6px 10px',
                                borderRadius: 10,
                                border: '1px solid rgba(255,255,255,0.12)',
                                background: 'rgba(0,0,0,0.12)',
                                color: 'var(--text-color, #fff)',
                                cursor: 'pointer',
                                fontSize: 12,
                            }}
                        >
                            {b.label}
                        </button>
                    ))}
                </div>
                <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                    <div style={{ width: 180 }}>
                        <Select
                            value={safe}
                            onChange={(v: any) => onDraftChange(String(clampDelay(Number(v))))}
                            options={delayPresets.map((ms) => ({ value: ms, label: `${ms} ms` }))}
                            placeholder={t("AssetManager.delay_preset")}
                            size="md"
                        />
                    </div>
                </div>
            </div>
        );
    };

    const modTypeLabel = (raw?: string) => {
        const v = String(raw || '').trim();
        if (!v) return t("common.unknown");
        const found = modTypeOptions.find(o => o.value === v);
        return found?.label || v;
    };

    const [editorData, setEditorData] = useState<any>(null);
    const [editingItem, setEditingItem] = useState<AssetItem | null>(null);
    const [isProcessing, setIsProcessing] = useState(false);

    const containerRef = useRef<HTMLDivElement>(null);

    // 判断当前版本是否为 GDK
    const isGDK = useMemo(() => version?.kind?.toString().toLowerCase() === 'gdk', [version]);
    const isMap = type === 'map';
    const isPack = type === 'resource_pack';

    // 辅助函数：解析版本 Edition (release / preview / education)
    // 返回值对应 Rust 枚举的 snake_case 字符串
    const getEditionParam = useCallback(() => {
        const vType = String(version?.versionType || '').toLowerCase();
        if (vType.includes('education')) {
            return vType.includes('preview') ? 'education_preview' : 'education';
        }
        return vType.includes('preview') ? 'preview' : 'release';
    }, [version]);

    const edition = getEditionParam();

    // --- Fetch Data ---
    const fetchAssets = useCallback(async () => {
        // 1. 获取配置 & 隔离状态
        let enableIsolation = false;
        const versionName = version?.folder || "";

        if (versionName) {
            try {
                // Tauri 2.0 自动映射: JS { folderName } -> Rust { folder_name }
                const config: any = await invoke('get_version_config', { folderName: versionName });
                if (config && config.enable_redirection) {
                    enableIsolation = true;
                }
            } catch (e) {
                console.warn("Config check failed", e);
            }
        }
        isIsolationActiveRef.current = enableIsolation;

        // 2. 准备标准参数
        const buildType = String(version?.kind || 'uwp').toLowerCase();
        const currentEdition = getEditionParam();

        // [GDK 阻塞逻辑]
        // 只有当: 是GDK地图 且 没选用户 且 没开启隔离 时，才阻塞加载。
        // (开启隔离后，后端会自动扫描隔离目录下的所有用户，无需强制指定)
        if (buildType === 'gdk' && isMap && !gdkUserId && !enableIsolation) {
            setAssets([]);
            return;
        }

        setLoading(true);
        setSelectedKeys(new Set());

        // 构造传给后端的通用参数对象
        // Rust 参数名 (snake_case): build_type, edition, version_name, enable_isolation, user_id
        // Tauri invoke 自动将 camelCase 转换为 snake_case
        const requestArgs = {
            buildType,
            edition: currentEdition,
            versionName,
            enableIsolation,
            userId: gdkUserId
        };

        try {
            let data: any[] = [];

            if (type === 'dll_mod') {
                // Mod 逻辑目前独立
                data = await invoke('get_mod_list', { folderName: versionName });
            }
            else if (type === 'resource_pack') {
                const cmd = packSubtype === 'behavior' ? 'get_behavior_packs' : 'get_resource_packs';
                // 传入标准参数 + lang
                data = await invoke(cmd, { ...requestArgs, lang: i18n.language });
            }
            else if (type === 'map') {
                data = await invoke('get_minecraft_worlds', requestArgs);
            }

            // 统一数据格式化
            const formatted = Array.isArray(data) ? data.map((item: any, index: number) => {
                const rawId = item.uuid || item.folder_path || item.id || `unknown-${index}`;
                let finalPath = item.path || item.folder_path || item.dir || item.location || "";

                let realFolderName = item.folder_name;
                if (!realFolderName && item.uuid) realFolderName = item.uuid;
                if (!realFolderName && finalPath) {
                    const normalized = finalPath.replace(/\\/g, '/');
                    const parts = normalized.split('/').filter((s: string) => s.length > 0);
                    if (parts.length > 0) realFolderName = parts[parts.length - 1];
                }
                if (type === 'dll_mod' && item.id) realFolderName = item.id;
                if (!realFolderName) realFolderName = item.name || item.level_name || `unknown-${index}`;

                let fallbackName = t("common.unknown_asset");
                if (item.level_name) fallbackName = item.level_name;
                else if (item.manifest?.header?.name) fallbackName = item.manifest.header.name;
                else if (item.manifest_parsed?.header?.name) fallbackName = item.manifest_parsed.header.name;
                else if (item.name) fallbackName = item.name;
                else if (item.fileName) fallbackName = item.fileName;
                else fallbackName = realFolderName;

                let desc = item.description;
                if (!desc && item.manifest?.header?.description) desc = item.manifest.header.description;
                if (item.short_description) desc = item.short_description;

                let iconRaw = item.icon_path || item.iconPath || item.icon || null;
                let displayImage = null;
                if (iconRaw) {
                    if (iconRaw.startsWith('data:')) displayImage = iconRaw;
                    else if (iconRaw.includes('/') || iconRaw.includes('\\')) displayImage = convertFileSrc(iconRaw);
                    else displayImage = `data:image/png;base64,${iconRaw}`;
                }

                return {
                    id: rawId,
                    uniqueKey: `${type}-${packSubtype}-${rawId}-${index}`,
                    name: fallbackName,
                    folderName: realFolderName,
                    path: finalPath,
                    enabled: item.enabled,
                    modType: item.mod_type ?? item.modType ?? item.type,
                    injectDelayMs: item.inject_delay_ms ?? item.injectDelayMs,
                    image: displayImage,
                    lastPlayed: item.lastPlayed,
                    modified: item.modified,
                    description: desc,
                    size: item.size_readable,
                    sizeBytes: item.size_bytes,
                    source: item.source,
                    edition: item.edition
                };
            }) : [];
            setAssets(formatted);
        } catch (e: any) {
            console.error(e);
            toast.error(t("AssetManager.load_failed", { message: e.message }));
        } finally {
            setLoading(false);
        }
    }, [version, type, gdkUserId, isMap, packSubtype, getEditionParam, i18n.language, t]);

    useEffect(() => { if (!editorData) fetchAssets(); }, [fetchAssets, editorData]);

    // --- Search & Sort ---
    const processedAssets = useMemo(() => {
        let result = [...assets];
        if (searchQuery) {
            const q = searchQuery.toLowerCase();
            result = result.filter(a => (a.name || '').toLowerCase().includes(q));
        }
        result.sort((a, b) => {
            let valA: any, valB: any;
            if (sortConfig.key === 'name') {
                valA = a.name.toLowerCase(); valB = b.name.toLowerCase();
            } else if (sortConfig.key === 'size') {
                valA = a.sizeBytes || 0; valB = b.sizeBytes || 0;
            } else {
                valA = a.lastPlayed || (a.modified ? new Date(a.modified).getTime() : 0);
                valB = b.lastPlayed || (b.modified ? new Date(b.modified).getTime() : 0);
            }
            if (valA < valB) return sortConfig.order === 'asc' ? -1 : 1;
            if (valA > valB) return sortConfig.order === 'asc' ? 1 : -1;
            return 0;
        });
        return result;
    }, [assets, searchQuery, sortConfig]);

    const handleSort = (key: SortKey) => {
        setSortConfig(prev => {
            if (prev.key === key) return { key, order: prev.order === 'asc' ? 'desc' : 'asc' };
            return { key, order: key === 'name' ? 'asc' : 'desc' };
        });
    };

    // --- Actions ---

    const handleLaunchMap = (item: AssetItem) => {
        if (!item.folderName) { toast.error(t("AssetManager.no_map_folder")); return; }
        if (onLaunchMap) onLaunchMap(item.folderName, item.name);
        else toast.error(t("AssetManager.launch_not_ready"));
    };

    const handleDeleteAction = (targets: AssetItem[]) => {
        if (!targets || targets.length === 0) return;

        setConfirmModal({
            isOpen: true,
            title: targets.length > 1 ? t("AssetManager.delete_title_bulk") : t("AssetManager.delete_title_single"),
            content: (
                <div>
                    <p>{t("AssetManager.delete_confirm", { count: targets.length })}</p>
                    <p style={{ marginTop: 8, fontSize: 13, color: 'var(--text-muted)' }}>{t("AssetManager.delete_warning")}</p>
                </div>
            ),
            isDanger: true,
            onConfirm: async () => {
                setIsProcessing(true);
                try {
                    if (type === 'dll_mod') {
                        await invoke('delete_mods', {
                            folderName: version?.folder,
                            modIds: targets.map(t => t.folderName)
                        });
                    } else {
                        const deleteTypeMap: any = {
                            'map': 'maps',
                            'resource_pack': packSubtype === 'behavior' ? 'behaviorPacks' : 'resourcePacks'
                        };

                        const buildType = String(version?.kind || 'uwp').toLowerCase();
                        const versionName = version?.folder || "";

                        await Promise.all(targets.map(t => {
                            if (!t.folderName) throw new Error(t("AssetManager.missing_resource_folder", { name: t.name }));

                            return deleteAsset({
                                kind: buildType,
                                userId: gdkUserId,
                                folder: versionName,
                                edition: getEditionParam(),
                                deleteType: deleteTypeMap[type],
                                name: t.folderName,
                                enableIsolation: isIsolationActiveRef.current // 关键：传入隔离状态
                            });
                        }));
                    }
                    toast.success(t("AssetManager.delete_success", { count: targets.length }));
                    setSelectedKeys(new Set());
                    fetchAssets();
                    setConfirmModal(null);
                } catch (e: any) {
                    toast.error(t("AssetManager.delete_failed", { message: e.message }));
                } finally {
                    setIsProcessing(false);
                }
            }
        });
    };

    const handleMenuAction = async (action: string, targetItems?: AssetItem[]) => {
        const targets = targetItems || processedAssets.filter(a => selectedKeys.has(a.uniqueKey));
        const first = targets[0];
        if (!first) return;

        switch (action) {
            case 'launch_map': handleLaunchMap(first); break;
            case 'open_folder':
                if (first.path && first.path.trim() !== "") {
                    try {
                        let target = first.path;
                        if (type === 'dll_mod') {
                            const normalized = String(target).replace(/\//g, '\\');
                            target = normalized.replace(/\\[^\\]+\.dll$/i, '');
                        }
                        await openFolder(target);
                    } catch (e: any) {
                        toast.error(e.message || t("AssetManager.open_folder_failed"));
                    }
                } else { toast.error(t("AssetManager.invalid_path")); }
                break;
            case 'edit_nbt': try { const data = await invoke('read_level_dat_cmd', { folderPath: first.path }); setEditorData(data); setEditingItem(first); } catch (e: any) { toast.error(e.message); } break;
            case 'export': const target = await saveFileDialog({ defaultPath: `${first.name}.mcworld`, filters: [{ name: t("AssetManager.filter_world"), extensions: ['mcworld'] }] }); if (target) await invoke('export_map_cmd', { folderPath: first.path, targetPath: target }); break;
            case 'backup': await invoke('backup_map_cmd', { folderPath: first.path, mapName: first.name }); toast.success(t("AssetManager.backup_success")); break;
            case 'toggle_mod':
                try {
                    await invoke('set_mod', {
                        folderName: version?.folder,
                        modId: first.folderName,
                        enabled: !first.enabled,
                        delay: 0
                    });
                    toast.success(t("AssetManager.status_updated"));
                    fetchAssets();
                } catch (e: any) { toast.error(t("AssetManager.toggle_failed", { message: e.message })); }
                break;
            case 'edit_mod_type': {
                const current = String(first.modType || 'preload-native');
                setModTypeEditing({
                    item: first,
                    draftType: current,
                    draftDelay: String(first.injectDelayMs ?? 0),
                });
                break;
            }
            case 'edit_inject_delay':
                setInjectDelayEditing({
                    item: first,
                    draft: String(first.injectDelayMs ?? 0),
                });
                break;
            case 'delete': handleDeleteAction(targets); break;
        }
    };

    // --- Import Logic ---

    const performImport = async (files: string[]) => {
        if (!files.length) return;
        setIsProcessing(true);
        try {
            const validPaths = files.filter(p => p && p.includes && (p.includes('/') || p.includes('\\')));
            if (validPaths.length === 0) { toast.error(t("AssetManager.import_no_paths")); return; }

            if (type === 'dll_mod') {
                await invoke('import_mods', { folderName: version?.folder, paths: validPaths });
            } else {
                // [修改] 使用新的 import_assets 逻辑
                // 必须构造符合后端结构体的参数
                const buildType = String(version?.kind || 'uwp').toLowerCase();
                const versionName = version?.folder || "";

                const payload = {
                    build_type: buildType,
                    edition: getEditionParam(),
                    version_name: versionName,
                    enable_isolation: isIsolationActiveRef.current,
                    user_id: gdkUserId || null,
                    file_paths: validPaths
                };

                await invoke('import_assets', { payload });
            }
            toast.success(t("AssetManager.import_success", { count: validPaths.length })); fetchAssets();
        } catch (e: any) {
            console.error(e);
            toast.error(typeof e === 'string' ? e : e.message || t("AssetManager.import_failed"));
        } finally { setIsProcessing(false); }
    };

    const handleImportBtn = async () => {
        const selected = await openFileDialog({ multiple: true, filters: [{ name: t("AssetManager.filter_supported"), extensions: fileExtensions }] });
        if (selected) {
            const files = Array.isArray(selected) ? selected : [selected];
            await performImport(files);
        }
    };

    // --- UI Helpers (Sort, Drag, Menu) ---

    const handleCheckboxToggle = (e: React.MouseEvent, key: string) => {
        e.stopPropagation();
        const newSelection = new Set(selectedKeys);
        if (newSelection.has(key)) newSelection.delete(key); else newSelection.add(key);
        lastSelectedRef.current = key;
        setSelectedKeys(newSelection);
    };

    const handleSelect = (e: React.MouseEvent, key: string) => {
        if ((e.target as HTMLElement).closest('.am-list-more-btn') || (e.target as HTMLElement).closest('.am-inline-menu-wrapper')) return;
        const isCheckboxClick = !!(e.target as HTMLElement).closest('.am-list-check');
        let newSelection = new Set(selectedKeys);
        if (isCheckboxClick || e.ctrlKey || e.metaKey) {
            if (newSelection.has(key)) newSelection.delete(key); else newSelection.add(key);
            lastSelectedRef.current = key;
        } else if (e.shiftKey && lastSelectedRef.current) {
            const lastIdx = processedAssets.findIndex(a => a.uniqueKey === lastSelectedRef.current);
            const currIdx = processedAssets.findIndex(a => a.uniqueKey === key);
            if (lastIdx !== -1 && currIdx !== -1) {
                const start = Math.min(lastIdx, currIdx);
                const end = Math.max(lastIdx, currIdx);
                newSelection = new Set();
                for (let i = start; i <= end; i++) newSelection.add(processedAssets[i].uniqueKey);
            }
        } else {
            if (e.button !== 2) { newSelection = new Set([key]); lastSelectedRef.current = key; }
        }
        setSelectedKeys(newSelection);
    };

    const handleKeyDown = (e: React.KeyboardEvent) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
            e.preventDefault();
            setSelectedKeys(new Set(processedAssets.map(a => a.uniqueKey)));
        }
    };

    const handleContextMenu = (e: React.MouseEvent, item: AssetItem) => {
        e.preventDefault(); e.stopPropagation();
        if (!selectedKeys.has(item.uniqueKey)) {
            setSelectedKeys(new Set([item.uniqueKey]));
            lastSelectedRef.current = item.uniqueKey;
        }
        const items = getMenuItems(selectedKeys.has(item.uniqueKey) ? processedAssets.filter(a => selectedKeys.has(a.uniqueKey)) : [item]);
        setContextMenu({ x: e.clientX, y: e.clientY, items });
    };

    const handleMoreBtnClick = (e: React.MouseEvent, item: AssetItem) => {
        e.preventDefault(); e.stopPropagation();
        const nextSelectedKeys = selectedKeys.has(item.uniqueKey)
            ? selectedKeys
            : new Set([item.uniqueKey]);
        if (nextSelectedKeys !== selectedKeys) {
            setSelectedKeys(nextSelectedKeys);
            lastSelectedRef.current = item.uniqueKey;
        }

        const targets = selectedKeys.has(item.uniqueKey)
            ? processedAssets.filter(a => selectedKeys.has(a.uniqueKey))
            : [item];

        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const items = getMenuItems(targets);
        setContextMenu({ x: rect.right, y: rect.bottom, items });
    };

    const handleDragOver = (e: React.DragEvent) => { e.preventDefault(); e.stopPropagation(); if (!isDragging) setIsDragging(true); };
    const handleDragLeave = (e: React.DragEvent) => { e.preventDefault(); e.stopPropagation(); if (e.currentTarget.contains(e.relatedTarget as Node)) return; setIsDragging(false); };
    const handleDrop = async (e: React.DragEvent) => {
        e.preventDefault(); e.stopPropagation(); setIsDragging(false);
        const files = Array.from(e.dataTransfer.files).map((f: any) => f.path || f.name);
        if (files.length > 0) await performImport(files);
    };

    const getMenuItems = (items: AssetItem[]): ContextMenuItem[] => {
        const isMulti = items.length > 1;
        const first = items[0];
        const menuItems: ContextMenuItem[] = [];
        if (!isMulti && first) {
            if (isMap && onLaunchMap) {
                menuItems.push({ label: t("AssetManager.menu_launch"), icon: <Play size={14} />, action: 'launch_map' });
                menuItems.push({ separator: true, label: '', action: '' });
            }
            menuItems.push({ label: t("AssetManager.menu_open_folder"), icon: <FolderOpen size={14} />, action: 'open_folder' });
            if (isMap) {
                menuItems.push({ separator: true, label: '', action: '' });
                menuItems.push({ label: t("AssetManager.menu_edit_nbt"), icon: <FilePenLine size={14} />, action: 'edit_nbt' });
                menuItems.push({ label: t("AssetManager.menu_export"), icon: <Share2 size={14} />, action: 'export' });
                menuItems.push({ label: t("AssetManager.menu_backup"), icon: <Archive size={14} />, action: 'backup' });
            }
            if (type === 'dll_mod') {
                menuItems.push({ separator: true, label: '', action: '' });
                menuItems.push({ label: first.enabled ? t("AssetManager.menu_disable") : t("AssetManager.menu_enable"), icon: first.enabled ? <ToggleLeft size={14} /> : <ToggleRight size={14} />, action: 'toggle_mod' });
                menuItems.push({ label: t("AssetManager.menu_set_mod_type"), icon: <Layers size={14} />, action: 'edit_mod_type' });
                if (first.modType === 'hot-inject') {
                    menuItems.push({ label: t("AssetManager.menu_set_inject_delay"), icon: <Clock size={14} />, action: 'edit_inject_delay' });
                }
            }
        }
        menuItems.push({ separator: true, label: '', action: '' });
        menuItems.push({ label: isMulti ? t("AssetManager.delete_with_count", { count: items.length }) : t("common.delete"), icon: <Trash2 size={14} />, danger: true, action: 'delete' });
        return menuItems;
    };

    const renderSortButton = (label: string, key: SortKey, icon: React.ReactNode) => {
        const isActive = sortConfig.key === key;
        return (
            <button key={key} className={`am-sort-btn ${isActive ? 'active' : ''}`} onClick={() => handleSort(key)}>
                {icon}<span>{label}</span>
                {isActive && <span className="sort-arrow">{sortConfig.order === 'asc' ? <ArrowUp size={12} /> : <ArrowDown size={12} />}</span>}
            </button>
        );
    };

    const renderSortGroup = () => {
        const buttons = [renderSortButton(t("AssetManager.sort_name"), "name", <Type size={14} />)];
        if (type === 'map') {
            buttons.push(renderSortButton(t("AssetManager.sort_date"), "date", <Clock size={14} />));
            buttons.push(renderSortButton(t("AssetManager.sort_size"), "size", <HardDrive size={14} />));
        } else if (type === 'resource_pack') {
            buttons.push(renderSortButton(t("AssetManager.sort_size"), "size", <HardDrive size={14} />));
        }
        return <div className="am-sort-group">{buttons}</div>;
    };

    if (editorData && editingItem) {
        return (
            <LevelDatEditor
                data={editorData}
                fileName={editingItem.name}
                onBack={() => { setEditorData(null); setEditingItem(null); }}
                onSave={async (d) => { await invoke('write_level_dat_cmd', { folderPath: editingItem.path, data: d, version: 9 }); toast.success(t('common.saved')); }}
                onLaunch={isMap && onLaunchMap ? () => handleLaunchMap(editingItem) : undefined}
            />
        );
    }

    return (
        <div className="asset-manager-container" ref={containerRef} tabIndex={0} onKeyDown={handleKeyDown} onClick={() => setSelectedKeys(new Set())} onDragOver={handleDragOver} onDragLeave={handleDragLeave} onDrop={handleDrop} style={{ outline: 'none' }}>
            <AnimatePresence>{isDragging && <motion.div className="am-drag-overlay" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}><div className="drag-content"><UploadCloud size={48} className="bounce" /><p>{t("AssetManager.drag_drop_hint")}</p></div></motion.div>}</AnimatePresence>

            <div className="am-header">
                <div className="am-header-left">
                    {/* 即使开启隔离，也显示用户选择器，但允许不选（后端自动扫描） */}
                    {isGDK && isMap && <div style={{ flexShrink: 1, minWidth: 0 }}><GDKUserSelect currentUserId={gdkUserId} onChange={setGdkUserId} edition={edition} /></div>}
                    {isPack && (
                        <div className="am-type-switcher">
                            <button className={`switcher-btn ${packSubtype === 'resource' ? 'active' : ''}`} onClick={() => setPackSubtype('resource')}><Package size={14} /> {t("AssetManager.pack_resource")}</button>
                            <button className={`switcher-btn ${packSubtype === 'behavior' ? 'active' : ''}`} onClick={() => setPackSubtype('behavior')}><Layers size={14} /> {t("AssetManager.pack_behavior")}</button>
                        </div>
                    )}
                    <div className="am-search-box">
                        <Search size={14} className="icon" />
                        <input placeholder={t("AssetManager.search_placeholder")} value={searchQuery} onChange={e => setSearchQuery(e.target.value)} />
                        {searchQuery && <X size={14} className="clear-btn" onClick={() => setSearchQuery('')} />}
                    </div>
                    <span className="am-count-badge" title={t("AssetManager.count_title")}>{processedAssets.length}</span>
                </div>

                <div className="am-header-right">
                    {renderSortGroup()}
                    <div className="divider-vertical" style={{ height: 20 }} />
                    {selectedKeys.size > 0 ? (
                        <Button size="sm" variant="danger" onClick={(e) => { e.stopPropagation(); handleDeleteAction(processedAssets.filter(a => selectedKeys.has(a.uniqueKey))); }}>
                            <Trash2 size={16} /> {t("AssetManager.delete_with_count", { count: selectedKeys.size })}
                        </Button>
                    ) : (
                        <Button size="sm" onClick={(e) => { e.stopPropagation(); handleImportBtn(); }}><Plus size={16} /> {t("AssetManager.import_button")}</Button>
                    )}
                </div>
            </div>

            <div className="am-content custom-scrollbar">
                {processedAssets.length === 0 ? <div className="am-empty"><Box size={48} opacity={0.3} /><p>{t("AssetManager.empty")}</p></div> :
                    <div className="am-list-view">
                        <AnimatePresence mode='popLayout'>
                            {processedAssets.map(item => {
                                const isSelected = selectedKeys.has(item.uniqueKey);
                                return (
                                    <motion.div
                                        key={item.uniqueKey}
                                        initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
                                        className={`am-list-item ${isSelected ? 'selected' : ''}`}
                                        onClick={(e) => handleSelect(e, item.uniqueKey)}
                                        onContextMenu={(e) => handleContextMenu(e, item)}
                                    >
                                        <div className="am-list-check" onClick={(e) => handleCheckboxToggle(e, item.uniqueKey)} style={{ cursor: 'pointer' }}>
                                            {isSelected ? <CheckSquare size={16} color="var(--primary-color)" /> : <div className="unchecked-box" />}
                                        </div>
                                        <div className="am-list-icon-wrapper">
                                            {item.image ? <img src={item.image} className="am-list-icon" alt="" /> : <div className="am-list-icon placeholder"><Package size={16} /></div>}
                                        </div>
                                        <div className="am-list-info">
                                            <span className="am-list-title"><MinecraftFormattedText text={item.name} />{type === 'dll_mod' && <span className={`status-dot ${item.enabled ? 'on' : 'off'}`} />}</span>
                                            <div className="am-list-sub">
                                                {item.description && <span className="desc">{item.description.replace(/§./g, '')}</span>}
                                                {type === 'dll_mod' && item.modType && (
                                                    <span className="meta"><Layers size={10} /> {modTypeLabel(item.modType)}</span>
                                                )}
                                                {type === 'dll_mod' && item.modType === 'hot-inject' && typeof item.injectDelayMs === 'number' && (
                                                    <span className="meta"><Clock size={10} /> {t("AssetManager.mod_inject_delay", { ms: item.injectDelayMs })}</span>
                                                )}
                                                {item.size && <span className="meta"><HardDrive size={10} /> {item.size}</span>}
                                                {item.modified && <span className="meta"><Clock size={10} /> {new Date(item.modified).toLocaleDateString()}</span>}
                                                {/* [修改] 移除了 item.source (UWP/GDK) 的显示 */}
                                            </div>
                                        </div>
                                        <div className="am-inline-menu-wrapper" style={{ position: 'relative', display: 'flex', alignItems: 'center', gap: 4 }}>
                                            {isMap && onLaunchMap && (
                                                <button className="am-list-more-btn" title={t("AssetManager.menu_launch")} onClick={(e) => { e.stopPropagation(); handleLaunchMap(item); }} style={{ color: 'var(--success-color, #4caf50)', opacity: 0.8 }} >
                                                    <Play size={16} fill="currentColor" />
                                                </button>
                                            )}
                                            <button className="am-list-more-btn" onClick={(e) => handleMoreBtnClick(e, item)}><MoreHorizontal size={16} /></button>
                                        </div>                                    </motion.div>
                                );
                            })}
                        </AnimatePresence>
                    </div>
                }
            </div>

            {contextMenu && <AssetContextMenu x={contextMenu.x} y={contextMenu.y} items={contextMenu.items} onClose={() => setContextMenu(null)} onAction={(act) => handleMenuAction(act)} />}

            {confirmModal && <ConfirmModal {...confirmModal} onCancel={() => setConfirmModal(null)} isLoading={isProcessing} />}

            {injectDelayEditing && (
                <ConfirmModal
                    isOpen={true}
                    title={t("AssetManager.inject_delay_title")}
                    content={
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                            <div style={{ fontSize: 13, opacity: 0.8 }}>{t("AssetManager.inject_delay_help")}</div>
                            {renderDelayEditor(
                                injectDelayEditing.draft,
                                (next) => setInjectDelayEditing(prev => prev ? ({ ...prev, draft: next }) : prev)
                            )}
                        </div>
                    }
                    confirmText={t("common.save")}
                    cancelText={t("common.cancel")}
                    onCancel={() => setInjectDelayEditing(null)}
                    onConfirm={async () => {
                        const n = parseDelay(injectDelayEditing.draft);
                        if (n === null) {
                            toast.error(t("AssetManager.inject_delay_invalid"));
                            return;
                        }
                        setIsProcessing(true);
                        try {
                            await invoke('set_mod_inject_delay', {
                                folderName: version?.folder,
                                modId: injectDelayEditing.item.folderName,
                                injectDelayMs: clampDelay(n),
                            });
                            toast.success(t("common.saved"));
                            setInjectDelayEditing(null);
                            fetchAssets();
                        } catch (e: any) {
                            toast.error(t("common.save_failed"));
                        } finally {
                            setIsProcessing(false);
                        }
                    }}
                    isLoading={isProcessing}
                />
            )}

            {modTypeEditing && (
                <ConfirmModal
                    isOpen={true}
                    title={t("AssetManager.mod_type_title")}
                    content={
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                            <div style={{ fontSize: 13, opacity: 0.8 }}>{t("AssetManager.mod_type_help")}</div>
                            <Select
                                value={modTypeEditing.draftType}
                                onChange={(v: any) => {
                                    const next = String(v || '');
                                    setModTypeEditing(prev => prev ? ({ ...prev, draftType: next }) : prev);
                                }}
                                options={modTypeOptions}
                                size="md"
                                style={{ width: '100%' }}
                            />

                            {modTypeEditing.draftType === 'hot-inject' && (
                                <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                                    <div style={{ fontSize: 13, opacity: 0.8 }}>{t("AssetManager.inject_delay_help")}</div>
                                    {renderDelayEditor(
                                        modTypeEditing.draftDelay,
                                        (next) => setModTypeEditing(prev => prev ? ({ ...prev, draftDelay: next }) : prev)
                                    )}
                                </div>
                            )}
                        </div>
                    }
                    confirmText={t("common.save")}
                    cancelText={t("common.cancel")}
                    onCancel={() => setModTypeEditing(null)}
                    onConfirm={async () => {
                        setIsProcessing(true);
                        try {
                            await invoke('set_mod_type', {
                                folderName: version?.folder,
                                modId: modTypeEditing.item.folderName,
                                modType: modTypeEditing.draftType,
                            });

                            if (modTypeEditing.draftType === 'hot-inject') {
                                const n = parseDelay(modTypeEditing.draftDelay);
                                if (n === null) {
                                    toast.error(t("AssetManager.inject_delay_invalid"));
                                    return;
                                }
                                await invoke('set_mod_inject_delay', {
                                    folderName: version?.folder,
                                    modId: modTypeEditing.item.folderName,
                                    injectDelayMs: clampDelay(n),
                                });
                            }

                            toast.success(t("common.saved"));
                            setModTypeEditing(null);
                            fetchAssets();
                        } catch (e: any) {
                            toast.error(t("common.save_failed"));
                        } finally {
                            setIsProcessing(false);
                        }
                    }}
                    isLoading={isProcessing}
                />
            )}
        </div>
    );
};
