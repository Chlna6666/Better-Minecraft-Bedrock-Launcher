// File: /src/pages/Manage/versionManage/useVersionManageData.js
import { useEffect, useMemo, useState } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { useToast } from '../../../components/Toast.tsx';

export default function useVersionManageData(props = {}) {
    const { folder = null, path = null, kind = 'gdk', edition = 'release', versionTypeLabel = null, onClose = null } = props;
    const { t } = useTranslation?.() || { t: (s, o) => (o?.defaultValue || s) };
    const toast = useToast?.() || { success: () => {}, error: () => {}, info: () => {} };

    const normalizedKind = useMemo(() => String(kind || '').toLowerCase(), [kind]);
    const [activeTab, setActiveTab] = useState('maps');
    const [users, setUsers] = useState([]);
    const [activeUserId, setActiveUserId] = useState(null);
    const [mods, setMods] = useState([]);
    const [maps, setMaps] = useState([]);
    const [mapTemplates, setMapTemplates] = useState([]);
    const [resourcePacks, setResourcePacks] = useState([]);
    const [behaviorPacks, setBehaviorPacks] = useState([]);
    const [skins, setSkins] = useState([]);
    const [searchTerms, setSearchTerms] = useState({ mods: '', maps: '', mapTemplates: '', resourcePacks: '', behaviorPacks: '', skins: '' });
    const [loading, setLoading] = useState(false);
    const [confirmDeleteUserId, setConfirmDeleteUserId] = useState(null);
    const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);
    const [versionSettings, setVersionSettings] = useState({ instanceName: '', iconPath: '', desktopShortcutCreated: false, versionIsolation: false, enableEditMode: false, enableConsole: false });

    const tabs = useMemo(() => {
        if (normalizedKind === 'gdk') return [
            { key: 'maps', label: '地图' },
            { key: 'mapTemplates', label: '地图模板' },
            { key: 'resourcePacks', label: '资源包' },
            { key: 'behaviorPacks', label: '行为包' },
            { key: 'skins', label: '皮肤包' },
            { key: 'mods', label: 'Mods' },
            { key: 'versionSettings', label: '版本设置' },
        ];
        return [
            { key: 'mods', label: 'Mods' },
            { key: 'versionSettings', label: '版本设置' },
        ];
    }, [normalizedKind]);

    useEffect(() => {
        if (tabs && tabs.length > 0) {
            if (!tabs.find(t => t.key === activeTab)) setActiveTab(tabs[0].key);
        } else {
            setActiveTab('mods');
        }
    }, [tabs]);

    // ------------------ backend integration ------------------
    async function fetchGdkUsersByEdition(editionToQuery) {
        try {
            const res = await invoke('get_gdk_users', { edition: editionToQuery });
            if (!Array.isArray(res)) return [];
            return res;
        } catch (e) {
            console.warn('fetchGdkUsersByEdition failed', e);
            return [];
        }
    }

    async function loadGdkUsersToState() {
        try {
            setLoading(true);
            const found = await fetchGdkUsersByEdition(edition);
            const mapped = (found || []).map((it, idx) => {
                const id = (it && it.user_id) ? `gdkid:${String(it.user_id)}` : `gdkpath:${encodeURIComponent(it.path || (`${it.edition}|${it.user_folder}|${idx}`))}`;
                const name = (it && it.user_folder)
                    ? String(it.user_folder)
                    : ((it && it.user_id)
                        ? String(it.user_id)
                        : (it?.edition_label || it?.edition || `GDK ${idx}`));
                return { id, name, path: it.path || null, edition: it.edition, edition_label: it.edition_label, raw: it };
            });
            setUsers(mapped);
            if (mapped.length > 0) {
                setActiveUserId(mapped[0].id);
            } else {
                setActiveUserId(null);
            }
            return mapped;
        } catch (e) {
            console.error('loadGdkUsersToState failed', e);
            return users;
        } finally {
            setLoading(false);
        }
    }

    async function loadResourcesForActiveUser(userId) {
        if (!userId) {
            setMaps([]); setMapTemplates([]); setResourcePacks([]); setBehaviorPacks([]); setSkins([]);
            return;
        }
        const user = users.find(u => u.id === userId);
        if (!user) {
            setMaps([]); setMapTemplates([]); setResourcePacks([]); setBehaviorPacks([]); setSkins([]);
            return;
        }

        try {
            setLoading(true);
            const user_folder = user.raw?.user_folder || null;
            const user_id = user.raw?.user_id ?? null;
            const versions_file = folder || null;

            const mapsRes = await invoke('list_minecraft_worlds_for_user', { user_folder, user_id, edition, versions_file, concurrency: 0 });
            if (Array.isArray(mapsRes)) setMaps(mapsRes); else setMaps([]);
        } catch (e) {
            console.error('loadResourcesForActiveUser -> list_minecraft_worlds_for_user failed', e);
            setMaps([]);
        } finally {
            setLoading(false);
        }
    }

    async function loadAll() {
        setLoading(true);
        try {
            setVersionSettings(v => ({ ...v, instanceName: folder || '', iconPath: '' }));
            if (normalizedKind === 'gdk') {
                const loadedUsers = await loadGdkUsersToState();
                await loadResourcesForActiveUser(activeUserId ?? (loadedUsers[0]?.id || null));
            } else {
                setUsers([]); setActiveUserId(null);
                await loadResourcesForActiveUser(null);
            }
        } catch (e) {
            console.error('loadAll failed', e);
            toast.error((t('common.load_failed') || '加载失败') + ': ' + (e?.message || String(e)));
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        if (activeUserId === null) {
            loadResourcesForActiveUser(null).catch(e => console.error(e));
        } else {
            loadResourcesForActiveUser(activeUserId).catch(e => console.error('loadResourcesForActiveUser error', e));
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [activeUserId]);

    useEffect(() => {
        loadAll();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [folder, normalizedKind, edition]);

    function removeUser(id) {
        setUsers(prev => prev.filter(u => u.id !== id));
        if (activeUserId === id) setActiveUserId(users[0]?.id || null);
    }

    function toggleModEnabled(modId) {
        setMods(prev => prev.map(m => m.id === modId ? { ...m, enabled: !m.enabled } : m));
    }

    function setModDelay(modId, delay) {
        const v = Math.max(0, parseInt(String(delay || '0'), 10) || 0);
        setMods(prev => prev.map(m => m.id === modId ? { ...m, delay: v } : m));
    }

    async function createDesktopShortcut() {
        try {
            setVersionSettings(s => ({ ...s, desktopShortcutCreated: true }));
            toast.success('桌面快捷方式已创建');
        } catch (e) {
            console.error('createDesktopShortcut failed', e);
            toast.error('创建桌面快捷方式失败: ' + (e?.message || String(e)));
        }
    }

    async function openFolder() {
        try {
            let target = path || null;
            if (!target && activeUserId) {
                const activeUser = users.find(u => u.id === activeUserId);
                if (activeUser && activeUser.path) target = activeUser.path;
            }
            if (target) {
                await invoke('open_path', { path: target });
                return;
            }
            let picked = null;
            try { picked = await open({ directory: true, multiple: false }); } catch (e) {}
            if (picked) {
                const p = Array.isArray(picked) ? picked[0] : picked;
                await invoke('open_path', { path: p });
            } else {
                toast.info('未选择目录');
            }
        } catch (e) {
            console.error('openFolder failed', e);
            toast.error('打开目录失败: ' + (e?.message || String(e)));
        }
    }

    return {
        t,
        toast,
        normalizedKind,
        loading,
        users,
        activeTab,
        setActiveTab,
        activeUserId,
        setActiveUserId,
        mods,
        maps,
        mapTemplates,
        resourcePacks,
        behaviorPacks,
        skins,
        searchTerms,
        setSearchTerms,
        confirmDeleteUserId,
        setConfirmDeleteUserId,
        confirmDeleteOpen,
        setConfirmDeleteOpen,
        versionSettings,
        setVersionSettings,
        tabs,
        loadAll,
        openFolder,
        removeUser,
        toggleModEnabled,
        setModDelay,
        createDesktopShortcut,
        versionTypeLabel,
        edition,
        folder,
        onClose,
    };
}