import React, { useEffect, useState, useMemo, useRef } from 'react';
import { useParams, useNavigate, useLocation } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { motion } from 'framer-motion';
import {
    ChevronLeft, Download, Calendar,
    Globe, FileText, Layers, User, Box, Copy, Share2
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useToast } from "../../../components/Toast";
import Select from "../../../components/Select.jsx";
import { tCurseForgeTag } from "./curseForgeTagI18n";
import { CurseForgeInstallModal } from "./CurseForgeInstallModal";
import './CurseForgeModPage.css';

interface Mod {
    id: number;
    name: string;
    summary: string;
    authors: { name: string }[];
    logo: { url: string };
    downloadCount: number;
    categories: { id: number; name: string; slug?: string; iconUrl?: string }[];
    dateModified: string;
    links?: { websiteUrl?: string; sourceUrl?: string; issuesUrl?: string };
    classId?: number;
}

interface ModFile {
    id: number;
    displayName: string;
    fileName: string;
    fileLength: number;
    downloadUrl: string;
    gameVersions: string[];
    fileDate: string;
    releaseType: number;
}

const CACHE_TTL = 1000 * 60 * 15;

const CurseForgeModPage: React.FC = () => {
    const { id } = useParams<{ id: string }>();
    const navigate = useNavigate();
    const location = useLocation();
    const toast = useToast();
    const { t } = useTranslation();

    // Data State
    const [mod, setMod] = useState<Mod | null>(null);
    const [description, setDescription] = useState<string>('');
    const [files, setFiles] = useState<ModFile[]>([]);

    // UI State
    const [loading, setLoading] = useState(true);
    const [filesLoading, setFilesLoading] = useState(true);
    const [activeTab, setActiveTab] = useState<'desc' | 'files'>('desc');
    const [selectedVersion, setSelectedVersion] = useState<string>('all');
    const filesSectionRef = useRef<HTMLDivElement | null>(null);
    const [tagsExpanded, setTagsExpanded] = useState(false);
    const [installOpen, setInstallOpen] = useState(false);
    const [installFile, setInstallFile] = useState<ModFile | null>(null);

    const readCache = <T,>(key: string, ttl: number): T | null => {
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
    const writeCache = (key: string, value: any) => {
        try { sessionStorage.setItem(key, JSON.stringify({ ts: Date.now(), value })); } catch { }
    };

    useEffect(() => {
        if (!id) return;
        const modId = parseInt(id);
        const cacheModKey = `cf:mod:${modId}`;
        const cacheDescKey = `cf:modDesc:${modId}`;
        const cacheFilesKey = `cf:modFiles:${modId}`;

        const cachedMod = readCache<Mod>(cacheModKey, CACHE_TTL);
        const cachedDesc = readCache<string>(cacheDescKey, CACHE_TTL);
        const cachedFiles = readCache<ModFile[]>(cacheFilesKey, CACHE_TTL);

        if (cachedMod) setMod(cachedMod);
        if (cachedDesc) setDescription(cachedDesc);
        if (cachedFiles) setFiles(cachedFiles);
        setFilesLoading(!cachedFiles);
        setLoading(!(cachedMod && cachedDesc && cachedFiles));

        const fetchData = async () => {
            setLoading(!(cachedMod && cachedDesc && cachedFiles));
            try {
                const [modData, descData, filesData] = await Promise.all([
                    invoke<Mod>('get_curseforge_mod', { modId }),
                    invoke<string>('get_curseforge_mod_description', { modId }),
                    invoke<ModFile[]>('get_curseforge_mod_files', { modId, gameVersion: null })
                ]);
                setMod(modData);
                const safeDesc = descData || `<p>${t("CurseForgeMod.no_description")}</p>`;
                setDescription(safeDesc);
                setFiles(filesData || []);
                setFilesLoading(false);

                writeCache(cacheModKey, modData);
                writeCache(cacheDescKey, safeDesc);
                writeCache(cacheFilesKey, filesData || []);
            } catch (e: any) {
                toast.error(t("CurseForgeMod.load_failed", { message: e.message }));
            } finally {
                setLoading(false);
                setFilesLoading(false);
            }
        };
        fetchData();
    }, [id, t]);

    const allGameVersions = useMemo(() => {
        const versions = new Set<string>();
        files.forEach(f => {
            f.gameVersions?.forEach(v => {
                if (/^\d/.test(v)) versions.add(v);
            });
        });
        return Array.from(versions).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
    }, [files]);

    const versionOptions = useMemo(() => {
        return [
            { value: 'all', label: t("CurseForgeMod.all_versions") },
            ...allGameVersions.map(v => ({ value: v, label: v })),
        ];
    }, [allGameVersions, t]);

    const filteredFiles = useMemo(() => {
        const scoped = selectedVersion === 'all'
            ? [...files]
            : files.filter(f => f.gameVersions.includes(selectedVersion));
        return scoped.sort((a, b) => new Date(b.fileDate).getTime() - new Date(a.fileDate).getTime());
    }, [files, selectedVersion]);

    const formatCount = (n: number) => {
        if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
        if (n >= 1e3) return (n / 1e3).toFixed(1) + 'K';
        return n;
    };

    const formatSize = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const goToFiles = () => {
        setActiveTab('files');
        setTimeout(() => {
            filesSectionRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
        }, 80);
    };

    const getModLink = () => {
        return (
            mod?.links?.websiteUrl ||
            mod?.links?.sourceUrl ||
            `https://www.curseforge.com/projects/${mod?.id}`
        );
    };

    const getModType = () => {
        const url = getModLink();
        try {
            const parsed = new URL(url);
            const parts = parsed.pathname.split('/').filter(Boolean);
            const idx = parts.indexOf('minecraft-bedrock');
            const kind = idx >= 0 ? parts[idx + 1] : undefined;
            const map: Record<string, string> = {
                addons: 'Addons',
                maps: 'Maps',
                skins: 'Skins',
                'texture-packs': 'Texture Packs',
                scripts: 'Scripts',
                worlds: 'Maps',
            };
            if (kind && map[kind]) return map[kind];
        } catch {
            // ignore parse errors
        }
        return 'Resource';
    };

    const openWebsite = () => {
        const url = getModLink();
        if (!url) return;
        (async () => {
            try {
                await open(url);
            } catch {
                try { window.open(url, '_blank'); } catch { /* ignore */ }
            }
        })();
    };

    const copyShareLink = async () => {
        if (!mod) return;
        const url = getModLink();
        try {
            if (navigator?.clipboard?.writeText) {
                await navigator.clipboard.writeText(url);
            } else {
                const ta = document.createElement('textarea');
                ta.value = url;
                document.body.appendChild(ta);
                ta.select();
                document.execCommand('copy');
                document.body.removeChild(ta);
            }
            toast.success('链接已复制');
        } catch {
            toast.error('复制失败');
        }
    };

    const copyShareMessage = async () => {
        if (!mod) return;
        const url = getModLink();
        const type = getModType();
        const content = [
            `你的好友向你推荐了一个资源【${mod.name}】`,
            `地址：${url}`,
            `前往 [BMCBL]，Ctrl+V 即可获取该资源`,
            `ID: ${mod.id} Type: ${type}`,
        ].join('\n');
        try {
            if (navigator?.clipboard?.writeText) {
                await navigator.clipboard.writeText(content);
            } else {
                const ta = document.createElement('textarea');
                ta.value = content;
                document.body.appendChild(ta);
                ta.select();
                document.execCommand('copy');
                document.body.removeChild(ta);
            }
            toast.success('分享文本已复制');
        } catch {
            toast.error('复制失败');
        }
    };

    const copyAnalysis = async () => {
        if (!mod) return;
        const categories = mod.categories?.map(c => tCurseForgeTag(t, c)).join(', ') || t("common.unknown");
        const url = getModLink();
        const type = getModType();
        const content = [
            `名称: ${mod.name}`,
            `作者: ${mod.authors?.[0]?.name || t("common.unknown")}`,
            `更新时间: ${new Date(mod.dateModified).toLocaleDateString()}`,
            `下载量: ${formatCount(mod.downloadCount)}`,
            `分类: ${categories}`,
            `ID: ${mod.id} Type: ${type}`,
            `简介: ${mod.summary || ''}`,
            `链接: ${url || ''}`
        ].join('\n');
        try {
            if (navigator?.clipboard?.writeText) {
                await navigator.clipboard.writeText(content);
            } else {
                const ta = document.createElement('textarea');
                ta.value = content;
                document.body.appendChild(ta);
                ta.select();
                document.execCommand('copy');
                document.body.removeChild(ta);
            }
            toast.success('分析已复制');
        } catch {
            toast.error('复制失败');
        }
    };

    const handleDownload = (file: ModFile) => {
        setInstallFile(file);
        setInstallOpen(true);
    };

    const getGameVer = (file: ModFile) => {
        const vers = file.gameVersions.filter(v => /^\d/.test(v));
        if (vers.length === 0) return t("common.unknown");
        return vers[0] + (vers.length > 1 ? ` (+${vers.length-1})` : "");
    }

    const pageRef = useRef<HTMLDivElement | null>(null);

    useEffect(() => {
        const el = pageRef.current;
        if (!el) return;
        const onScroll = () => {
            if (el.scrollTop > 60) {
                el.classList.add('hero-compact');
            } else {
                el.classList.remove('hero-compact');
            }
        };
        onScroll();
        el.addEventListener('scroll', onScroll);
        return () => el.removeEventListener('scroll', onScroll);
    }, []);

    const goBack = () => {
        const from = (location.state as any)?.from;
        if (from?.pathname) {
            navigate(`${from.pathname}${from.search || ''}`, { state: from.state });
            return;
        }
        if (typeof from === 'string') {
            navigate(from);
            return;
        }
        navigate('/download', { state: { initialTab: 'resource' } });
    };

    if (loading) return (
        <div className="cf-mod-page loading">
            <div className="mod-skeleton-hero">
                <div className="sk-hero-logo" />
                <div className="sk-hero-lines">
                    <span className="sk-line long" />
                    <span className="sk-line" />
                    <span className="sk-line short" />
                </div>
            </div>
            <div className="mod-skeleton-body">
                {[...Array(6)].map((_, i) => (
                    <div key={i} className="sk-body-line" />
                ))}
            </div>
        </div>
    );

    if (!mod) return <div className="cf-mod-page error">{t("CurseForgeMod.not_found")}</div>;

    return (
        <>
        <motion.div ref={pageRef} className="cf-mod-page custom-scrollbar" initial={{ opacity: 0 }} animate={{ opacity: 1 }}>

            {/* --- Hero Header --- */}
            <div className="mod-hero-header">
                <div className="hero-content">
                    {/* [修复] 顶部导航栏 (返回按钮) */}
                    <div className="hero-nav-bar" data-tauri-drag-region>
                        <button className="back-btn-glass" onClick={goBack}>
                            <ChevronLeft size={20} /> {t("common.back")}
                        </button>
                    </div>

                    <div className="mod-hero-card">
                        <div className="hero-header">
                            <motion.div
                                className="hero-logo-wrapper"
                                initial={{ scale: 0.9, opacity: 0 }}
                                animate={{ scale: 1, opacity: 1 }}
                            >
                                <img
                                    src={mod.logo?.url}
                                    className="hero-logo-img"
                                    alt=""
                                    loading="eager"
                                    referrerPolicy="no-referrer"
                                />
                            </motion.div>

                            <div className="hero-header-content">
                                <h1 className="hero-title">{mod.name}</h1>
                                {mod.summary && <p className="hero-summary">{mod.summary}</p>}

                                <div className="hero-stats-row">
                                    <span className="stat-chip"><Download size={14} /> {formatCount(mod.downloadCount)}</span>
                                    <span className="stat-chip"><Calendar size={14} /> {new Date(mod.dateModified).toLocaleDateString()}</span>
                                    <span className="stat-chip"><User size={14} /> {mod.authors?.[0]?.name || t("common.unknown")}</span>
                                    <span className="stat-chip id">ID: {mod.id}</span>
                                </div>

                                <div className={`hero-tags-row ${tagsExpanded ? 'expanded' : 'collapsed'}`}>
                                    {(tagsExpanded ? mod.categories : mod.categories.slice(0, 6)).map(c => (
                                        <span key={c.id} className="hero-tag-pill" data-bm-title={tCurseForgeTag(t, c)}>
                                            {c.iconUrl ? (
                                                <img
                                                    src={c.iconUrl}
                                                    alt=""
                                                    loading="lazy"
                                                    referrerPolicy="no-referrer"
                                                    onError={(e) => { e.currentTarget.style.display = 'none'; }}
                                                />
                                            ) : (
                                                <Box size={12} />
                                            )}
                                            {tCurseForgeTag(t, c)}
                                        </span>
                                    ))}
                                    {!tagsExpanded && mod.categories.length > 6 && (
                                        <button className="hero-tag-more" onClick={() => setTagsExpanded(true)}>
                                            +{mod.categories.length - 6}
                                        </button>
                                    )}
                                    {tagsExpanded && mod.categories.length > 6 && (
                                        <button className="hero-tag-more" onClick={() => setTagsExpanded(false)}>
                                            收起
                                        </button>
                                    )}
                                </div>
                            </div>

                            <div className="hero-actions">
                                <button className="hero-primary-btn" onClick={goToFiles}>
                                    <Download size={18} strokeWidth={2.5} /> 安装
                                </button>
                                <div className="hero-action-mini">
                                    <button className="icon-btn-ghost" onClick={openWebsite} data-bm-title="打开浏览器">
                                        <Globe size={16} />
                                    </button>
                                    <button className="icon-btn-ghost" onClick={copyShareLink} data-bm-title="复制链接">
                                        <Copy size={16} />
                                    </button>
                                    <button className="icon-btn-ghost" onClick={copyShareMessage} data-bm-title="复制分享文本">
                                        <Share2 size={16} />
                                    </button>
                                    <button className="icon-btn-ghost" onClick={copyAnalysis} data-bm-title="复制分析">
                                        <FileText size={16} />
                                    </button>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>

            {/* --- Content Body --- */}
            <div className="mod-body-container">
                <div className="body-tabs-wrapper">
                    <div className="body-tabs">
                        <button className={`body-tab ${activeTab === 'desc' ? 'active' : ''}`} onClick={() => setActiveTab('desc')}>
                            <FileText size={16}/> {t("CurseForgeMod.tab_description")}
                        </button>
                        <button className={`body-tab ${activeTab === 'files' ? 'active' : ''}`} onClick={() => setActiveTab('files')}>
                            <Layers size={16}/> {t("CurseForgeMod.tab_files")} <span className="tab-badge">{files.length}</span>
                        </button>
                    </div>
                </div>

                <div className="body-content-area custom-scrollbar">
                    {activeTab === 'desc' && (
                        <div className="desc-content-wrapper">
                            <div dangerouslySetInnerHTML={{ __html: description }} className="html-content" />
                        </div>
                    )}

                    {activeTab === 'files' && (
                        <div className="files-content-wrapper" ref={filesSectionRef}>
                            <div className="files-header-tools">
                                <span className="tool-label">{t("CurseForgeMod.filter_version")}</span>
                                <div style={{ width: 180 }}>
                                    <Select
                                        className="ver-select"
                                        size="sm"
                                        value={selectedVersion}
                                        onChange={(v: any) => setSelectedVersion(String(v))}
                                        options={versionOptions}
                                        placeholder={t("CurseForgeMod.all_versions")}
                                    />
                                </div>
                            </div>

                            <div className="files-table-container">
                                <table className="files-table">
                                    <thead>
                                    <tr>
                                        <th style={{width: '40%'}}>{t("CurseForgeMod.table_filename")}</th>
                                        <th style={{width: '20%'}}>{t("CurseForgeMod.table_game_version")}</th>
                                        <th style={{width: '15%'}}>{t("CurseForgeMod.table_size")}</th>
                                        <th style={{width: '15%'}}>{t("CurseForgeMod.table_date")}</th>
                                        <th style={{width: '10%', textAlign: 'right'}}>{t("CurseForgeMod.table_action")}</th>
                                    </tr>
                                    </thead>
                                    <tbody>
                                    {filesLoading ? (
                                        [...Array(6)].map((_, i) => (
                                            <tr key={`sk-${i}`} className="file-skeleton-row">
                                                <td><div className="sk-cell long" /></td>
                                                <td><div className="sk-cell" /></td>
                                                <td><div className="sk-cell short" /></td>
                                                <td><div className="sk-cell" /></td>
                                                <td style={{textAlign: 'right'}}><div className="sk-cell icon" /></td>
                                            </tr>
                                        ))
                                    ) : (
                                        <>
                                            {filteredFiles.map(file => (
                                                <tr key={file.id}>
                                                    <td>
                                                        <div className="file-name-row" data-bm-title={file.displayName}>
                                                            {file.displayName}
                                                        </div>
                                                    </td>
                                                    <td><span className="ver-badge">{getGameVer(file)}</span></td>
                                                    <td className="text-sub">{formatSize(file.fileLength)}</td>
                                                    <td className="text-sub">{new Date(file.fileDate).toLocaleDateString()}</td>
                                                    <td style={{textAlign: 'right'}}>
                                                        <button className="dl-icon-btn" onClick={() => handleDownload(file)} data-bm-title={t("CurseForgeMod.download_this_version")}>
                                                            <Download size={18} />
                                                        </button>
                                                    </td>
                                                </tr>
                                            ))}
                                            {filteredFiles.length === 0 && (
                                                <tr><td colSpan={5} className="empty-row">{t("CurseForgeMod.no_files")}</td></tr>
                                            )}
                                        </>
                                    )}
                                    </tbody>
                                </table>
                            </div>
                        </div>
                    )}
                </div>
            </div>
        </motion.div>
        <CurseForgeInstallModal
            open={installOpen}
            mod={mod as any}
            file={installFile as any}
            onClose={() => { setInstallOpen(false); setInstallFile(null); }}
        />
        </>
    );
};

export default CurseForgeModPage;
