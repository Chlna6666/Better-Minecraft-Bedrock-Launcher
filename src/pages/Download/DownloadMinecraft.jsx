import React, { useEffect, useRef, useState, useCallback } from "react";
import { useTranslation } from 'react-i18next';
import { open } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import releaseIcon from "../../assets/img/minecraft/Release.png";
import previewIcon from "../../assets/img/minecraft/Preview.png";
import unknownIcon from "../../assets/feather/box.svg";
import downloadIcon from "../../assets/feather/download.svg";
import uploadIcon from "../../assets/feather/upload.svg";
import "./DownloadMinecraft.css";
import InstallProgressBar from "./InstallProgressBar.jsx";
import { getConfig } from "../../utils/config.jsx";
import {Input, Select} from "../../components/index.js";
import "../Manage/VersionManager.css";
import {invoke} from "@tauri-apps/api/core";
import { useToast } from "../../components/Toast.tsx"; // 如有路径差异请调整
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
    const toast = useToast();
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
    // localStorage 缓存设置
    const CACHE_KEY = "appx_api_cache";
    const CACHE_TTL = 1000 * 60 * 60 * 12; // 12 小时
    // 读取本地缓存（如果存在且未过期）
    const loadCacheFromLocalStorage = () => {
        try {
            const raw = window.localStorage.getItem(CACHE_KEY);
            if (!raw) return null;
            const obj = JSON.parse(raw);
            if (!obj || !obj.ts) return null;
            if (Date.now() - obj.ts > (obj.ttl || CACHE_TTL)) {
                // 过期
                window.localStorage.removeItem(CACHE_KEY);
                return null;
            }
            return obj;
        } catch (e) {
            console.warn("[loadCacheFromLocalStorage] 读取缓存失败", e);
            return null;
        }
    };
    // 保存缓存：增加 rawCreationTime 字段（如果有）
    const saveCacheToLocalStorage = (rawBody, parsed, rawCreationTime = null) => {
        try {
            const obj = {
                ts: Date.now(),
                raw: rawBody,
                parsed,
                ttl: CACHE_TTL,
                rawCreationTime: rawCreationTime || null
            };
            window.localStorage.setItem(CACHE_KEY, JSON.stringify(obj));
        } catch (e) {
            console.warn("[saveCacheToLocalStorage] 写入缓存失败", e);
        }
    };
    function mapArchivalToLabel(code) {
        const key = ["0", "1", "2", "3"].includes(String(code)) ? String(code) : "unknown";
        return t(`DownloadMinecraft.archival_status.${key}`);
    }
    // 初始化：先尝试从 localStorage 加载缓存，减少网络请求（页面初始渲染）
    useEffect(() => {
        const cache = loadCacheFromLocalStorage();
        if (cache && Array.isArray(cache.parsed) && cache.parsed.length > 0) {
            cachedVersions.current = cache.parsed;
            setVersions(cache.parsed);
            toast.info(t('DownloadMinecraft.cache_used_on_init'));
        }
        // 触发一次网络拉取（非强制，会在有缓存时直接使用缓存）
        fetchVersions(false);
        // eslint-disable-next-line react-hooks/exhaustive-deps
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
        // 如果不强制并且有内存缓存，直接用缓存
        if (!forceRefresh && cachedVersions.current) {
            setVersions(cachedVersions.current);
            return;
        }
        if (fetchLockRef.current) {
            console.log("[fetchVersions] 正在进行中，忽略并发请求");
            return;
        }
        fetchLockRef.current = true;
        setLoading(true);
        setError(null);
        // 读取本地缓存以便和 API CreationTime 对比
        const localCache = loadCacheFromLocalStorage();
        try {
            const config = await getConfig().catch(() => ({}));
            const api = (config && config.launcher && config.launcher.custom_appx_api) || "https://data.mcappx.com/v2/bedrock.json";
            const defaultUA = (config && config.launcher && config.launcher.user_agent) || "BMCBL";
            const allowedHosts = [];
            try {
                const u = new URL(api);
                if (u.hostname) allowedHosts.push(u.hostname);
            } catch (e) {
            }
            const backendOptions = {
                method: "GET",
                headers: { "User-Agent": defaultUA, "Accept": "application/json, text/*" },
                timeout_ms: 20000,
                allow_redirects: true,
                allowed_hosts: allowedHosts.length ? allowedHosts : undefined,
            };
            const backendRes = await invoke("fetch_remote", { url: api, options: backendOptions });
            if (!backendRes || !backendRes.body) {
                throw new Error("empty response from backend");
            }
            let data;
            try {
                data = JSON.parse(backendRes.body);
            } catch (e) {
                throw new Error("invalid json from backend: " + (e.message || e));
            }
            // 取 CreationTime（若有）
            const apiCreationTimeRaw = (data && data.CreationTime) ? String(data.CreationTime) : null;
            let apiCreationTs = null;
            if (apiCreationTimeRaw) {
                const parsedTs = Date.parse(apiCreationTimeRaw);
                if (!isNaN(parsedTs)) apiCreationTs = parsedTs;
            }
            // 如果本地有缓存，并且 API 提供了 CreationTime，且 API 的 CreationTime <= 本地缓存的 rawCreationTime，
            // 则认为远端数据并不新，直接使用本地缓存（避免覆盖更新的缓存）
            if (!forceRefresh && localCache && localCache.rawCreationTime && apiCreationTs) {
                const localCreationTs = Date.parse(localCache.rawCreationTime);
                if (!isNaN(localCreationTs) && apiCreationTs <= localCreationTs) {
                    // 使用本地缓存并提前返回
                    cachedVersions.current = localCache.parsed || [];
                    setVersions(localCache.parsed || []);
                    toast.info(t('DownloadMinecraft.using_cache_when_api_not_newer'));
                    setLoading(false);
                    fetchLockRef.current = false;
                    return;
                }
            }
            // 找到包含版本对象的字段
            let src = null;
            if (data && typeof data === 'object') {
                const keys = Object.keys(data);
                for (const key of keys) {
                    const val = data[key];
                    if (val && typeof val === 'object') {
                        src = val;
                        break;
                    }
                }
            }
            src = src || data;
            // 现在 parsed 每项改为数组结构：
            // [ versionKey, packageId, typeNum, typeStr, buildType, archivalStatus, metaPresent ]
            const parsed = [];
            Object.entries(src).forEach(([versionKey, item]) => {
                try {
                    if (!item || typeof item !== "object") return;
                    // 优先使用 item.Type 字符串（如果存在）
                    const typeStr = (item.Type && String(item.Type).trim()) || "";
                    const typeMap = { "Release": 0, "Beta": 1, "Preview": 2 };
                    let typeNum = typeMap[typeStr];
                    // 从 Variations[*].ArchivalStatus 回退映射
                    if (typeNum === undefined) {
                        let archival = undefined;
                        if (Array.isArray(item.Variations) && item.Variations.length > 0) {
                            for (const v of item.Variations) {
                                if (v && (v.ArchivalStatus !== undefined && v.ArchivalStatus !== null)) {
                                    archival = Number(v.ArchivalStatus);
                                    break;
                                }
                            }
                        }
                        if (archival !== undefined) {
                            if (archival === 3) typeNum = 0;
                            else if (archival === 2) typeNum = 1;
                            else typeNum = 2;
                        } else {
                            typeNum = 2;
                        }
                    }
                    // **只选择 x64 架构**（如果没找到 x64 则跳过该版本）
                    let chosenVar = null;
                    if (Array.isArray(item.Variations) && item.Variations.length > 0) {
                        chosenVar = item.Variations.find(v => String(v.Arch).toLowerCase() === "x64");
                        if (!chosenVar) return; // skip
                    } else {
                        return; // skip
                    }
                    // 拿 packageId（以前的逻辑）
                    let packageId = "";
                    if (chosenVar) {
                        if (Array.isArray(chosenVar.MetaData) && chosenVar.MetaData.length > 0) {
                            packageId = chosenVar.MetaData[0] || "";
                        } else if (chosenVar.MetaData && typeof chosenVar.MetaData === "string") {
                            packageId = chosenVar.MetaData;
                        } else if (chosenVar.MD5) {
                            packageId = chosenVar.MD5;
                        }
                    }
                    if (!packageId && item.ID) packageId = item.ID;
                    packageId = packageId || "";
                    // BuildType 可能为 "GDK" 或 "UWP"
                    const buildType = item.BuildType ? String(item.BuildType) : "";
                    // ArchivalStatus 优先取 chosenVar 的字段
                    const archivalStatus = (chosenVar && (chosenVar.ArchivalStatus !== undefined && chosenVar.ArchivalStatus !== null))
                        ? Number(chosenVar.ArchivalStatus)
                        : (item.ArchivalStatus !== undefined ? Number(item.ArchivalStatus) : null);
                    // metaPresent 表示 Variations.MetaData 存在且第一个元素非空
                    const metaPresent = !!(
                        (chosenVar && Array.isArray(chosenVar.MetaData) && chosenVar.MetaData.length > 0 && String(chosenVar.MetaData[0]).trim() !== "")
                        || (chosenVar && chosenVar.MetaData && typeof chosenVar.MetaData === "string" && String(chosenVar.MetaData).trim() !== "")
                    );
                    const md5Value = (chosenVar && chosenVar.MD5) ? String(chosenVar.MD5) : null;
                    parsed.push([versionKey, packageId, typeNum, typeStr, buildType, archivalStatus, metaPresent, md5Value]);
                } catch (e) {
                    console.warn("[fetchVersions] 解析单个版本失败", versionKey, e);
                }
            });
            parsed.sort((a, b) => compareVersion(b[0], a[0]));
            // 设置并缓存到 localStorage，保存 apiCreationTimeRaw 便于下次比较
            setVersions(parsed);
            cachedVersions.current = parsed;
            saveCacheToLocalStorage(backendRes.body, parsed, apiCreationTimeRaw);
            console.log("[fetchVersions] v2 数据解析完成，版本数：", parsed.length);
            toast.success(t('DownloadMinecraft.fetch_success'));
        } catch (e) {
            console.error("[fetchVersions] 拉取或解析版本数据失败：", e);
            setError(e.message || String(e));
            // 尝试从 localStorage 回退
            const cache = loadCacheFromLocalStorage();
            if (cache && Array.isArray(cache.parsed) && cache.parsed.length > 0) {
                cachedVersions.current = cache.parsed;
                setVersions(cache.parsed);
                toast.info(t('DownloadMinecraft.using_cache_fallback'));
            } else {
                setVersions([]);
                cachedVersions.current = [];
                toast.error(t('DownloadMinecraft.fetch_failed', { message: e.message || e }));
            }
        } finally {
            setLoading(false);
            fetchLockRef.current = false;
        }
    };
    // 无限滚动：接近底部增加 displayCount
    const handleScrollBottom = useCallback(() => {
        const c = containerRef.current;
        if (!c || isDownloading) return;
        if (c.scrollTop + c.clientHeight >= c.scrollHeight - 50) {
            setDisplayCount((prev) => prev + 20);
        }
    }, [isDownloading]);
    useEffect(() => {
        const c = containerRef.current;
        if (!c) return;
        c.addEventListener("scroll", handleScrollBottom);
        return () => c.removeEventListener("scroll", handleScrollBottom);
    }, [handleScrollBottom]);
    // 滚动处理：使用 throttle 减少 setScrollTop 调用频率，降低 re-render
    const throttle = (func, delay) => {
        let lastCall = 0;
        return (...args) => {
            const now = Date.now();
            if (now - lastCall >= delay) {
                lastCall = now;
                func(...args);
            }
        };
    };
    const handleScroll = useCallback(throttle(() => {
        const c = containerRef.current;
        if (!c || isDownloading) return;
        setScrollTop(c.scrollTop);
    }, 50), [isDownloading]); // 50ms throttle
    useEffect(() => {
        const c = containerRef.current;
        if (!c) return;
        c.addEventListener("scroll", handleScroll);
        return () => c.removeEventListener("scroll", handleScroll);
    }, [handleScroll]);
    // 修复 passive listener 错误：给容器添加非被动 wheel listener
    useEffect(() => {
        const el = containerRef.current;
        if (!el) return;
        const onWheel = (e) => {
            if (isDownloading) e.preventDefault();
        };
        el.addEventListener("wheel", onWheel, { passive: false });
        return () => el.removeEventListener("wheel", onWheel);
    }, [isDownloading]);
    const isDownloadingRef = useRef(isDownloading);
    const activeDownloadRef = useRef(activeDownload);
    const tRef = useRef(t);
    const toastRef = useRef(toast);
    useEffect(() => {
        isDownloadingRef.current = isDownloading;
    }, [isDownloading]);
    useEffect(() => {
        activeDownloadRef.current = activeDownload;
    }, [activeDownload]);
    useEffect(() => {
        tRef.current = t;
    }, [t]);
    useEffect(() => {
        toastRef.current = toast;
    }, [toast]);
    useEffect(() => {
        let cleaned = false;
        const unlisteners = [];

        console.log("[DragEvents] useEffect mounted");

        (async () => {
            try {
                console.log("[DragEvents] registering listeners...");

                const uEnter = await listen('tauri://drag-enter', (event) => {
                    console.log("[DragEvents] drag-enter event");
                    if (!isDownloadingRef.current && !activeDownloadRef.current) {
                        setDragOver(true);
                    }
                });
                if (cleaned) {
                    console.log("[DragEvents] component already cleaned — executing uEnter()");
                    uEnter();
                    return;
                }
                console.log("[DragEvents] registered drag-enter");
                unlisteners.push(uEnter);

                const uOver = await listen('tauri://drag-over', (event) => {
                    console.log("[DragEvents] drag-over event");
                    if (!isDownloadingRef.current && !activeDownloadRef.current) {
                        setDragOver(true);
                    }
                });
                if (cleaned) {
                    console.log("[DragEvents] component already cleaned — executing uOver()");
                    uOver();
                    return;
                }
                console.log("[DragEvents] registered drag-over");
                unlisteners.push(uOver);

                const uLeave = await listen('tauri://drag-leave', () => {
                    console.log("[DragEvents] drag-leave event");
                    setDragOver(false);
                });
                if (cleaned) {
                    console.log("[DragEvents] component already cleaned — executing uLeave()");
                    uLeave();
                    return;
                }
                console.log("[DragEvents] registered drag-leave");
                unlisteners.push(uLeave);

                const uDrop = await listen('tauri://drag-drop', (event) => {
                    console.log("[DragEvents] drag-drop event", event.payload);
                    setDragOver(false);

                    if (isDownloadingRef.current || activeDownloadRef.current) {
                        console.log("[DragEvents] drop ignored due to downloading");
                        return;
                    }

                    const paths = event.payload?.paths;
                    if (paths && paths.length > 0) {
                        const path = paths[0];
                        const ext = path.toLowerCase().split('.').pop();
                        if (ext === 'appx' || ext === 'zip') {
                            console.log("[DragEvents] accepted drop:", path);
                            setSourcePath(path);
                            setIsImporting(true);
                            setActiveDownload('import');
                        } else {
                            console.log("[DragEvents] drop rejected, unsupported ext:", ext);
                            toastRef.current.error(tRef.current('DownloadMinecraft.import_unsupported'));
                        }
                    } else {
                        console.warn("[DragEvents] No paths in drop event");
                    }
                });
                if (cleaned) {
                    console.log("[DragEvents] component already cleaned — executing uDrop()");
                    uDrop();
                    return;
                }
                console.log("[DragEvents] registered drag-drop");
                unlisteners.push(uDrop);

                console.log("[DragEvents] all listeners registered");
            } catch (e) {
                console.warn("[DragEvents] setup failed", e);
            }
        })();

        return () => {
            console.log("[DragEvents] useEffect cleanup start");
            cleaned = true;

            for (const u of unlisteners) {
                try {
                    console.log("[DragEvents] calling unlisten function");
                    u && u();
                } catch (err) {
                    console.warn("[DragEvents] unlisten failed", err);
                }
            }
            console.log("[DragEvents] useEffect cleanup finished");
        };
    }, []);

    // 排序与过滤
    const sorted = React.useMemo(
        () => [...versions].sort((a, b) => compareVersion(b[0], a[0])),
        [versions]
    );
    const filterOptions = [
        { value: "all", label: t('common.all_versions') || 'All' },
        { value: "release", label: t('common.release') || 'Release' },
        { value: "beta", label: t('common.beta') || 'Beta' },
        { value: "preview", label: t('common.preview') || 'Preview' },
    ];
    const [filter, setFilter] = useState("all");
    const reload = () => { if (!loading) fetchVersions(true); };
    // 修改 filtered 的过滤函数以支持 filter Select
    const filtered = React.useMemo(() => {
        const showType = (type) => {
            if (filter === "all") {
                return type === 0 ? showRelease : type === 1 ? showBeta : showPreview;
            }
            if (filter === "release") return type === 0;
            if (filter === "beta") return type === 1;
            if (filter === "preview") return type === 2;
            return true;
        };
        const trimmedSearch = searchTerm.trim();
        return sorted
            .filter(([v, , t]) => showType(t) && (!trimmedSearch || String(v).includes(trimmedSearch)))
            .slice(0, displayCount);
    }, [sorted, showRelease, showBeta, showPreview, searchTerm, displayCount, filter]);
    // 虚拟列表计算
    const [containerHeight, setContainerHeight] = useState(0);
    // 使用 ResizeObserver 监听容器高度变化，减少 render 时计算
    useEffect(() => {
        const c = containerRef.current;
        if (!c) return;
        const resizeObserver = new ResizeObserver(() => {
            setContainerHeight(c.clientHeight);
        });
        resizeObserver.observe(c);
        return () => resizeObserver.unobserve(c);
    }, []);
    const totalCount = filtered.length;
    const visibleCount = containerHeight ? Math.ceil(containerHeight / ROW_HEIGHT) : 10;
    const startIndex = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
    const endIndex = Math.min(totalCount, startIndex + visibleCount + OVERSCAN * 2);
    const topPadding = startIndex * ROW_HEIGHT;
    const bottomPadding = (totalCount - endIndex) * ROW_HEIGHT;
    const visibleList = React.useMemo(() => filtered.slice(startIndex, endIndex), [filtered, startIndex, endIndex]);
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
    // 点击下载：现在接收额外元信息用于校验
    const handleDownloadClick = (pkgId, buildType, archivalStatus, metaPresent, md5) => {
        if (isDownloading || activeDownload) return;
        // 检查是否被短期取消
        const ts = cancelledRef.current.get(pkgId);
        if (ts && Date.now() - ts < CANCEL_BLOCK_MS) {
            console.log("[handleDownloadClick] 点击被忽略：刚被取消，等待冷却", pkgId);
            return;
        }
        if (ts) cancelledRef.current.delete(pkgId);
        const isGDK = buildType && String(buildType).toLowerCase() === "gdk";
        if (isGDK) {
            toast.info(t('DownloadMinecraft.gdk_not_supported'));
            return;
        }
        if (!metaPresent) {
            toast.error(t('DownloadMinecraft.no_metadata'));
            return;
        }
        // ArchivalStatus 逻辑：允许下载的状态这里设为 2 或 3（3 最安全，2 可能不可用）
        if (archivalStatus === null || archivalStatus === undefined) {
            // 未知状态，提示但允许（如果你想更严格可以把它禁止）
            toast.info(t('DownloadMinecraft.archival_unknown'));
        } else if (archivalStatus === 1 || archivalStatus === 0) {
            toast.error(t('DownloadMinecraft.archival_not_available'));
            return;
        } else if (archivalStatus === 2) {
            // 允许但提醒
            toast.info(t('DownloadMinecraft.archival_maybe_unavailable'));
        }
        // 通过校验：启动下载
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
    return (
        <div
            className={`container ${dragOver ? 'drag-over' : ''}`}
            ref={containerRef}
        >
            {/* 操作区域 */}
            <div
                className="vtoolbar"
                style={{
                    pointerEvents: (isDownloading || !!activeDownload) ? "none" : "auto",
                    opacity: (isDownloading || !!activeDownload) ? 0.5 : 1,
                    display: 'flex',
                    gap: 8,
                    alignItems: 'center',
                    padding: '8px',
                }}
            >
                <Input
                    type="text"
                    placeholder={t('common.search_placeholder') || 'Search...'}
                    value={searchTerm}
                    onChange={(e) => setSearchTerm(e.target.value)}
                    style={{ flex: 1 }}
                    inputStyle={{ height: '29px' }}
                />
                <Select
                    size={13}
                    value={filter}
                    onChange={(val) => setFilter(val)}
                    options={filterOptions}
                    placeholder={t('common.all_versions')}
                />
                <button onClick={reload} className="vrefresh-btn" title={t('common.refresh')}>
                    {t('common.refresh')}
                </button>
                <div className="action-btn" onClick={handleImportClick} title={t('DownloadMinecraft.import')}>
                    <img src={uploadIcon} alt="Import" />
                </div>
            </div>
            {/* 版本列表 */}
            <div className="table">
                <div style={{ paddingTop: topPadding, paddingBottom: bottomPadding }}>
                    {visibleList.map(([version, pkgId, type, typeStr, buildType, archivalStatus, metaPresent, md5],idx) => {
                        const key = pkgId || version || idx;
                        const isGDK = buildType && String(buildType).toLowerCase() === "gdk";
                        // ArchivalStatus: 3 可下载；2 可能不可用；1/0 不可下载
                        const archivalLabel = mapArchivalToLabel(archivalStatus);
                        const canDownload = !isGDK && metaPresent && (archivalStatus === 3 || archivalStatus === 2);
                        // tooltip reason when disabled
                        let disabledReason = null;
                        if (isGDK) disabledReason = t('DownloadMinecraft.gdk_not_supported');
                        else if (!metaPresent) disabledReason = t('DownloadMinecraft.no_metadata');
                        else if (archivalStatus === 1 || archivalStatus === 0) disabledReason = t('DownloadMinecraft.archival_not_available');
                        return (
                            <div key={key} className="table-row" style={{ height: ROW_HEIGHT }}>
                                <div className="table-icon-cell">
                                    <img src={getVersionIconByType(type)} alt="Version" className="version-icon" />
                                </div>
                                <div className="table-cell">
                                    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                                        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                                            <div className="table-version-number">{version}</div>
                                        </div>
                                    </div>
                                    <div className="table-version-type">
                                        {mapVersionTypeToLabel(type)}
                                        {isGDK && (
                                            <span className="build-note" title="GDK: 暂不支持安装">· 暂不支持GDK</span>
                                        )}
                                        {(archivalStatus !== null && archivalStatus !== undefined && archivalStatus !== 3) ? (
                                            <span title={`ArchivalStatus: ${archivalStatus}`}>· {archivalLabel}</span>
                                        ) : null}
                                    </div>
                                </div>
                                <div className="table-download-cell">
                                    {activeDownload === pkgId ? (
                                        <InstallProgressBar
                                            key={pkgId}
                                            version={version}
                                            packageId={pkgId}
                                            versionType={type}
                                            md5={md5}
                                            onStatusChange={setIsDownloading}
                                            onCompleted={(id) => handleChildCompleted(id)}
                                            onCancel={(id) => handleChildCancel(id)}
                                        >
                                            <button className="download-button" disabled title={t('DownloadMinecraft.downloading')}>
                                                <img src={downloadIcon} alt="Download" className="download-icon" />
                                            </button>
                                        </InstallProgressBar>
                                    ) : (
                                        <button
                                            className="download-button"
                                            onClick={() => handleDownloadClick(pkgId, buildType, archivalStatus, metaPresent, md5)}
                                            disabled={isDownloading || !!activeDownload || !canDownload}
                                            title={(!canDownload && disabledReason) ? disabledReason : t('DownloadMinecraft.download')}
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