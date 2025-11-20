import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from 'react-i18next';
import useVersions from "../../hooks/useVersions.jsx";
import "./VersionManager.css";
import unknownIcon from "../../assets/feather/box.svg";
import { invoke } from "@tauri-apps/api/core";
import VersionManagePage from "./VersionManagePage.jsx";
import { useToast } from "../../components/Toast.jsx";
import Select from "../../components/Select.jsx";
import { Input } from "../../components/index.js";

/**
 * VersionManager
 * - 过滤基于 versionTypeLabel（本地化文本）
 * - 搜索防抖（debounce）
 * - useMemo/useCallback 优化性能
 */

function VersionManager() {
    const { t } = useTranslation();
    const { versions, counts, reload } = useVersions();

    const [filter, setFilter] = useState("");
    // 原始输入（立刻更新），用于防抖
    const [rawSearch, setRawSearch] = useState("");
    // 实际用于过滤的搜索值（防抖结果）
    const [search, setSearch] = useState("");
    const [deleting, setDeleting] = useState({});

    const [manageOpen, setManageOpen] = useState(false);
    const [manageVersion, setManageVersion] = useState(null);

    const [confirmOpen, setConfirmOpen] = useState(false);
    const [confirmTarget, setConfirmTarget] = useState(null);

    const toast = useToast();

    const debounceRef = useRef(null);
    useEffect(() => {
        if (debounceRef.current) clearTimeout(debounceRef.current);
        debounceRef.current = setTimeout(() => {
            setSearch(rawSearch.trim().toLowerCase());
        }, 300);
        return () => clearTimeout(debounceRef.current);
    }, [rawSearch]);


    const uniqueLabels = useMemo(() => {
        // versions可能为空或尚未加载，防守检查
        if (!versions || versions.length === 0) return [];

        const seen = new Set();
        const labels = [];
        for (const v of versions) {
            const lbl = v.versionTypeLabel || "";
            if (!lbl) continue;
            if (!seen.has(lbl)) {
                seen.add(lbl);
                labels.push(lbl);
            }
        }
        return labels;
    }, [versions]);

    // 构造 filterOptions：先放 All，然后把 uniqueLabels 放入（保持本地化文本为 value）
    const filterOptions = useMemo(() => {
        const base = [
            { value: '', label: t('common.all_versions') || 'All' },
        ];
        // 保留 uniqueLabels 的排序，并去重已由 uniqueLabels 保证
        uniqueLabels.forEach(lbl => base.push({ value: lbl, label: lbl }));
        return base;
    }, [uniqueLabels, t]);

    const filteredVersions = useMemo(() => {
        if (!versions || versions.length === 0) return [];

        const normalized = versions.map(v => ({
            ...v,
            __searchName: (v.name || "").toLowerCase(),
            __searchVersion: (v.version || "").toLowerCase(),
            __searchFolder: (v.folder || "").toLowerCase(),
            __labelLower: (v.versionTypeLabel || "").toLowerCase(),
        }));

        const wantLabel = (filter || "").toLowerCase();

        return normalized.filter(v => {
            // 1) 类型过滤（'' 表示全部）
            if (wantLabel && wantLabel !== '') {
                if (v.__labelLower !== wantLabel) return false;
            }
            // 2) 搜索匹配（若 search 为空则通过）
            if (!search) return true;
            const s = search;
            return v.__searchName.includes(s) || v.__searchVersion.includes(s) || v.__searchFolder.includes(s);
        });
    }, [versions, filter, search]);

    const openConfirm = useCallback((folder) => {
        setConfirmTarget(folder);
        setConfirmOpen(true);
    }, []);

    const closeConfirm = useCallback(() => {
        setConfirmOpen(false);
        setConfirmTarget(null);
    }, []);

    const confirmDelete = useCallback(async () => {
        const folder = confirmTarget;
        if (!folder) return;
        setDeleting(prev => ({ ...prev, [folder]: true }));
        closeConfirm();

        try {
            const res = await invoke('delete_version', { folderName: folder });
            const msg = typeof res === 'string' ? res : (t('common.delete_success') || '删除成功');
            toast.success(msg);
            if (typeof reload === 'function') await reload();
        } catch (err) {
            let msg = '';
            if (!err) msg = t('common.delete_failed') || '删除失败';
            else if (err?.message) msg = err.message;
            else if (typeof err === 'string') msg = err;
            else msg = JSON.stringify(err);
            toast.error((t('common.delete_failed') || '删除失败') + ': ' + msg, { duration: 5000 });
        } finally {
            setDeleting(prev => {
                const copy = { ...prev };
                if (folder && copy[folder]) delete copy[folder];
                return copy;
            });
        }
    }, [confirmTarget, closeConfirm, reload, t, toast]);

    const handleManage = useCallback((version) => {
        setManageVersion(version);
        setManageOpen(true);
    }, []);

    const handleManageDone = useCallback((result) => {
        setManageOpen(false);
        setManageVersion(null);
        if (result?.action === 'deleted' && typeof reload === 'function') {
            reload();
        }
    }, [reload]);

    const handleImgError = useCallback((e) => {
        try { e.target.src = unknownIcon; } catch (_) {}
    }, []);

    const handleReload = useCallback(() => {
        if (typeof reload === 'function') reload();
    }, [reload]);

    return (
        <div className="vlist-wrapper">
            <div className="vtoolbar">
                <Input
                    type="text"
                    placeholder={t('common.search_placeholder')}
                    value={rawSearch}
                    onChange={(e) => setRawSearch(e.target.value)}
                    style={{ flex: 1 }}
                    inputStyle={{ height: '29px' }}
                />
                <Select
                    size={13}
                    value={filter}
                    onChange={(val) => setFilter(String(val))}
                    options={filterOptions}
                    placeholder={t('common.all_versions')}
                />
                <button onClick={handleReload} className="vrefresh-btn">
                    {t('common.refresh')}
                </button>
            </div>

            <div className="vlist-container">
                {filteredVersions.map(({ folder, name, version, path, kind, kindLabel, versionType, versionTypeLabel, icon }) => {
                    const count = counts[folder] || 0;
                    const isDeleting = !!deleting[folder];
                    return (
                        <div key={folder} className="vcard">
                            <img
                                src={icon || unknownIcon}
                                alt={name}
                                className="vimg"
                                onError={handleImgError}
                            />
                            <div className="vdetails">
                                <div className="vname">{folder}</div>
                                <div className="vmeta">
                                    <span className="vbadge">{versionTypeLabel}</span>{" "}
                                    <span className="vbadge">{kindLabel}</span>{" "}
                                    {version} · {t('common.launch_count')}: {count}
                                </div>
                            </div>
                            <div className="vactions">
                                <button
                                    className="vbtn-manage"
                                    onClick={() => handleManage({ folder, path, name, version })}
                                    disabled={isDeleting}
                                >
                                    {t('common.manage')}
                                </button>
                                <button
                                    className="vbtn-delete"
                                    onClick={() => openConfirm(folder)}
                                    disabled={isDeleting}
                                >
                                    {isDeleting ? (t('common.processing') || '处理中…') : t('common.delete')}
                                </button>
                            </div>
                        </div>
                    );
                })}

                {filteredVersions.length === 0 && (
                    <div className="vempty">{t('common.no_result')}</div>
                )}
            </div>

            {/* 管理页 Modal */}
            {manageOpen && createPortal(
                <div className="version-manage-modal-overlay">
                    <div className="version-manage-modal">
                        <VersionManagePage
                            folder={manageVersion?.folder}
                            path={manageVersion?.path}
                            onDone={handleManageDone}
                            onClose={() => setManageOpen(false)}
                        />
                    </div>
                </div>,
                document.body
            )}

            {/* 删除确认 Modal */}
            {confirmOpen && createPortal(
                <div className="modal-overlay" onClick={closeConfirm}>
                    <div className="modal" onClick={(e) => e.stopPropagation()}>
                        <div className="modal-title">{t('common.confirm_title') || '确认删除'}</div>
                        <div className="modal-body">
                            {t('common.confirm_delete_text', { folder: confirmTarget })}
                        </div>
                        <div className="modal-actions">
                            <button className="modal-btn modal-cancel" onClick={closeConfirm}>
                                {t('common.cancel') || '取消'}
                            </button>
                            <button className="modal-btn modal-confirm" onClick={confirmDelete}>
                                {t('common.confirm') || '确认删除'}
                            </button>
                        </div>
                    </div>
                </div>,
                document.body
            )}
        </div>
    );
}

export default VersionManager;
