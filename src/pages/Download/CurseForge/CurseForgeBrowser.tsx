import React, { useState, useEffect, useMemo, useRef, useDeferredValue, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { useLocation, useNavigate } from 'react-router-dom';
import { motion, AnimatePresence } from 'framer-motion';
import {
    Package, Loader2, Download, Calendar, User,
    List, ChevronLeft, ChevronRight, LayoutGrid, X, ChevronDown, Clipboard
} from 'lucide-react'; // 引入 List/Grid 图标
import { useTranslation } from 'react-i18next';
import { useToast } from '../../../components/Toast';
import Select from "../../../components/Select.jsx";
import { tCurseForgeTag } from "./curseForgeTagI18n";
import { CurseForgeInstallModal } from "./CurseForgeInstallModal";
import { idbCacheGet, idbCachePrune, idbCacheSet } from "../../../utils/idbCache";
import './CurseForgeBrowser.css';

// --- Types ---
interface Category {
    id: number;
    name: string;
    slug: string;
    iconUrl?: string;
    isClass?: boolean;
    classId?: number;
    parentCategoryId?: number;
}

interface Mod {
    id: number;
    name: string;
    summary: string;
    authors: { name: string }[];
    logo: { url: string; thumbnailUrl?: string };
    downloadCount: number;
    categories: Category[];
    dateModified: string;
    classId?: number;
    latestFilesIndexes?: any[]; // 用于获取版本信息
}

interface ModFile {
    id: number;
    displayName: string;
    fileName: string;
    fileLength: number;
    downloadUrl?: string;
    gameVersions: string[];
    fileDate: string;
}

const buildSortOptions = (t: (key: string, options?: any) => string) => [
    { label: t("CurseForge.sort_featured"), value: 1 },
    { label: t("CurseForge.sort_popularity"), value: 2 },
    { label: t("CurseForge.sort_updated"), value: 3 },
    { label: t("CurseForge.sort_name"), value: 4 },
    { label: t("CurseForge.sort_downloads"), value: 6 },
];

const CachedImage = ({ src, alt, className }: { src?: string; alt?: string; className?: string }) => (
    <img
        src={src}
        alt={alt || ''}
        className={className}
        loading="lazy"
        referrerPolicy="no-referrer"
    />
);

interface Props {
    searchQuery: string;
    refreshNonce?: number;
    onLoadingChange?: (loading: boolean) => void;
    onClearSearch?: () => void;
}

const PAGE_SIZE = 20;
const CACHE_TTL = 1000 * 60 * 60 * 24; // 1 day 缓存
const MOD_FILE_CACHE_TTL = 1000 * 60 * 30;
const SKELETON_DELAY_MS = 120;

export const CurseForgeBrowser: React.FC<Props> = ({ searchQuery, refreshNonce, onLoadingChange, onClearSearch }) => {
    const { t } = useTranslation();
    const location = useLocation();
    const initialCfState = (location.state as any)?.cfState;
    const resultsRef = useRef<HTMLDivElement | null>(null);
    const didRestoreScrollRef = useRef(false);
    // Data
    const [allCategories, setAllCategories] = useState<Category[]>([]);
    const [mods, setMods] = useState<Mod[]>([]);
    const [versions, setVersions] = useState<{value: string, label: string}[]>([]);

    // Filter States
    const [selectedRootId, setSelectedRootId] = useState<number | null>(() => initialCfState?.selectedRootId ?? null);
    const [selectedSubId, setSelectedSubId] = useState<number | null>(() => initialCfState?.selectedSubId ?? null);
    const [selectedVersion, setSelectedVersion] = useState<string>(() => initialCfState?.selectedVersion ?? '');
    const [currentSort, setCurrentSort] = useState<number>(() => initialCfState?.currentSort ?? 1);

    // UI States
    const [fetching, setFetching] = useState(false);
    const [slotReady, setSlotReady] = useState(false);
    const [viewMode, setViewMode] = useState<'grid' | 'list'>(() => initialCfState?.viewMode ?? 'list'); // 默认为列表，更专业
    const [hasLoadedOnce, setHasLoadedOnce] = useState(false);
    const [showSkeleton, setShowSkeleton] = useState(false);
    const [subCollapsed, setSubCollapsed] = useState(false);
    const deferredSearch = useDeferredValue(searchQuery);
    const didInitRefreshRef = useRef(false);
    const [reloadSeq, setReloadSeq] = useState(0);
    const [listAnimOn, setListAnimOn] = useState(true);
    const skeletonTimerRef = useRef<number | null>(null);
    const lastKeyRef = useRef<string>('');
    const [lastDataTs, setLastDataTs] = useState<number | null>(null);
    const [lastDataSource, setLastDataSource] = useState<'memory' | 'idb' | 'network' | null>(null);
    const [progressSeq, setProgressSeq] = useState(0);

    // Pagination States
    const [page, setPage] = useState(() => initialCfState?.page ?? 1);
    const [jumpPage, setJumpPage] = useState('');
    // 注意：CF API 某些端点不直接返回 totalCount，这里模拟无限滚动或简单分页
    // 如果后端 search_curseforge_mods 返回 { data: [], pagination: ... } 更好，
    // 这里假设只返回了列表，我们通过是否抓够了 PAGE_SIZE 来判断是否有下一页
    const [hasMore, setHasMore] = useState(true);

    // Quick download modal
    const [downloadTarget, setDownloadTarget] = useState<Mod | null>(null);
    const [downloadFiles, setDownloadFiles] = useState<ModFile[]>([]);
    const [downloadsLoading, setDownloadsLoading] = useState(false);
    const [installMod, setInstallMod] = useState<Mod | null>(null);
    const [installFile, setInstallFile] = useState<ModFile | null>(null);
    const [installOpen, setInstallOpen] = useState(false);

    // caches
    const modsCache = useRef<Map<string, { ts: number, data: Mod[], hasMore: boolean }>>(new Map());
    const filesCache = useRef<Map<number, { ts: number, data: ModFile[] }>>(new Map());
    const latestReq = useRef(0);

    const toast = useToast();
    const navigate = useNavigate();
    const sortOptions = useMemo(() => buildSortOptions(t), [t]);

    const navigateToMod = useCallback((modId: number) => {
        navigate(`/curseforge/mod/${modId}`, {
            state: {
                from: {
                    pathname: location.pathname,
                    search: location.search,
                    state: {
                        ...(location.state as any),
                        initialTab: 'resource',
                        searchTerm: searchQuery,
                        cfState: {
                            selectedRootId,
                            selectedSubId,
                            selectedVersion,
                            currentSort,
                            viewMode,
                            page,
                            scrollTop: resultsRef.current?.scrollTop ?? 0,
                        },
                    },
                },
            },
        });
    }, [
        navigate,
        location.pathname,
        location.search,
        location.state,
        searchQuery,
        selectedRootId,
        selectedSubId,
        selectedVersion,
        currentSort,
        viewMode,
        page,
    ]);

    const readSessionCache = <T,>(key: string, ttl: number): T | null => {
        try {
            const raw = sessionStorage.getItem(key);
            if (!raw) return null;
            const parsed = JSON.parse(raw);
            if (!parsed?.value || !parsed?.ts) return null;
            if (Date.now() - parsed.ts > ttl) return null;
            return parsed.value as T;
        } catch {
            return null;
        }
    };

    const writeSessionCache = (key: string, value: any) => {
        try {
            sessionStorage.setItem(key, JSON.stringify({ ts: Date.now(), value }));
        } catch { /* ignore quota */ }
    };

    // 1. Init Data
    useEffect(() => {
        const init = async () => {
            try {
                const cachedCats = readSessionCache<Category[]>('cf:categories', CACHE_TTL);
                const cachedVers = readSessionCache<string[]>('cf:versions', CACHE_TTL);

                const [cats, vers] = await Promise.all([
                    cachedCats ? Promise.resolve(cachedCats) : invoke<Category[]>('get_curseforge_categories'),
                    cachedVers ? Promise.resolve(cachedVers) : invoke<string[]>('get_minecraft_versions')
                ]);

                const sortedCats = cats.sort((a, b) => {
                    if (a.slug.includes('addons') || a.slug.includes('mods')) return -1;
                    if (b.slug.includes('addons') || b.slug.includes('mods')) return 1;
                    return a.id - b.id;
                });

                setAllCategories(sortedCats);
                setVersions([
                    { value: '', label: t("CurseForge.all_versions") },
                    ...vers.map(v => ({ value: v, label: v }))
                ]);

                if (!cachedCats) writeSessionCache('cf:categories', cats);
                if (!cachedVers) writeSessionCache('cf:versions', vers);
            } catch (e) {
                console.error(e);
                toast.error(t("CurseForge.connect_failed"));
            }
        };
        init();
        setTimeout(() => setSlotReady(!!document.getElementById('cf-header-slot')), 50);
    }, [t]);

    useEffect(() => {
        idbCachePrune('cf_mods', CACHE_TTL);
    }, []);

    // 2. Derived
    const rootCategories = useMemo(() => allCategories.filter(c => c.isClass === true), [allCategories]);
    const subCategories = useMemo(() => {
        if (!selectedRootId) return [];
        return allCategories.filter(c => c.classId === selectedRootId || c.parentCategoryId === selectedRootId);
    }, [allCategories, selectedRootId]);

    // Reset page when filters change
    useEffect(() => { setPage(1); }, [selectedRootId, selectedSubId, selectedVersion, currentSort, deferredSearch]);

    // 持久化视图模式
    useEffect(() => {
        const stored = sessionStorage.getItem('cf:viewMode');
        if (stored === 'grid' || stored === 'list') setViewMode(stored);
    }, []);
    useEffect(() => {
        sessionStorage.setItem('cf:viewMode', viewMode);
    }, [viewMode]);

    // 3. Load Mods
    useEffect(() => {
        const loadMods = async () => {
            const index = (page - 1) * PAGE_SIZE;
            const cacheKey = [
                selectedRootId ?? 'all',
                selectedSubId ?? 'all',
                selectedVersion || 'all',
                currentSort,
                deferredSearch || 'none',
                page
            ].join('|');

            const reqId = ++latestReq.current;

            if (skeletonTimerRef.current) {
                clearTimeout(skeletonTimerRef.current);
                skeletonTimerRef.current = null;
            }

            const applyCached = (cachedData: Mod[], cachedHasMore: boolean) => {
                if (reqId !== latestReq.current) return;
                setMods(cachedData);
                setHasMore(cachedHasMore);
                setHasLoadedOnce(true);
                setShowSkeleton(false);
                lastKeyRef.current = cacheKey;
            };

            const now = Date.now();
            const memCached = modsCache.current.get(cacheKey);
            const memFresh = !!memCached && now - memCached.ts < CACHE_TTL;
            if (memFresh && memCached) {
                applyCached(memCached.data, memCached.hasMore);
                setLastDataSource('memory');
                setLastDataTs(memCached.ts);
            } else {
                const keyChanged = cacheKey !== lastKeyRef.current;
                if (keyChanged) {
                    setMods([]);
                    setHasLoadedOnce(false);
                    setShowSkeleton(true);
                } else {
                    setShowSkeleton(false);
                    skeletonTimerRef.current = window.setTimeout(() => {
                        if (reqId !== latestReq.current) return;
                        setShowSkeleton(true);
                    }, SKELETON_DELAY_MS);
                }

                idbCacheGet<{ data: Mod[]; hasMore: boolean }>('cf_mods', cacheKey).then((rec) => {
                    if (reqId !== latestReq.current) return;
                    if (!rec) return;
                    if (Date.now() - rec.ts > CACHE_TTL) return;
                    const persisted = rec.value;
                    if (!persisted?.data) return;
                    modsCache.current.set(cacheKey, { ts: rec.ts, data: persisted.data, hasMore: persisted.hasMore });
                    applyCached(persisted.data, persisted.hasMore);
                    setLastDataSource('idb');
                    setLastDataTs(rec.ts);
                });
            }

            setFetching(true);
            try {
                const res = await invoke<Mod[]>('search_curseforge_mods', {
                    classId: selectedRootId,
                    categoryId: selectedSubId,
                    gameVersion: selectedVersion || null,
                    searchFilter: deferredSearch || null,
                    sortField: currentSort,
                    sortOrder: 'desc',
                    pageSize: PAGE_SIZE,
                    index: index
                });

                if (reqId !== latestReq.current) return;

                const uniqueMods = res && res.length > 0 ? Array.from(new Map(res.map(m => [m.id, m])).values()) : [];
                setMods(uniqueMods);
                const more = uniqueMods.length === PAGE_SIZE;
                setHasMore(more); // 如果取满了一页，假设还有下一页
                const ts = Date.now();
                modsCache.current.set(cacheKey, { ts, data: uniqueMods, hasMore: more });
                idbCacheSet('cf_mods', cacheKey, { data: uniqueMods, hasMore: more });
                lastKeyRef.current = cacheKey;
                setLastDataSource('network');
                setLastDataTs(ts);
            } catch (e) {
                console.error(e);
            } finally {
                if (reqId === latestReq.current) {
                    setFetching(false);
                    setHasLoadedOnce(true);
                    setShowSkeleton(false);
                }
            }
        };

        // 防抖
        const timer = setTimeout(loadMods, 300);
        return () => clearTimeout(timer);
    }, [selectedRootId, selectedSubId, selectedVersion, currentSort, deferredSearch, page, reloadSeq]);

    const openDownloadSheet = useCallback(async (mod: Mod) => {
        setDownloadTarget(mod);
        setDownloadFiles([]);
        const cached = filesCache.current.get(mod.id);
        if (cached && Date.now() - cached.ts < MOD_FILE_CACHE_TTL) {
            setDownloadFiles(cached.data);
            return;
        }
        setDownloadsLoading(true);
        try {
            const files = await invoke<ModFile[]>('get_curseforge_mod_files', { modId: mod.id, gameVersion: selectedVersion || null });
            setDownloadFiles(files || []);
            filesCache.current.set(mod.id, { ts: Date.now(), data: files || [] });
        } catch (e) {
            console.error(e);
            toast.error(t("CurseForge.connect_failed"));
            setDownloadFiles([]);
        } finally {
            setDownloadsLoading(false);
        }
    }, [selectedVersion, t, toast]);

    // 4. Portal Content
    const HeaderControls = (
        <>
            <div className="cf-view-toggle">
                <button
                    className={`toggle-btn ${viewMode === 'list' ? 'active' : ''}`}
                    onClick={() => setViewMode('list')}
                    title={t("CurseForge.view_list")}
                >
                    <List size={16} />
                </button>
                <button
                    className={`toggle-btn ${viewMode === 'grid' ? 'active' : ''}`}
                    onClick={() => setViewMode('grid')}
                    title={t("CurseForge.view_grid")}
                >
                    <LayoutGrid size={16} />
                </button>
            </div>

            <div className="divider-vertical"></div>

            <div style={{ width: 140 }}>
                <Select
                    value={selectedVersion}
                    onChange={setSelectedVersion}
                    options={versions}
                    size="md"
                    className="cf-header-select"
                    placeholder={t("CurseForge.all_versions")}
                />
            </div>
            <div style={{ width: 150 }}>
                <Select
                    value={currentSort}
                    onChange={setCurrentSort}
                    options={sortOptions}
                    size="md"
                    className="cf-header-select"
                />
            </div>
        </>
    );

    const isLoadingState = showSkeleton && !hasLoadedOnce;
    const canShowEmptyState = hasLoadedOnce && !fetching && mods.length === 0;

    const currentRootLabel = useMemo(() => {
        if (selectedRootId === null) return t("common.all");
        const cat = allCategories.find(c => c.id === selectedRootId);
        return cat ? tCurseForgeTag(t, cat) : t("common.all");
    }, [allCategories, selectedRootId, t]);

    const currentSubLabel = useMemo(() => {
        if (selectedSubId === null) return null;
        const cat = allCategories.find(c => c.id === selectedSubId);
        return cat ? tCurseForgeTag(t, cat) : null;
    }, [allCategories, selectedSubId, t]);

    const currentSortLabel = useMemo(() => {
        const opt = sortOptions.find((o: any) => o.value === currentSort);
        return opt?.label || t("CurseForge.sort_featured");
    }, [sortOptions, currentSort, t]);

    const lastUpdatedText = useMemo(() => {
        if (!lastDataTs) return null;
        const d = new Date(lastDataTs);
        const hh = String(d.getHours()).padStart(2, '0');
        const mm = String(d.getMinutes()).padStart(2, '0');
        const ss = String(d.getSeconds()).padStart(2, '0');
        return `${hh}:${mm}:${ss}`;
    }, [lastDataTs]);

    useEffect(() => {
        onLoadingChange?.(fetching || isLoadingState);
    }, [fetching, isLoadingState, onLoadingChange]);

    useEffect(() => {
        if (!didInitRefreshRef.current) {
            didInitRefreshRef.current = true;
            return;
        }
        setReloadSeq((v) => v + 1);
    }, [refreshNonce]);

    useEffect(() => {
        if (!didInitRefreshRef.current) return;
        setListAnimOn(false);
        requestAnimationFrame(() => setListAnimOn(true));
    }, [reloadSeq]);

    useEffect(() => {
        setProgressSeq((v) => v + 1);
    }, [page]);

    const parseSharedCurseForge = (text: string) => {
        const trimmed = (text || '').trim();
        if (!trimmed) return null;

        const normalized = trimmed
            .replace(/\uFF1A/g, ':')
            .replace(/\u200B/g, '')
            .replace(/\s+/g, ' ');

        const patterns = [
            /\/curseforge\/mod\/(\d+)/i,
            /ID\s*[:：]\s*(\d+)/i,
            /\bID\s*(\d+)\b/i,
            /projects\/(\d+)/i,
        ];
        for (const pattern of patterns) {
            const match = normalized.match(pattern);
            if (match?.[1]) {
                const id = Number(match[1]);
                if (Number.isFinite(id) && id > 0) return { id };
            }
        }
        return null;
    };

    const openFromClipboard = useCallback(async () => {
        try {
            const text = await navigator.clipboard.readText();
            const parsed = parseSharedCurseForge(text);
            if (!parsed?.id) {
                toast.error('未识别到资源 ID');
                return;
            }
            toast.success('已识别分享内容，正在打开…');
            navigateToMod(parsed.id);
        } catch {
            toast.error('读取剪贴板失败');
        }
    }, [navigateToMod, toast]);

    useEffect(() => {
        const onPaste = (e: ClipboardEvent) => {
            const text = e.clipboardData?.getData('text/plain') || '';
            const parsed = parseSharedCurseForge(text);
            if (!parsed?.id) return;

            toast.success('已识别分享内容，正在打开…');
            navigateToMod(parsed.id);
        };

        window.addEventListener('paste', onPaste);
        return () => window.removeEventListener('paste', onPaste);
    }, [navigateToMod, toast]);

    useEffect(() => {
        if (didRestoreScrollRef.current) return;
        if (isLoadingState) return;
        const el = resultsRef.current;
        if (!el) return;
        const top = initialCfState?.scrollTop;
        if (typeof top === 'number' && top > 0) {
            el.scrollTop = top;
        }
        didRestoreScrollRef.current = true;
    }, [isLoadingState, initialCfState?.scrollTop]);
    const maxPage = !hasMore ? page : null;
    const pageInfoText = useMemo(() => {
        if (maxPage) return `第 ${page} / ${maxPage} 页`;
        const raw = t("CurseForge.page_info", { page });
        if (raw && !raw.includes("{{")) return raw;
        return `第 ${page} / ? 页`;
    }, [t, page, maxPage]);

    const applyJump = () => {
        let next = Math.max(1, parseInt(jumpPage, 10) || 1);
        if (maxPage && next > maxPage) next = maxPage;
        setPage(next);
        setJumpPage('');
    };

    return (
        <div className="cf-browser-container">
            {slotReady && document.getElementById('cf-header-slot') &&
                createPortal(HeaderControls, document.getElementById('cf-header-slot')!)
            }

            {/* Sidebar */}
            <div className="cf-sidebar custom-scrollbar">
                <div className="cf-sidebar-header">{t("CurseForge.categories")}</div>
                <div
                    className={`cf-sidebar-item ${selectedRootId === null ? 'active' : ''}`}
                    onClick={() => { setSelectedRootId(null); setSelectedSubId(null); }}
                >
                    <Package size={18} className="cat-icon" />
                    <span>{t("common.all")}</span>
                </div>
                {rootCategories.map(cat => (
                    <div
                        key={cat.id}
                        className={`cf-sidebar-item ${selectedRootId === cat.id ? 'active' : ''}`}
                        onClick={() => { setSelectedRootId(cat.id); setSelectedSubId(null); }}
                    >
                        <CachedImage src={cat.iconUrl} className="cat-icon" />
                        <span>{tCurseForgeTag(t, cat)}</span>
                    </div>
                ))}
                {subCategories.length > 0 && (
                    <>
                        <div className="cf-sidebar-section-header" onClick={() => setSubCollapsed(v => !v)}>
                            <span>{t("CurseForge.subcategories", { defaultValue: "子分类" })}</span>
                            <ChevronDown size={14} className={`cf-collapse-icon ${subCollapsed ? 'collapsed' : ''}`} />
                        </div>
                        {!subCollapsed && (
                            <>
                                <div className="cf-sidebar-divider" />
                                <div
                                    className={`cf-sidebar-item sub ${selectedSubId === null ? 'active' : ''}`}
                                    onClick={() => setSelectedSubId(null)}
                                >
                                    <span>{t("common.all")}</span>
                                </div>
                                {subCategories.map(cat => (
                                    <div
                                        key={cat.id}
                                        className={`cf-sidebar-item sub ${selectedSubId === cat.id ? 'active' : ''}`}
                                        onClick={() => setSelectedSubId(cat.id === selectedSubId ? null : cat.id)}
                                    >
                                        {cat.iconUrl && <CachedImage src={cat.iconUrl} className="cat-icon" />}
                                        <span>{tCurseForgeTag(t, cat)}</span>
                                    </div>
                                ))}
                            </>
                        )}
                    </>
                )}

                <div className="cf-sidebar-divider" />
                <div className="cf-share-import">
                    <div className="cf-share-title">分享导入</div>
                    <div className="cf-share-hint">Ctrl+V 粘贴分享内容，或点击按钮读取剪贴板</div>
                    <button className="cf-share-btn" onClick={openFromClipboard}>
                        <Clipboard size={16} /> 从剪贴板打开
                    </button>
                </div>
            </div>

            {/* Content */}
            <div className="cf-content-area">
                <div ref={resultsRef} className="cf-results-container custom-scrollbar">
                    <div className="cf-topbar">
                        <div className="cf-topbar-left" title={t("CurseForge.breadcrumb_title")}>
                            <div className="cf-breadcrumb">
                                <span
                                    className={`cf-crumb ${selectedRootId !== null ? 'clickable' : ''}`}
                                    role={selectedRootId !== null ? 'button' : undefined}
                                    tabIndex={selectedRootId !== null ? 0 : undefined}
                                    onClick={() => {
                                        if (selectedRootId === null) return;
                                        setSelectedSubId(null);
                                        resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                    }}
                                    onKeyDown={(e) => {
                                        if (e.key === 'Enter') {
                                            if (selectedRootId === null) return;
                                            setSelectedSubId(null);
                                            resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                        }
                                    }}
                                >
                                    {currentRootLabel}
                                </span>
                                {currentSubLabel && (
                                    <>
                                        <span className="cf-crumb-sep">/</span>
                                        <span
                                            className="cf-crumb clickable"
                                            role="button"
                                            tabIndex={0}
                                            onClick={() => {
                                                resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                            }}
                                            onKeyDown={(e) => {
                                                if (e.key === 'Enter') resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                            }}
                                        >
                                            {currentSubLabel}
                                        </span>
                                    </>
                                )}
                            </div>
                            <span
                                className={`cf-chip ${selectedVersion ? 'clickable' : ''}`}
                                role={selectedVersion ? 'button' : undefined}
                                tabIndex={selectedVersion ? 0 : undefined}
                                onClick={() => {
                                    if (!selectedVersion) return;
                                    setSelectedVersion('');
                                    resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                }}
                                onKeyDown={(e) => {
                                    if (e.key === 'Enter') {
                                        if (!selectedVersion) return;
                                        setSelectedVersion('');
                                        resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                    }
                                }}
                                title={selectedVersion ? t("CurseForge.click_clear_version") : undefined}
                            >
                                {selectedVersion ? selectedVersion : t("CurseForge.all_versions")}
                            </span>
                            <span
                                className={`cf-chip ${currentSort !== 1 ? 'clickable' : ''}`}
                                role={currentSort !== 1 ? 'button' : undefined}
                                tabIndex={currentSort !== 1 ? 0 : undefined}
                                onClick={() => {
                                    if (currentSort === 1) return;
                                    setCurrentSort(1);
                                    resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                }}
                                onKeyDown={(e) => {
                                    if (e.key === 'Enter') {
                                        if (currentSort === 1) return;
                                        setCurrentSort(1);
                                        resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                    }
                                }}
                                title={currentSort !== 1 ? t("CurseForge.click_reset_sort") : undefined}
                            >
                                {currentSortLabel}
                            </span>
                            {deferredSearch && (
                                <button
                                    type="button"
                                    className={`cf-chip cf-search-chip ${onClearSearch ? 'clickable' : ''}`}
                                    onClick={() => {
                                        if (!onClearSearch) return;
                                        onClearSearch();
                                        resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                    }}
                                    onKeyDown={(e) => {
                                        if (e.key === 'Enter') {
                                            if (!onClearSearch) return;
                                            onClearSearch();
                                            resultsRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
                                        }
                                    }}
                                    title={onClearSearch ? t("CurseForge.click_clear_search") : undefined}
                                >
                                    <span className="cf-search-chip-text">“{deferredSearch}”</span>
                                    {onClearSearch && <X size={14} />}
                                </button>
                            )}
                        </div>
                        <div className="cf-topbar-right">
                            {fetching ? (
                                <span className="cf-status fetching">
                                    {t("CurseForge.refreshing")}
                                </span>
                            ) : (
                                <>
                                    {lastDataSource === 'idb' && (
                                        <span className="cf-status cache" title={t("CurseForge.cache_used")}>
                                            {t("CurseForge.cache")}
                                        </span>
                                    )}
                                    {lastUpdatedText && (
                                        <span className="cf-status ok" title={t("CurseForge.updated_at")}>
                                            {t("CurseForge.updated")} {lastUpdatedText}
                                        </span>
                                    )}
                                </>
                            )}
                        </div>
                        <div key={progressSeq} className={`cf-refresh-bar ${fetching ? 'active' : ''}`} />
                    </div>
                    <div className={`cf-results-body ${listAnimOn ? "bm-anim-page-in" : ""}`}>
                        {isLoadingState ? (
                            <ModsSkeleton viewMode={viewMode} />
                        ) : canShowEmptyState ? (
                            <div className="cf-state"><Package size={48} /><p>{t("CurseForge.no_results")}</p></div>
                        ) : (
                            <>
                                {mods.length > 0 && (
                                    <div className={`cf-mod-list ${viewMode}`}>
                                        <AnimatePresence mode='popLayout'>
                                            {mods.map((mod, i) => (
                                                <ModItem
                                                    key={mod.id}
                                                    mod={mod}
                                                    viewMode={viewMode}
                                                    index={i}
                                                    allCategories={allCategories}
                                                    onOpen={navigateToMod}
                                                    onDownload={openDownloadSheet}
                                                />
                                            ))}
                                        </AnimatePresence>
                                    </div>
                                )}
                            </>
                        )}
                    </div>

                    {/* Pagination Controls */}
                    {!fetching && mods.length > 0 && (
                        <div className="cf-pagination">
                            <button
                                className="cf-page-btn"
                                disabled={page === 1}
                                onClick={() => setPage(p => p - 1)}
                            >
                                <ChevronLeft size={16} /> {t("CurseForge.prev_page")}
                            </button>
                            <span className="cf-page-info">{pageInfoText}</span>
                            <div className="cf-page-jump">
                                <span className="jump-label">跳转</span>
                                <input
                                    className="cf-page-input"
                                    type="number"
                                    inputMode="numeric"
                                    min={1}
                                    value={jumpPage}
                                    onChange={(e) => setJumpPage(e.target.value)}
                                    onWheel={(e) => {
                                        (e.currentTarget as HTMLInputElement).blur();
                                    }}
                                    onKeyDown={(e) => { if (e.key === 'Enter') applyJump(); }}
                                    placeholder="页码"
                                />
                                <button className="cf-page-go" onClick={applyJump}>Go</button>
                            </div>
                            <button
                                className="cf-page-btn"
                                disabled={!hasMore}
                                onClick={() => setPage(p => p + 1)}
                            >
                                {t("CurseForge.next_page")} <ChevronRight size={16} />
                            </button>
                        </div>
                    )}
                </div>
            </div>
            <DownloadSheet
                mod={downloadTarget}
                files={downloadFiles}
                loading={downloadsLoading}
                onClose={() => { setDownloadTarget(null); setDownloadFiles([]); }}
                onInstall={(mod, file) => {
                    setDownloadTarget(null);
                    setDownloadFiles([]);
                    setInstallFile(file);
                    setInstallMod(mod);
                    setInstallOpen(true);
                }}
            />

            <CurseForgeInstallModal
                open={installOpen}
                mod={installMod}
                file={installFile}
                onClose={() => { setInstallOpen(false); setInstallFile(null); setInstallMod(null); }}
            />
        </div>
    );
};

// --- Sub Components ---
// Mod Item (Handles both Grid and List view)
const ModItem = ({ mod, viewMode, index, allCategories, onOpen, onDownload }: any) => {
    const { t } = useTranslation();
    const formatCount = (n: number) => {
        if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
        if (n >= 1e3) return (n / 1e3).toFixed(1) + 'K';
        return n;
    };

    // 主分类
    const mainCategory = mod.categories.find((c: any) => c.id === mod.classId) || mod.categories[0];
    const extraCategories = mod.categories
        .filter((c: any) => c.id !== mainCategory?.id)
        .slice(0, 3);

    return (
        <motion.div
            className={`cf-item ${viewMode}`}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.2, delay: index * 0.03 }}
            onClick={() => onOpen(mod.id)}
        >
            <div className="cf-item-icon">
                <img src={mod.logo?.thumbnailUrl || mod.logo?.url} alt={mod.name} loading="lazy" />
            </div>

            <div className="cf-item-content">
                <div className="cf-item-header">
                    <h3 title={mod.name}>{mod.name}</h3>
                </div>
                <p className="cf-item-summary" title={mod.summary}>{mod.summary}</p>

                {viewMode === 'list' && (
                    <div className="cf-item-meta-list">
                        <span className="meta-stat"><User size={12}/> {mod.authors?.[0]?.name || t("common.unknown")}</span>
                        {mainCategory && (
                            <span className="cf-cat-tag">
                                {mainCategory.iconUrl && <CachedImage src={mainCategory.iconUrl} />}
                                {tCurseForgeTag(t, mainCategory)}
                            </span>
                        )}
                        <span className="meta-sep">|</span>
                        <span className="meta-stat"><Download size={12}/> {formatCount(mod.downloadCount)}</span>
                        <span className="meta-stat"><Calendar size={12}/> {new Date(mod.dateModified).toLocaleDateString()}</span>
                    </div>
                )}

                {viewMode === 'list' && extraCategories.length > 0 && (
                    <div className="cf-item-tags">
                        {extraCategories.map((c: any) => (
                            <span key={c.id} className="cf-tag-pill">
                                {c.iconUrl && <CachedImage src={c.iconUrl} />}
                                {tCurseForgeTag(t, c)}
                            </span>
                        ))}
                    </div>
                )}
            </div>

            {viewMode === 'grid' && (
                <div className="cf-item-footer">
                    <span className="meta-stat"><Download size={12}/> {formatCount(mod.downloadCount)}</span>
                    <span className="meta-stat"><Calendar size={12}/> {new Date(mod.dateModified).toLocaleDateString()}</span>
                </div>
            )}

            <div className="cf-item-actions">
                <button
                    className="cf-btn-install-sm"
                    onClick={(e) => { e.stopPropagation(); onDownload(mod); }}
                >
                    <Download size={16} />
                    {viewMode === 'list' && <span>{t("CurseForge.install")}</span>}
                </button>
            </div>
        </motion.div>
    );
};

