import React, { useEffect, useState, useMemo } from "react";
import { useTranslation } from 'react-i18next';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import "./McMapManager.css";
import defaultWorldImg from "../../assets/img/minecraft/WorldDemoScreen_Big_Grayscale.png";

function McMapManager() {
    const { t } = useTranslation();
    const [worlds, setWorlds] = useState([]);        // 原始数据
    const [loading, setLoading] = useState(true);
    const [search, setSearch] = useState("");        // 搜索关键字
    const [refreshing, setRefreshing] = useState(false);

    async function fetchWorlds() {
        setLoading(true);
        try {
            const result = await invoke('list_minecraft_worlds_cmd');
            setWorlds(Array.isArray(result) ? result : (result ? [result] : []));
        } catch (err) {
            console.error('Failed to load worlds:', err);
            setWorlds([]);
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        fetchWorlds();
    }, []);

    async function handleRefresh() {
        // 防止重复刷新：当已经在刷新或初次 loading 时禁用
        if (refreshing || loading) return;
        setRefreshing(true);
        try {
            await fetchWorlds();
        } finally {
            setRefreshing(false);
        }
    }

    const filteredWorlds = useMemo(() => {
        const q = search.trim().toLowerCase();
        if (!q) return worlds;
        return worlds.filter(w => {
            const name = (w?.level_name || "").toString().toLowerCase();
            const folder = (w?.folder_name || "").toString().toLowerCase();
            return name.includes(q) || folder.includes(q);
        });
    }, [worlds, search]);

    // 图片出错时使用默认图片，并避免无限循环（使用 data-fallback 标记）
    const handleImgError = (e) => {
        const img = e.currentTarget;
        if (img.dataset.fallback) return; // 已回退过，避免循环
        img.dataset.fallback = "true";
        img.src = defaultWorldImg;
    };

    return (
        <div className="mc-map-manager">
            {/* toolbar 始终渲染 */}
            <div className="vtoolbar">
                <input
                    type="text"
                    placeholder={t("McPackManager.search")}
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    className="vsearch"
                />
                <button onClick={handleRefresh} className="vrefresh-btn">
                    {t('common.refresh')}
                </button>
            </div>

            {/* loading 区放在 toolbar 之后 */}
            {loading ? (
                <div className="mc-map-manager-loading" role="status" aria-live="polite">
                    {t('loading') || 'Loading...'}
                </div>
            ) : (
                // 非 loading 时显示列表/空状态
                <>
                    {filteredWorlds.length === 0 ? (
                        <div className="empty">
                            {worlds.length === 0 ? t('no_worlds_found') : t('no_search_results') || 'No results found'}
                        </div>
                    ) : (
                        <div className="world-list" role="list" aria-label={t('world_list') || 'World list'}>
                            {filteredWorlds.map((w, idx) => {
                                const itemKey = w?.folder_name ?? w?.level_name ?? `world-${idx}`;
                                const label = w?.level_name || w?.folder_name || '';
                                const src = w?.icon_path ? convertFileSrc(w.icon_path) : defaultWorldImg;

                                // Packs count 和 size 回退显示
                                const behaviorCount = (w?.behavior_packs_count ?? (Array.isArray(w?.behavior_packs) ? w.behavior_packs.length : 0));
                                const resourceCount = (w?.resource_packs_count ?? (Array.isArray(w?.resource_packs) ? w.resource_packs.length : 0));
                                const sizeReadable = w?.size_readable ?? (w?.size_bytes ? `${w.size_bytes} B` : t('unknown') || 'Unknown');

                                return (
                                    <div
                                        key={itemKey}
                                        className="world-item"
                                        role="listitem"
                                        tabIndex={0}
                                        onClick={() => {
                                            console.log("clicked world:", w?.folder_name);
                                        }}
                                        onKeyDown={(e) => {
                                            if (e.key === 'Enter' || e.key === ' ') {
                                                e.preventDefault();
                                                console.log("activated world (keyboard):", w?.folder_name);
                                            }
                                        }}
                                        aria-label={label}
                                    >
                                        <img
                                            src={src}
                                            alt={label || t('world') || 'World'}
                                            onError={handleImgError}
                                            loading="lazy"
                                            data-fallback=""
                                            role="img"
                                        />
                                        <div className="world-meta">
                                            <div className="world-name">{label}</div>

                                            {/* size 总是显示 */}
                                            <div className="world-stats" aria-hidden="false">
                                                <span className="stat stat-size">
                                                    {sizeReadable}
                                                </span>

                                                {/* 仅当有行为包时显示 */}
                                                {behaviorCount > 0 && (
                                                    <span className="stat stat-behavior">
                                                        {`${behaviorCount} ${t('McPackManager.behaviorPacks')}`}
                                                    </span>
                                                )}

                                                {/* 仅当有资源包时显示 */}
                                                {resourceCount > 0 && (
                                                    <span className="stat stat-resource">
                                                        {`${resourceCount} ${t('McPackManager.resourcePacks')}`}
                                                    </span>
                                                )}
                                            </div>
                                        </div>
                                    </div>
                                );
                            })}
                        </div>
                    )}
                </>
            )}
        </div>
    );
}

export default McMapManager;
