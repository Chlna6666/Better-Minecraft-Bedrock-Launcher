import { useEffect, useState, useCallback, useRef, useMemo } from "react";
import { createPortal } from "react-dom"; // <- 用于 portal
import { useTranslation } from 'react-i18next';
import useVersions from "../../hooks/useVersions.jsx";
import "./VersionManager.css";
import unknownIcon from "../../assets/feather/box.svg";
import { invoke } from "@tauri-apps/api/core";
import VersionManagePage from "./VersionManagePage.jsx";
import {useToast} from "../../components/Toast.jsx";
import Select from "../../components/Select.jsx";
import {Input} from "../../components/index.js"; // <- 新增导入

function VersionManager() {
    const { t } = useTranslation();
    const { versions, counts, reload } = useVersions();
    const [filter, setFilter] = useState("");
    const [search, setSearch] = useState("");
    const [deleting, setDeleting] = useState({});

    const [manageOpen, setManageOpen] = useState(false);
    const [manageVersion, setManageVersion] = useState(null);

    // 确认弹窗状态
    const [confirmOpen, setConfirmOpen] = useState(false);
    const [confirmTarget, setConfirmTarget] = useState(null);

    // useToast for notifications (ToastProvider must wrap app)
    const toast = useToast();

    // 筛选逻辑
    const filteredVersions = useMemo(() => {
        return versions.filter(v => {
            const matchType = filter ? v.type?.toLowerCase() === filter.toLowerCase() : true;
            const matchSearch = search
                ? (v.name?.toLowerCase().includes(search.toLowerCase()) ||
                    v.version?.toLowerCase().includes(search.toLowerCase()) ||
                    v.folder?.toLowerCase().includes(search.toLowerCase()))
                : true;
            return matchType && matchSearch;
        });
    }, [filter, search, versions]);

    const uniqueTypes = useMemo(() => {
        const types = Array.from(new Set(versions.map(v => v.type)));
        return types.filter(Boolean);
    }, [versions]);

    // 为自定义 Select 构造 options（第一个为 "全部"）
    const filterOptions = useMemo(() => {
        const opts = [{ value: '', label: t('common.all_versions') || 'All' }];
        uniqueTypes.forEach(type => opts.push({ value: type, label: type }));
        return opts;
    }, [uniqueTypes, t]);

    // 打开确认弹窗（用于点击删除）
    const openConfirm = (folder) => {
        setConfirmTarget(folder);
        setConfirmOpen(true);
    };

    // 关闭弹窗
    const closeConfirm = () => {
        setConfirmOpen(false);
        setConfirmTarget(null);
    };

    // 真正执行删除（确认后）
    const confirmDelete = async () => {
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
    };

    const handleManage = (version) => {
        setManageVersion(version);
        setManageOpen(true);
    };

    const handleManageDone = (result) => {
        setManageOpen(false);
        setManageVersion(null);
        if (result?.action === 'deleted' && typeof reload === 'function') {
            reload();
        }
    };

    // ---- Portal 渲染的 Modal（删除确认） ----
    const modalPortal = confirmOpen ? createPortal(
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
    ) : null;
    return (
        <div className="vlist-wrapper">
            <div className="vtoolbar">
                <Input
                    type="text"
                    placeholder={t('common.search_placeholder')}
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
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
                <button onClick={reload} className="vrefresh-btn">
                    {t('common.refresh')}
                </button>
            </div>

            <div className="vlist-container">
                {filteredVersions.map(({ folder, path, name, version, type, icon }) => {
                    const count = counts[folder] || 0;
                    const isDeleting = !!deleting[folder];
                    return (
                        <div key={folder} className="vcard">
                            <img
                                src={icon}
                                alt={name}
                                className="vimg"
                                onError={(e) => (e.target.src = unknownIcon)}
                            />
                            <div className="vdetails">
                                <div className="vname">{folder}</div>
                                <div className="vmeta">
                                    <span className="vbadge">{type}</span> {version} · {t('common.launch_count')}: {count}
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

            {/* 管理页 Modal：这里不再渲染 Close 按钮，全部交给子组件自己布局 */}
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

            {modalPortal}
        </div>
    );
}

export default VersionManager;
