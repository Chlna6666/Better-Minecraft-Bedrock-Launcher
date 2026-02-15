import React, { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import * as shell from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';
import { ExternalLink, RefreshCw } from 'lucide-react';
import { motion } from "framer-motion";
import { useToast } from "../../components/Toast";

import Chlan6666 from "../../assets/img/about/Chlna6666.jpg";
import github from "../../assets/img/about/github.png";
import MCAPPX from "../../assets/img/about/MCAPPX.webp";
import Tauri from "../../assets/img/about/Tauri.png";
import BedrockLauncherCore from "../../assets/img/about/BedrockLauncher.Core.webp";
import Fufuha from "../../assets/img/about/Fufuha.jpg";
import Ustiniana1641 from "../../assets/img/about/Ustiniana1641.jpg";
import afdian from "../../assets/img/about/afdian.png";
import MCIM from "../../assets/img/about/MCIM.png";
import EasyTier from "../../assets/img/about/easytier.png";
import logo from "../../assets/logo.png";

// 引入组件 (保持不变)
import IconButton from "../../components/IconButton";
import UpdateModal from "../../components/UpdateModal";
import { useUpdaterWithModal } from "../../hooks/useUpdaterWithModal";

// 引入样式
import "./About.css";

// 动画变量 (保持不变)
const pageVariants = {
    initial: { opacity: 0, y: 10, scale: 0.98 },
    animate: {
        opacity: 1,
        y: 0,
        scale: 1,
        transition: { duration: 0.4, ease: [0.25, 1, 0.5, 1], staggerChildren: 0.05 }
    },
    exit: {
        opacity: 0,
        y: -10,
        scale: 0.98,
        transition: { duration: 0.2 }
    }
};

const itemVariants = {
    initial: { opacity: 0, y: 10 },
    animate: { opacity: 1, y: 0, transition: { type: "spring", stiffness: 300, damping: 30 } }
};

// --- 修复：更稳定的科幻加载组件 ---
// About 页面用一个更克制的 spinner：纯 SVG + CSS 动画，避免 framer-motion 造成的“抖动感”。
const SciFiLoader = () => (
    <svg className="sci-fi-loader-svg" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
        <circle className="sci-fi-loader-track" cx="12" cy="12" r="9" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2" />
        <circle className="sci-fi-loader-head" cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
    </svg>
);

type SponsorItem = {
    all_sum_amount: string;
    user: { avatar: string; name: string; user_id: string };
};

type FetchRemoteResponse = {
    status: number;
    headers: Record<string, string>;
    body: string;
};

export default function About() {
    const { t } = useTranslation();
    const toast = useToast();
    const [appVersion, setAppVersion] = useState("");
    const [appLicense, setAppLicense] = useState("");
    const [tauriVersion, setTauriVersion] = useState("");
    const [webview2Version, setWebview2Version] = useState("");

    const [sponsorsOpen, setSponsorsOpen] = useState(false);
    const [sponsorsLoading, setSponsorsLoading] = useState(false);
    const [sponsorsError, setSponsorsError] = useState<string | null>(null);
    const [sponsors, setSponsors] = useState<SponsorItem[]>([]);
    const [sponsorsVisible, setSponsorsVisible] = useState(24);
    const sponsorsSentinelRef = useRef<HTMLDivElement | null>(null);
    const sponsorsReqIdRef = useRef(0);

    // --- 集成更新 Hook ---
    const {
        modalOpen,
        setModalOpen, // [重要] 需要从 hook 中解构出 setModalOpen
        closeModal,
        newRelease,
        downloading,
        progressSnapshot,
        startDownload,
        cancelDownload,
        checkForUpdates,
        checking
    } = useUpdaterWithModal({
        owner: "Chlna6666",
        repo: "Better-Minecraft-Bedrock-Launcher",
        autoCheck: false,
        autoOpen: true
    });

    useEffect(() => {
        invoke("get_app_version").then(v => setAppVersion(v as string)).catch(console.error);
        invoke("get_app_license").then(v => setAppLicense(v as string)).catch(console.error);
        invoke("get_tauri_sdk_version").then(v => setTauriVersion(v as string)).catch(console.error);
        invoke("get_webview2_version").then(v => setWebview2Version(v as string)).catch(console.error);
    }, []);

    const openInBrowser = (url: string) => {
        if (url) shell.open(url);
    };

    const LinkIcon = () => <ExternalLink size={16} />;

    // --- 修复：弹窗重开逻辑 ---
    const handleCheckUpdate = async () => {
        // 如果已经检测到有新版本，且弹窗未打开，直接强制打开弹窗
        // 避免因为 newRelease 数据没变导致 useEffect 不触发
        if (newRelease && !modalOpen && setModalOpen) {
            setModalOpen(true);
            return;
        }

        try {
            const resp: any = await checkForUpdates();
            const available =
                typeof resp?.update_available === 'boolean'
                    ? resp.update_available
                    : !!newRelease;

            if (!available) {
                toast.info(t("AboutSection.update.no_update"));
            }
        } catch (e) {
            console.error("Manual check failed", e);
        }
    };

    const openSponsors = () => setSponsorsOpen(true);
    const closeSponsors = () => {
        sponsorsReqIdRef.current += 1;
        setSponsorsOpen(false);
    };

    useEffect(() => {
        if (!sponsorsOpen) return;
        const onKeyDown = (e: KeyboardEvent) => {
            if (e.key === 'Escape') closeSponsors();
        };
        document.addEventListener('keydown', onKeyDown);
        return () => document.removeEventListener('keydown', onKeyDown);
    }, [sponsorsOpen]);

    const loadSponsors = async () => {
        const reqId = (sponsorsReqIdRef.current += 1);
        setSponsorsLoading(true);
        setSponsorsError(null);

        try {
            const resp = await invoke<FetchRemoteResponse>("fetch_remote", {
                url: "https://api.chlna6666.com/sponsors",
                options: {
                    timeout_ms: 15000,
                    allow_redirects: true,
                    allowed_hosts: ["api.chlna6666.com"]
                }
            });

            if (reqId !== sponsorsReqIdRef.current) return;

            if (!resp || resp.status < 200 || resp.status >= 300) {
                throw new Error(`HTTP ${resp?.status ?? "?"}`);
            }

            const json = JSON.parse(resp.body);
            const list: SponsorItem[] = Array.isArray(json?.data) ? json.data : [];
            list.sort((a, b) => (parseFloat(b?.all_sum_amount || '0') - parseFloat(a?.all_sum_amount || '0')));
            setSponsors(list);
        } catch (e: any) {
            if (reqId !== sponsorsReqIdRef.current) return;
            setSponsorsError(String(e?.message || e));
        } finally {
            if (reqId !== sponsorsReqIdRef.current) return;
            setSponsorsLoading(false);
        }
    };

    useEffect(() => {
        if (!sponsorsOpen) return;
        if (sponsorsLoading) return;
        if (sponsors.length > 0) return;
        void loadSponsors();
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [sponsorsOpen]);

    useEffect(() => {
        if (!sponsorsOpen) return;
        setSponsorsVisible(24);
    }, [sponsorsOpen]);

    useEffect(() => {
        if (!sponsorsOpen) return;
        const el = sponsorsSentinelRef.current;
        if (!el) return;

        const obs = new IntersectionObserver((entries) => {
            const e = entries[0];
            if (!e?.isIntersecting) return;
            setSponsorsVisible((v) => Math.min(v + 24, sponsors.length || v + 24));
        }, { root: el.closest('.about-modal-body') as Element | null, threshold: 0.1 });

        obs.observe(el);
        return () => obs.disconnect();
    }, [sponsorsOpen, sponsors.length]);

    const visibleSponsors = useMemo(() => sponsors.slice(0, sponsorsVisible), [sponsors, sponsorsVisible]);

    return (
        <>
            <motion.div
                className="settings-inner-container about-section"
                variants={pageVariants}
                initial="initial"
                animate="animate"
                exit="exit"
            >
                <motion.h3 className="settings-group-title">{t("Settings.tabs.about")}</motion.h3>

                {/* --- 开发者卡片 --- */}
                <motion.div variants={itemVariants} className="about-card developer-card">
                    <img src={Chlan6666} alt="Dev Icon" className="card-icon" />
                    <div className="card-content">
                        <h4>Chlna6666</h4>
                        <p>{t("AboutSection.dev.description")}</p>
                    </div>
                    <div className="card-actions">
                        <IconButton
                            onClick={() => openInBrowser('https://afdian.com/a/Chlna6666')}
                            title={t("AboutSection.dev.sponsor")}
                            icon={<LinkIcon />}
                        />
                    </div>
                </motion.div>

                {/* --- 应用信息卡片 --- */}
                <motion.div variants={itemVariants} className="about-card app-card">
                    <img src={logo} alt="App Icon" className="card-icon square" />
                    <div className="card-content">
                        <h4>Better-Minecraft-Bedrock-Launcher</h4>
                        <p className="version-info">
                            <span>v{appVersion}</span>
                            <span className="divider">|</span>
                            <span className="tech-detail">Tauri {tauriVersion}</span>
                            <span className="divider">|</span>
                            <span className="tech-detail">WebView2 {webview2Version}</span>
                        </p>
                    </div>
                    <div className="card-actions">
                        <IconButton
                            onClick={handleCheckUpdate}
                            title={checking ? t("AboutSection.app.checking") : t("AboutSection.app.official")}
                            // 使用 key 强制 icon 在切换时重新渲染，避免动画卡住
                            icon={checking ? <SciFiLoader key="loader" /> : <RefreshCw key="icon" size={16} />}
                            disabled={checking}
                            className={checking ? "is-checking" : ""}
                        />
                    </div>
                </motion.div>

                <motion.h4 variants={itemVariants} className="settings-sub-title">{t("AboutSection.thanks.title")}</motion.h4>

                {/* --- 鸣谢部分 (Grid 布局) --- */}
                <div className="special-thanks-grid">
                    {[
                        { img: Tauri, title: "Tauri", desc: t("AboutSection.thanks.tauri"), link: 'https://v2.tauri.app/', isSquare: true },
                        { img: MCAPPX, title: "MCAPPX", desc: t("AboutSection.thanks.MCAPPX"), link: 'https://www.mcappx.com/', isSquare: true },
                        { img: MCIM, title: "MCIM", desc: t("AboutSection.thanks.mcim"), link: null, isSquare: true },
                        { img: EasyTier, title: "EasyTier", desc: t("AboutSection.thanks.easytier"), link: 'https://github.com/EasyTier/EasyTier', isSquare: true },
                        {
                            img: BedrockLauncherCore,
                            title: "BedrockLauncher.Core",
                            desc: t("AboutSection.thanks.bl_core"),
                            link: 'https://github.com/Round-Studio/BedrockLauncher.Core',
                            isSquare: true
                        },
                        { img: "https://avatars.githubusercontent.com/u/5191659?v=4", title: "MCMrARM", desc: t("AboutSection.thanks.mcmrarm"), link: 'https://github.com/MCMrARM/mc-w10-versiondb' },
                        { img: Fufuha, title: "Fufuha", desc: t("AboutSection.thanks.fufuha"), link: 'https://space.bilibili.com/1798893653/' },
                        { img: Ustiniana1641, title: "Ustiniana1641", desc: t("AboutSection.thanks.ustiniana1641"), link: '#' },
                        { img: github, title: t("AboutSection.thanks.contributors"), desc: t("AboutSection.thanks.community"), link: 'https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher/graphs/contributors', isSquare: true },
                        { img: afdian, title: t("AboutSection.thanks.sponsors"), desc: t("AboutSection.thanks.support"), action: openSponsors, isSquare: true },
                        { img: logo, title: t("AboutSection.thanks.users"), desc: t("AboutSection.thanks.user_support"), link: null, isSquare: true, isSmall: true }
                    ].map((item, idx) => (
                        <motion.div
                            key={idx}
                            className="special-thanks-card"
                            variants={itemVariants}
                        >
                            <img
                                src={item.img}
                                alt={item.title}
                                className={`card-icon ${item.isSquare ? 'square' : ''} ${item.isSmall ? 'small' : ''}`}
                            />
                            <div className="card-content">
                                <h4>{item.title}</h4>
                                <p>{item.desc}</p>
                            </div>
                            {(item.link || (item as any).action) && (
                                <div className="card-actions">
                                    <IconButton
                                        onClick={() => ((item as any).action ? (item as any).action() : openInBrowser(item.link!))}
                                        title={t("AboutSection.common.view")}
                                        icon={<LinkIcon />}
                                    />
                                </div>
                            )}
                        </motion.div>
                    ))}
                </div>

                <motion.h4 variants={itemVariants} className="settings-sub-title">{t("AboutSection.legal.title")}</motion.h4>

                {/* --- 法律信息 --- */}
                <motion.div variants={itemVariants} className="legal-notices">
                    {[
                        { title: t("AboutSection.legal.copyright.title"), content: t("AboutSection.legal.copyright.content"), link: 'https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher' },
                        { title: t("AboutSection.legal.agreement.title"), content: t("AboutSection.legal.agreement.content"), link: null },
                        { title: t("AboutSection.legal.license.title"), content: appLicense, link: null }
                    ].map((item, idx) => (
                        <div key={idx} className="legal-notices-card">
                            <div className="card-content">
                                <h4>
                                    {item.link ? (
                                        <a onClick={() => openInBrowser(item.link!)}>
                                            {item.title}
                                        </a>
                                    ) : (
                                        <span>{item.title}</span>
                                    )}
                                </h4>
                                <p className="legal-text">{item.content}</p>
                            </div>
                        </div>
                    ))}
                </motion.div>
            </motion.div>

            {/* --- Portal 弹窗 --- */}
            {createPortal(
                <UpdateModal
                    open={modalOpen}
                    onClose={closeModal}
                    release={newRelease}
                    onDownload={startDownload}
                    downloading={downloading}
                    snapshot={progressSnapshot}
                    onCancel={cancelDownload}
                />,
                document.body
            )}

            {sponsorsOpen && createPortal(
                <div
                    className="about-modal-overlay"
                    role="dialog"
                    aria-modal="true"
                    onMouseDown={(e) => {
                        if (e.target === e.currentTarget) closeSponsors();
                    }}
                >
                    <div className="about-modal">
                        <div className="about-modal-header">
                            <div className="about-modal-title">{t("AboutSection.sponsors.title")}</div>
                            <div className="about-modal-actions">
                                <IconButton
                                    onClick={() => openInBrowser('https://afdian.com/a/Chlna6666')}
                                    title={t("AboutSection.sponsors.support_link")}
                                    icon={<LinkIcon />}
                                />
                                <IconButton
                                    onClick={closeSponsors}
                                    title={t("common.close")}
                                    icon={<span style={{ fontSize: 18, lineHeight: 1 }}>×</span>}
                                />
                            </div>
                        </div>
                        <div className="about-modal-body">
                            {sponsorsLoading ? (
                                <div className="about-modal-empty">
                                    <SciFiLoader />
                                    <div>{t("AboutSection.sponsors.loading")}</div>
                                </div>
                            ) : sponsorsError ? (
                                <div className="about-modal-empty">
                                    <div>{t("AboutSection.sponsors.error")}</div>
                                    <div style={{ opacity: 0.75, fontSize: 12 }}>{sponsorsError}</div>
                                    <button className="about-modal-retry" onClick={() => void loadSponsors()}>
                                        {t("retry")}
                                    </button>
                                </div>
                            ) : visibleSponsors.length === 0 ? (
                                <div className="about-modal-empty">
                                    <div>{t("AboutSection.sponsors.empty")}</div>
                                </div>
                            ) : (
                                <>
                                    <div className="sponsor-grid">
                                        {visibleSponsors.map((s) => (
                                            <div className="sponsor-item" key={s.user.user_id}>
                                                <img className="sponsor-avatar" src={s.user.avatar} alt={s.user.name} loading="lazy" referrerPolicy="no-referrer" />
                                                <div className="sponsor-meta">
                                                    <div className="sponsor-name" title={s.user.name}>{s.user.name}</div>
                                                    <div className="sponsor-amount">¥{s.all_sum_amount}</div>
                                                </div>
                                            </div>
                                        ))}
                                    </div>
                                    <div ref={sponsorsSentinelRef} style={{ height: 1 }} />
                                </>
                            )}
                        </div>
                    </div>
                </div>,
                document.body
            )}
        </>
    );
}
