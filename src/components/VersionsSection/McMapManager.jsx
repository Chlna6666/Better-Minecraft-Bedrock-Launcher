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

    // loading 简易显示
    if (loading) return <div className="mc-map-manager-loading">{t('loading')}...</div>;

    return (
        <div className="mc-map-manager">
            <div className="vtoolbar">
                <input
                    type="text"
                    placeholder={t("McPackManager.search") || "Search..."}
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    className="vsearch"
                />
                {search && (
                    <button
                        className="vclear-btn"
                        title={t('common.clear') || 'Clear'}
                        onClick={() => setSearch("")}
                    >
                        ×
                    </button>
                )}
                <button
                    onClick={handleRefresh}
                    className="vrefresh-btn"
                    disabled={refreshing}
                    title={t('common.refresh') || 'Refresh'}
                >
                    {refreshing ? (t('common.refreshing') || 'Refreshing…') : (t('common.refresh') || 'Refresh')}
                </button>
            </div>

            {filteredWorlds.length === 0 ? (
                <div className="empty">
                    {worlds.length === 0 ? t('no_worlds_found') : t('no_search_results') || 'No results found'}
                </div>
            ) : (
                // 使用 div 代替 ul，并添加 role="list" 以保留语义
                <div className="world-list" role="list" aria-label={t('world_list') || 'World list'}>
                    {filteredWorlds.map((w, idx) => {
                        const itemKey = w?.folder_name ?? w?.level_name ?? `world-${idx}`;
                        const label = w?.level_name || w?.folder_name || '';
                        // 选择图片源：优先使用 icon_path（经 convertFileSrc），否则使用默认图
                        const src = w?.icon_path ? convertFileSrc(w.icon_path) : defaultWorldImg;

                        return (
                            // 每项用 div 并标注 role="listitem"，并允许键盘访问
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
                                        // 同 onClick 的行为
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
                                    // 初始 data-fallback 标记为空，onError 会写入 "true"
                                    data-fallback=""
                                    // 为了更好的可访问性，声明 role
                                    role="img"
                                />
                                <div className="world-meta">
                                    <div className="world-name">{label}</div>
                                </div>
                            </div>
                        );
                    })}
                </div>
            )}
        </div>
    );
}

export default McMapManager;
