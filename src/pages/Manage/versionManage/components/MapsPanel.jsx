import React, { useRef, useState, useEffect, useMemo, useCallback } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import Input from '../../../../components/Input.jsx';
import { deleteAsset } from '../../api/assetApi'; // MapsPanel 相对位置： components -> ../api/assetApi
import defaultWorldImg from '../../../../assets/img/minecraft/WorldDemoScreen_Big_Grayscale.png';
import './MapsPanel.css';

export default function MapsPanel({
                                      maps = [],
                                      searchTerms = {},
                                      setSearchTerms,
                                      users = [],
                                      activeUserId,
                                      toast = { info: () => {}, error: () => {} },
                                  }) {
    // search query
    const q = (searchTerms.maps || '').toLowerCase().trim();

    // memoized filtered list
    const filtered = useMemo(() => {
        const src = maps || [];
        if (!q) return src;
        return src.filter(
            (m) =>
                (m.level_name || m.folder_name || '').toLowerCase().includes(q) ||
                String(m.folder_name || '').includes(q)
        );
    }, [maps, q]);

    // virtualization params (must match CSS .list-item height)
    const ITEM_HEIGHT = 96; // px per item — keep in sync with CSS
    const OVERSCAN = 6; // items to render above/below viewport

    const containerRef = useRef(null);
    const [containerHeight, setContainerHeight] = useState(480);
    const [scrollTop, setScrollTop] = useState(0);

    // ResizeObserver keeps containerHeight in sync.
    useEffect(() => {
        const el = containerRef.current;
        if (!el) return;
        const update = () => setContainerHeight(el.clientHeight || 480);
        update();
        const ro = new ResizeObserver(update);
        ro.observe(el);
        return () => ro.disconnect();
    }, []);

    // scroll handler (throttled via rAF)
    const rAF = useRef(null);
    useEffect(() => {
        return () => {
            if (rAF.current) cancelAnimationFrame(rAF.current);
        };
    }, []);

    const onScroll = useCallback((e) => {
        const st = e.currentTarget.scrollTop;
        if (rAF.current) cancelAnimationFrame(rAF.current);
        rAF.current = requestAnimationFrame(() => {
            setScrollTop(st);
        });
    }, []);

    // Reset scroll when filter or activeUser changes
    useEffect(() => {
        if (containerRef.current) {
            containerRef.current.scrollTop = 0;
            setScrollTop(0);
        }
    }, [q, activeUserId]);

    const total = filtered.length;
    const startIndex = Math.max(0, Math.floor(scrollTop / ITEM_HEIGHT) - OVERSCAN);
    const endIndex = Math.min(total, Math.ceil((scrollTop + containerHeight) / ITEM_HEIGHT) + OVERSCAN);
    const visible = filtered.slice(startIndex, endIndex);
    const topSpacer = startIndex * ITEM_HEIGHT;
    const bottomSpacer = Math.max(0, (total - endIndex) * ITEM_HEIGHT);

    const formatIsoToLocal = useCallback((iso) => {
        if (!iso) return '';
        try {
            const d = new Date(iso);
            if (Number.isNaN(d.getTime())) return iso;
            const pad = (n) => String(n).padStart(2, '0');
            return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(
                d.getHours()
            )}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
        } catch (e) {
            return iso;
        }
    }, []);

    const handleOpenPath = useCallback(
        async (path) => {
            try {
                await invoke('open_path', { path });
            } catch (e) {
                console.error('open_path failed', e);
                toast.error('打开世界目录失败: ' + (e?.message || String(e)));
            }
        },
        [toast]
    );

    if (!activeUserId)
        return (
            <div className="panel-empty">未选择用户 — 请先在右上角选择一个 GDK 用户以查看/管理此项。</div>
        );

    return (
        <div className="maps-panel" style={{ height: '100%', minHeight: 0 }}>
            <div className="panel-ops">
                <Input
                    className="search-wrapper"
                    placeholder="搜索 地图..."
                    value={searchTerms.maps}
                    onChange={(e) => setSearchTerms((s) => ({ ...s, maps: e.target.value }))}
                    inputStyle={{ minWidth: 220 }}
                />
                <div className="ops-right">
                    <button className="btn">新建地图</button>
                    <button className="btn">导入地图</button>
                </div>
            </div>

            <div
                className="maps-list scrollable"
                ref={containerRef}
                onScroll={onScroll}
                role="list"
                aria-label="地图列表"
                tabIndex={0}
            >
                {topSpacer > 0 && <div style={{ height: topSpacer }} className="spacer" aria-hidden />}

                {visible.map((m) => {
                    const thumbSrc = m.icon_path && typeof convertFileSrc === 'function' ? convertFileSrc(m.icon_path) : defaultWorldImg;
                    const modified = m.modified ? formatIsoToLocal(m.modified) : null;
                    const key = m.folder_name || `${m.folder_path}-${m.folder_name}`;

                    return (
                        <div key={key} className="list-item" role="listitem">
                            <div className="list-left">
                                <div className="thumb-wrap">
                                    {thumbSrc ? (
                                        <img
                                            src={thumbSrc}
                                            alt={m.level_name || m.folder_name || 'world'}
                                            className="map-thumb"
                                            loading="lazy"
                                            onError={(e) => {
                                                try {
                                                    if (defaultWorldImg) e.currentTarget.src = defaultWorldImg;
                                                    else e.currentTarget.style.display = 'none';
                                                } catch (err) {
                                                    e.currentTarget.style.display = 'none';
                                                }
                                            }}
                                        />
                                    ) : (
                                        <div className="map-thumb fallback">无图标</div>
                                    )}
                                </div>

                                <div className="map-meta">
                                    <div className="map-title" title={m.level_name || m.folder_name}>
                                        {m.level_name || m.folder_name}
                                    </div>
                                    <div className="map-sub">
                                        <div className="map-sub-line map-folder" title={`文件夹: ${m.folder_name}`}>
                                            <span className="value mono">{m.folder_name}</span>
                                        </div>
                                        {(m.size_readable || modified) && (
                                            <div className="map-sub-line map-info" title={`${m.size_readable ? `大小: ${m.size_readable}` : ''}${m.size_readable && modified ? ' · ' : ''}${modified || ''}`}>
                                                {m.size_readable ? <span className="info-item">{m.size_readable}</span> : null}
                                                {m.size_readable && modified ? <span className="sep"> · </span> : null}
                                                {modified ? <span className="info-item">{modified}</span> : null}
                                            </div>
                                        )}
                                    </div>

                                </div>
                            </div>

                            <div className="list-right">
                                <button className="btn" onClick={() => handleOpenPath(m.folder_path)}>
                                    打开目录
                                </button>

                                <button
                                    className="btn btn-danger"
                                    onClick={async () => {
                                        try {
                                            // simple confirm
                                            const ok = window.confirm(`确认删除地图：${m.folder_name} ?  （该操作会删除磁盘上的文件夹）`);
                                            if (!ok) return;

                                            // resolve userId / user_folder from users list
                                            let payloadUserId = null;
                                            try {
                                                const active = users.find(u => u.id === activeUserId);
                                                if (active && active.raw) {
                                                    // prefer numeric user_id if present
                                                    if (active.raw.user_id) payloadUserId = active.raw.user_id;
                                                    else if (active.raw.user_folder) payloadUserId = active.raw.user_folder;
                                                }
                                            } catch (e) {
                                                // swallow
                                            }

                                            // call unified API
                                            await deleteAsset({
                                                kind: normalizedKind || 'gdk',
                                                userId: payloadUserId ?? null,
                                                folder: folder ?? null,
                                                edition: edition ?? null,
                                                deleteType: 'maps',
                                                name: m.folder_name,
                                            });

                                            toast.success('已删除：' + m.folder_name);
                                            // refresh the list (hook 提供的 loadAll)
                                            if (typeof loadAll === 'function') await loadAll();
                                        } catch (e) {
                                            console.error('delete map failed', e);
                                            toast.error('删除失败: ' + (e?.message || String(e)));
                                        }
                                    }}
                                >
                                    删除
                                </button>
                            </div>
                        </div>
                    );
                })}

                {/* bottom spacer */}
                {bottomSpacer > 0 && <div style={{ height: bottomSpacer }} className="spacer" aria-hidden />}

                {filtered.length === 0 && <div className="empty">没有检测到 地图</div>}
            </div>
        </div>
    );
}