// Skeleton list/grid
const ModsSkeleton = ({ viewMode }: { viewMode: 'list' | 'grid' }) => {
    const items = Array.from({ length: PAGE_SIZE });
    return (
        <div className={`cf-mod-list ${viewMode}`}>
            {items.map((_, idx) => (
                <div key={idx} className={`cf-skeleton ${viewMode}`}>
                    <div className="sk-thumb" />
                    <div className="sk-lines">
                        <span className="sk-line long" />
                        <span className="sk-line" />
                        <span className="sk-line short" />
                    </div>
                </div>
            ))}
        </div>
    );
};

// Quick download sheet
const DownloadSheet = ({
    mod,
    files,
    loading,
    onClose,
    onInstall
}: {
    mod: Mod | null;
    files: ModFile[];
    loading: boolean;
    onClose: () => void;
    onInstall: (mod: Mod, file: ModFile) => void;
}) => {
    const { t } = useTranslation();
    const formatSize = (bytes: number) => {
        if (!bytes) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const installInApp = (file: ModFile) => {
        if (!mod) return;
        onInstall(mod, file);
    };

    return createPortal(
        <AnimatePresence>
            {mod && (
                <motion.div
                    className="cf-sheet-backdrop"
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    exit={{ opacity: 0 }}
                >
                    <motion.div
                        className="cf-sheet"
                        initial={{ y: 40, opacity: 0 }}
                        animate={{ y: 0, opacity: 1 }}
                        exit={{ y: 20, opacity: 0 }}
                        transition={{ type: 'spring', stiffness: 260, damping: 26 }}
                    >
                        <div className="cf-sheet-head">
                            <div className="cf-sheet-title">
                                <img src={mod.logo?.thumbnailUrl || mod.logo?.url} alt="" />
                                <div>
                                    <p className="sheet-label">{t("CurseForgeMod.tab_files")}</p>
                                    <h3>{mod.name}</h3>
                                </div>
                            </div>
                            <button className="cf-close-btn" onClick={onClose}><X size={16} /></button>
                        </div>

                        <div className="cf-sheet-body custom-scrollbar">
                            {loading ? (
                                <div className="cf-state"><Loader2 size={28} className="spin" /><p>{t("common.loading")}</p></div>
                            ) : files.length === 0 ? (
                                <div className="cf-state"><Package size={40} /><p>{t("CurseForge.no_results")}</p></div>
                            ) : (
                                <div className="cf-file-list">
                                    {files.map(file => (
                                        <div key={file.id} className="cf-file-row">
                                            <div className="file-meta">
                                                <p className="file-name" title={file.displayName}>{file.displayName}</p>
                                                <span className="file-sub">{new Date(file.fileDate).toLocaleDateString()}</span>
                                            </div>
                                            <div className="file-info">
                                                <span className="ver-badge-lite">{file.gameVersions?.find(v => /^\d/.test(v)) || t("common.unknown")}</span>
                                                <span className="file-size">{formatSize(file.fileLength)}</span>
                                                <button className="cf-btn-install-sm solid" onClick={() => installInApp(file)}>
                                                    <Download size={14} /> {t("CurseForge.install")}
                                                </button>
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            )}
                        </div>
                    </motion.div>
                </motion.div>
            )}
        </AnimatePresence>,
        document.body
    );
};
