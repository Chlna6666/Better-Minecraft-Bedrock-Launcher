// File: /src/pages/Manage/VersionManagePage.jsx
import React from 'react';
import './VersionManagePage.css';
import useVersionManageData from './versionManage/useVersionManageData';
import TabNav from './versionManage/components/TabNav';
import UserSwitcher from './versionManage/components/UserSwitcher';
import ModsPanel from './versionManage/components/ModsPanel';
import MapsPanel from './versionManage/components/MapsPanel';
import VersionSettingsPanel from './versionManage/components/VersionSettingsPanel';
import ConfirmDeleteModal from './versionManage/components/ConfirmDeleteModal';

export default function VersionManagePage(props) {
    const hook = useVersionManageData(props);

    const {
        t,
        toast,
        normalizedKind,
        loading,
        users,
        activeTab,
        setActiveTab,
        activeUserId,
        setActiveUserId,
        loadAll,
        openFolder,
        removeUser,
        confirmDeleteOpen,
        confirmDeleteUserId,
        setConfirmDeleteOpen,
        setConfirmDeleteUserId,
        versionTypeLabel,
        edition,
        folder,
        onClose,
    } = hook;

    return (
        <div className="vmp-root">
            <header className="vmp-header">
                <div className="vmp-title-area">
                    <h2 className="vmp-title">{folder || '版本管理'}</h2>
                    <div className="vmp-subtitle">
                        {normalizedKind === 'gdk' ? 'GDK' : 'UWP'}
                        {versionTypeLabel ? (
                            <span className="vmp-version-type"> · {versionTypeLabel}</span>
                        ) : (
                            <span className="vmp-version-type"> · {edition === 'preview' ? '预览版' : '正式版'}</span>
                        )}
                    </div>
                </div>

                <div className="vmp-controls">
                    <UserSwitcher {...hook} />
                    <div className="vmp-control-buttons">
                        <button className="btn" onClick={() => loadAll()}>刷新</button>
                        <button className="btn" onClick={() => openFolder()}>打开目录</button>
                        <button className="btn btn-ghost" onClick={() => { if (typeof onClose === 'function') onClose(); }}>关闭</button>
                    </div>
                </div>
            </header>

            <TabNav tabs={hook.tabs} activeTab={activeTab} onChange={setActiveTab} />

            <main className="vmp-main" role="main">
                {loading && <div className="vmp-loading">加载中…</div>}

                {!loading && normalizedKind === 'gdk' && users.length === 0 && (
                    <div className="vmp-no-users">
                        <h3>未检测到 GDK 用户</h3>
                        <p>未在 Minecraft 的 Users 目录中发现可用用户。请确保 Minecraft（正式版或预览版）至少启动一次，或者选择正确的“版本类型/通道”。</p>
                        <div style={{ display: 'flex', gap: 8, marginTop: 12 }}>
                            <button className="btn" onClick={() => loadAll()}>重试</button>
                            <button className="btn" onClick={() => toast.info('请启动 Minecraft 后重试，或在设置中切换版本通道（正式/预览）。')}>查看帮助</button>
                        </div>
                    </div>
                )}

                {!loading && (activeTab === 'mods') && <ModsPanel {...hook} />}
                {!loading && (activeTab === 'maps') && normalizedKind === 'gdk' && <MapsPanel {...hook} />}
                {!loading && (activeTab === 'versionSettings') && <VersionSettingsPanel {...hook} />}

                {confirmDeleteOpen && (
                    <ConfirmDeleteModal
                        onCancel={() => { setConfirmDeleteOpen(false); setConfirmDeleteUserId(null); }}
                        onDelete={() => {
                            try {
                                removeUser(confirmDeleteUserId);
                                toast.success('用户已删除');
                            } catch (e) {
                                console.error(e);
                                toast.error('删除用户失败: ' + (e?.message || String(e)));
                            } finally {
                                setConfirmDeleteOpen(false);
                                setConfirmDeleteUserId(null);
                            }
                        }}
                    />
                )}
            </main>
        </div>
    );
}