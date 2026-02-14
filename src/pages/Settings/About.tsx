import React, { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import * as shell from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';
import { ExternalLink, RefreshCw } from 'lucide-react';
import { motion } from "framer-motion";

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
// 之前的版本在 path 上旋转容易导致中心点偏移(wobble)。
// 现在的方案：旋转外层容器，内部只做缩放/透明度动画。
const SciFiLoader = () => (
    <div className="sci-fi-loader">
        <motion.svg
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
            // 核心修复：直接旋转 SVG 整体，确保绝对居中
            animate={{ rotate: 360 }}
            transition={{ duration: 1.5, ease: "linear", repeat: Infinity }}
            style={{ display: 'block' }}
        >
            {/* 外层能量环 - 呼吸式缩放 */}
            <motion.path
                d="M12 2C17.5228 2 22 6.47715 22 12C22 17.5228 17.5228 22 12 22"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeDasharray="4 4" // 增加虚线效果，旋转时更好看
                initial={{ scale: 0.9, opacity: 0.8 }}
                animate={{
                    scale: [0.9, 1.05, 0.9],
                    opacity: [0.8, 1, 0.8]
                }}
                transition={{
                    duration: 1.5, ease: "easeInOut", repeat: Infinity
                }}
                style={{ originX: 0.5, originY: 0.5 }} // 使用相对坐标 0.5 确保居中
            />

            {/* 内层反应环 - 静态或反向缩放 */}
            <motion.path
                d="M12 18C8.68629 18 6 15.3137 6 12C6 8.68629 8.68629 6 12 6"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="round"
                style={{ originX: 0.5, originY: 0.5, opacity: 0.6 }}
            />

            {/* 核心能量点 - 剧烈闪烁 */}
            <motion.circle
                cx="12"
                cy="12"
                r="3"
                fill="currentColor"
                animate={{
                    opacity: [0.3, 1, 0.3],
                    scale: [0.8, 1.2, 0.8]
                }}
                transition={{ duration: 0.8, repeat: Infinity, ease: "easeInOut" }}
            />
        </motion.svg>
    </div>
);

export default function About() {
    const { t } = useTranslation();
    const [appVersion, setAppVersion] = useState("");
    const [appLicense, setAppLicense] = useState("");
    const [tauriVersion, setTauriVersion] = useState("");
    const [webview2Version, setWebview2Version] = useState("");

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
            await checkForUpdates();
        } catch (e) {
            console.error("Manual check failed", e);
        }
    };

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
                            title={checking ? "Checking..." : t("AboutSection.app.official")}
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
                        { img: afdian, title: t("AboutSection.thanks.sponsors"), desc: t("AboutSection.thanks.support"), link: 'https://bmcbl.com/#sponsors', isSquare: true },
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
                            {item.link && (
                                <div className="card-actions">
                                    <IconButton
                                        onClick={() => openInBrowser(item.link!)}
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
        </>
    );
}
