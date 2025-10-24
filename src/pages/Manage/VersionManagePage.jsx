import React, { useEffect, useState, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { open } from '@tauri-apps/plugin-dialog';
import {useToast} from "../../components/Toast.jsx";

/**
 * VersionManagePage - fixed autosave bug (use modsRef for stale-closure issue)
 */
export default function VersionManagePage({
                                              folder: initialFolder = null,
                                              path: initialPath = null,
                                              onDone = null,
                                              onClose = null,
                                          }) {
    const { t } = useTranslation();
    const toast = useToast();

    const [folder, setFolder] = useState(initialFolder);
    const [path, setPath] = useState(initialPath);
    const [loading, setLoading] = useState(false);
    const [mods, setMods] = useState([]); // { id, name, enabled, delay, path }
    const [dirty, setDirty] = useState(false);

    const [confirmOpen, setConfirmOpen] = useState(false);
    const [confirmPayload, setConfirmPayload] = useState(null);

    const [searchTerm, setSearchTerm] = useState("");

    const [selectedIds, setSelectedIds] = useState(new Set());
    const [multiMode, setMultiMode] = useState(false);

    // debounced save timers
    const saveTimersRef = useRef(new Map());

    const selectedCount = confirmPayload?.ids?.length ?? 0;


    // important: keep a ref to latest mods to avoid stale closures inside setTimeout callbacks
    const modsRef = useRef(mods);
    useEffect(() => { modsRef.current = mods; }, [mods]);

    // sync props -> state
    useEffect(() => setFolder(initialFolder), [initialFolder]);
    useEffect(() => setPath(initialPath), [initialPath]);
    useEffect(() => { if (folder) fetchMods(folder); }, [folder]);

    // cleanup timers on unmount
    useEffect(() => {
        return () => {
            for (const t of saveTimersRef.current.values()) clearTimeout(t);
            saveTimersRef.current.clear();
        };
    }, []);

    function showToast(text, type = "info", duration = 3000) {
        if (type === "success") toast.success(text, { duration });
        else if (type === "error") toast.error(text, { duration });
        else toast.info(text, { duration });
    }

    // ------------------- Fetch / Open -------------------
    async function fetchMods(folderName) {
        setLoading(true);
        try {
            const list = await invoke("get_mod_list", { folderName });
            setMods(Array.isArray(list) ? list : []);
        } catch (e) {
            console.error(e);
            showToast((t("common.load_failed") || "加载失败") + ": " + (e?.message || String(e)), "error", 5000);
        } finally {
            setLoading(false);
        }
    }

    async function openFolder() {
        if (!path) {
            showToast(t("common.no_path") || "没有路径", "error");
            return;
        }
        try {
            await invoke("open_path", { path });
        } catch (e) {
            console.error("open failed:", e);
            showToast((t("common.open_failed") || "打开失败") + ": " + (e?.message || String(e)), "error");
        }
    }

    // ------------------- Unified remote save (single endpoint) -------------------
    function scheduleSave(modId, delayMs = 800) {
        const timers = saveTimersRef.current;
        if (timers.has(modId)) clearTimeout(timers.get(modId));

        const id = setTimeout(async () => {
            timers.delete(modId);
            await saveSingle(modId);
            if (timers.size === 0) setDirty(false);
        }, delayMs);

        timers.set(modId, id);
        setDirty(true);
    }

    async function saveSingle(modId) {
        // IMPORTANT: read from modsRef.current to get latest state (avoid stale closure)
        const currentMods = modsRef.current || [];
        const mod = currentMods.find(m => m.id === modId);
        if (!mod || !folder) return;
        try {
            // merged endpoint: set_mod
            await invoke("set_mod", { folderName: folder, modId: mod.id, enabled: !!mod.enabled, delay: mod.delay ?? 0 });
            showToast(t("common.save_success") || "保存成功", "success");
        } catch (e) {
            console.error("saveSingle failed:", e);
            showToast((t("common.save_failed") || "保存失败") + ": " + (e?.message || String(e)), "error", 4000);
        }
    }

    // flush all pending timers and write all current mods to backend
    async function flushAllSaves() {
        const timers = saveTimersRef.current;
        for (const t of timers.values()) clearTimeout(t);
        timers.clear();

        if (!folder) return;
        setLoading(true);
        try {
            // Use modsRef.current to ensure latest values
            const currentMods = modsRef.current || [];
            for (const m of currentMods) {
                await invoke("set_mod", { folderName: folder, modId: m.id, enabled: !!m.enabled, delay: m.delay ?? 0 });
            }
            showToast(t("common.save_success") || "保存成功", "success");
            setDirty(false);
        } catch (e) {
            console.error("flushAllSaves failed:", e);
            showToast((t("common.save_failed") || "保存失败") + ": " + (e?.message || String(e)), "error", 4000);
        } finally {
            setLoading(false);
        }
    }

    // ------------------- UI-triggered changes -------------------
    function toggleMod(modId) {
        // optimistic update
        setMods(prev => prev.map(m => m.id === modId ? { ...m, enabled: !m.enabled } : m));
        // schedule save (will use modsRef when fired)
        scheduleSave(modId);
    }

    function setDelay(modId, value) {
        const v = Math.max(0, parseInt(value || "0", 10) || 0);
        setMods(prev => prev.map(m => m.id === modId ? { ...m, delay: v } : m));
        scheduleSave(modId, 800);
    }

    // ------------------- Import / Open Mods Folder / Refresh -------------------
    async function handleImportMods() {
        if (!folder) return;
        try {
            const paths = await open({ multiple: true, filters: [{ name: 'DLL', extensions: ['dll'] }] });
            if (!paths) return;
            const arr = Array.isArray(paths) ? paths : [paths];
            await invoke("import_mods", { folderName: folder, paths: arr });
            showToast(t('manage.import_success') || '导入完成', 'success');
            fetchMods(folder);
        } catch (e) {
            console.error(e);
            showToast((t('manage.import_failed') || '导入失败') + ': ' + (e?.message || String(e)), 'error');
        }
    }

    async function openModsFolder() {
        if (!path) {
            showToast(t("common.no_path") || "没有路径", "error");
            return;
        }
        const base = String(path).replace(/[\\/]+$/, "");
        const sep = base.includes("\\") ? "\\" : "/";
        const modsPath = `${base}${sep}mods`;
        try {
            await invoke("open_path", { path: modsPath });
        } catch (e) {
            console.warn("open mods folder failed, fallback to base path:", e);
            try {
                await invoke("open_path", { path: base });
                showToast(t("manage.open_mods_folder") || "打开 Mod 文件夹", "success");
            } catch (err) {
                console.error(err);
                showToast((t("common.open_failed") || "打开失败") + ": " + (err?.message || String(err)), "error");
            }
        }
    }

    function handleRefresh() {
        if (folder) fetchMods(folder);
    }

    // ------------------- Multi-select -------------------
    function toggleSelection(modId) {
        setSelectedIds(prev => {
            const s = new Set(prev);
            if (s.has(modId)) s.delete(modId);
            else s.add(modId);
            return s;
        });
        setMultiMode(true);
    }

    function handleContextMenu(e, modId) {
        e.preventDefault();
        toggleSelection(modId);
    }

    function clearSelection() {
        setSelectedIds(new Set());
        setMultiMode(false);
    }

    async function enableSelected() {
        if (!folder) return;
        const ids = Array.from(selectedIds);
        if (!ids.length) return;
        try {
            // use set_mod for each id
            await Promise.all(ids.map(id => {
                const d = (modsRef.current?.find(m => m.id === id)?.delay ?? 0);
                return invoke('set_mod', { folderName: folder, modId: id, enabled: true, delay: d });
            }));
            setMods(prev => prev.map(m => ids.includes(m.id) ? { ...m, enabled: true } : m));
            showToast(t('manage.enable_selected_success') || '启用成功', 'success');
            clearSelection();
        } catch (e) {
            console.error(e);
            showToast((t('manage.enable_selected_failed') || '启用失败') + ': ' + (e?.message || String(e)), 'error');
        }
    }

    async function disableSelected() {
        if (!folder) return;
        const ids = Array.from(selectedIds);
        if (!ids.length) return;
        try {
            await Promise.all(ids.map(id => {
                const d = (modsRef.current?.find(m => m.id === id)?.delay ?? 0);
                return invoke('set_mod', { folderName: folder, modId: id, enabled: false, delay: d });
            }));
            setMods(prev => prev.map(m => ids.includes(m.id) ? { ...m, enabled: false } : m));
            showToast(t('manage.disable_selected_success') || '禁用成功', 'success');
            clearSelection();
        } catch (e) {
            console.error(e);
            showToast((t('manage.disable_selected_failed') || '禁用失败') + ': ' + (e?.message || String(e)), 'error');
        }
    }

    function askDeleteSelected() {
        const ids = Array.from(selectedIds);
        if (ids.length === 0) return;
        setConfirmPayload({ type: 'delete_selected', ids });
        setConfirmOpen(true);
    }

    // ------------------- Confirm modal handling -------------------
    async function handleConfirmOk() {
        if (!confirmPayload) return closeConfirm();
        if (confirmPayload.type === 'delete_version') {
            try {
                const res = await invoke('delete_version', { folderName: folder });
                showToast(typeof res === 'string' ? res : (t('common.delete_success') || '删除成功'), 'success');
                setConfirmOpen(false);
                setConfirmPayload(null);
                if (typeof onDone === 'function') onDone({ action: 'deleted', folder });
                if (typeof onClose === 'function') onClose();
            } catch (e) {
                console.error(e);
                showToast((t('common.delete_failed') || '删除失败') + ': ' + (e?.message || String(e)), 'error', 5000);
            }
        } else if (confirmPayload.type === 'delete_selected') {
            const ids = confirmPayload.ids || [];
            try {
                await invoke('delete_mods', { folderName: folder, modIds: ids });
                setMods(prev => prev.filter(m => !ids.includes(m.id)));
                showToast(t('manage.delete_selected_success') || '删除成功', 'success');
                setConfirmOpen(false);
                setConfirmPayload(null);
                clearSelection();
            } catch (e) {
                console.error(e);
                showToast((t('manage.delete_selected_failed') || '删除失败') + ': ' + (e?.message || String(e)), 'error', 5000);
            }
        }
    }

    function closeConfirm() {
        setConfirmOpen(false);
        setConfirmPayload(null);
    }

    // ------------------- Filtered list -------------------
    const filteredMods = useMemo(() => {
        const q = (searchTerm || "").toLowerCase().trim();
        if (!q) return mods;
        return mods.filter(m => (m.name || '').toLowerCase().includes(q) || (String(m.id) || '').toLowerCase().includes(q));
    }, [mods, searchTerm]);

    // ------------------- Close handler (auto-save) -------------------
    async function handleCloseClick() {
        try {
            await flushAllSaves();
        } catch (e) {
            console.error('error saving on close', e);
        }
        if (typeof onClose === 'function') onClose();
    }

    return (
        <div className="vm-page">
            <div className="vm-header">
                <div>
                    <h2 className="vm-title">{folder}</h2>
                    <div className="vm-sub">{t('manage.title') || '版本管理'}</div>
                </div>

                <div className="vm-actions">
                    <button className="btn btn-primary" onClick={openFolder} disabled={!path}>{t('manage.open_folder') || '打开目录'}</button>
                    <button
                        className="btn"
                        aria-label="Close"
                        onClick={handleCloseClick}
                        type="button"
                        title={t('common.close') || 'Close'}
                    >
                        {t('common.close') || 'Close'}
                    </button>
                </div>
            </div>

            <div className="vm-ops-bar">
                {multiMode && selectedIds.size > 0 ? (
                    <div className="vm-ops-multi">
                        <div className="vm-ops-left">{t('manage.selected') || '已选'}: {selectedIds.size}</div>
                        <div className="vm-ops-right">
                            <button className="btn" onClick={enableSelected}>{t('manage.enable_selected') || '启用'}</button>
                            <button className="btn" onClick={disableSelected}>{t('manage.disable_selected') || '禁用'}</button>
                            <button className="btn btn-danger" onClick={askDeleteSelected}>{t('manage.delete_selected') || '删除'}</button>
                            <button className="btn btn-ghost" onClick={clearSelection}>{t('common.cancel') || '取消'}</button>
                        </div>
                    </div>
                ) : (
                    <div className="vm-ops-normal">
                        <div className="vm-ops-left">
                            <button className="btn" onClick={handleImportMods}>{t('manage.import_mods') || '导入 Mod(s)'}</button>
                            <button className="btn" onClick={openModsFolder}>{t('manage.open_mods_folder') || '打开 Mod 文件夹'}</button>
                        </div>
                        <div className="vm-ops-right">
                            <input
                                className="vm-search-input"
                                placeholder={t('manage.search_placeholder') || '搜索 name / id...'}
                                value={searchTerm}
                                onChange={(e) => setSearchTerm(e.target.value)}
                            />
                            <button className="btn" onClick={handleRefresh}>{t('manage.refresh') || '刷新'}</button>
                        </div>
                    </div>
                )}
            </div>

            <div className="vm-card">
                <div className="vm-section-title">{t('manage.mods') || 'Mods 列表'}</div>
                {loading && <div className="vm-note">{t('common.loading') || '加载中...'}</div>}
                {!loading && filteredMods.length === 0 && <div className="vm-note">{t('manage.no_mods') || '没有检测到 mods'}</div>}

                <div className="vm-mods">
                    {filteredMods.map(mod => (
                        <div
                            key={mod.id}
                            className={`vm-mod ${selectedIds.has(mod.id) ? 'selected' : ''}`}
                            onContextMenu={(e) => handleContextMenu(e, mod.id)}
                        >
                            <div className="vm-mod-checkbox">
                                <input
                                    type="checkbox"
                                    aria-label={`select-${mod.id}`}
                                    checked={selectedIds.has(mod.id)}
                                    onChange={() => toggleSelection(mod.id)}
                                />
                            </div>

                            <div className="vm-mod-left">
                                <div className="vm-mod-name">{mod.name}</div>
                                <div className="vm-mod-id">{t('manage.mod_id') || 'ID'}: {mod.id}</div>
                            </div>

                            <div className="vm-mod-right">
                                <label className="vm-switch">
                                    <input type="checkbox" checked={!!mod.enabled} onChange={() => toggleMod(mod.id)} />
                                    <span className="vm-switch-label">{t('manage.enabled') || '启用'}</span>
                                </label>

                                <div className="vm-delay">
                                    <input
                                        type="number"
                                        min={0}
                                        value={mod.delay ?? 0}
                                        onChange={(e) => setDelay(mod.id, e.target.value)}
                                        className="vm-input-number"
                                    />
                                    <span className="vm-delay-label">{t('manage.delay_ms') || '延迟(ms)'}</span>
                                </div>
                            </div>
                        </div>
                    ))}
                </div>
            </div>

            {confirmOpen && createPortal(
                <div className="vm-modal-overlay">
                    <div className="vm-modal" onClick={(e) => e.stopPropagation()}>
                        <div className="vm-modal-title">{t('common.confirm_title') || '确认'}</div>
                        <div className="vm-modal-body">
                            {confirmPayload?.type === 'delete_version'
                                ? t('manage.confirm_delete_version', { folder, defaultValue: `确定删除版本 ${folder} 吗？` })
                                : confirmPayload?.type === 'delete_selected'
                                    ? t('manage.confirm_delete_selected', {
                                        count: selectedCount,
                                        defaultValue: `确定删除选中的 ${selectedCount} 个 Mod 吗？`
                                    })
                                    : t('common.confirm_text', { defaultValue: '确认执行操作？' })}
                        </div>
                        <div className="vm-modal-actions">
                            <button className="btn btn-ghost" onClick={closeConfirm}>{t('common.cancel') || '取消'}</button>
                            <button className="btn btn-danger" onClick={handleConfirmOk}>{t('common.confirm') || '确认'}</button>
                        </div>
                    </div>
                </div>, document.body
            )}
        </div>
    );
}
