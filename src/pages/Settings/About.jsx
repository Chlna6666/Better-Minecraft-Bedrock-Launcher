import React, { useEffect, useState } from "react";
import './About.css';
import * as shell from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';

import Chlan6666 from "../../assets/img/about/Chlna6666.jpg";
import github from "../../assets/img/about/github.png";
import MCAPPX from "../../assets/img/about/MCAPPX.webp";
import Tauri from "../../assets/img/about/Tauri.png";
import Fufuha from "../../assets/img/about/Fufuha.jpg";
import Ustiniana1641 from "../../assets/img/about/Ustiniana1641.jpg";
import afdian from "../../assets/img/about/afdian.png";
import logo from "../../assets/logo.png";

import IconButton from "../../components/IconButton.jsx";
import externalLink from "../../assets/feather/external-link.svg";


function About() {
    const { t, i18n } = useTranslation();
    const [loaded, setLoaded] = useState(false);
    const [appVersion, setAppVersion] = useState("");
    const [appLicense, setAppLicense] = useState("");
    const [tauriVersion, setTauriVersion] = useState("");
    const [webview2Version, setWebview2Version] = useState("");

    useEffect(() => {
        invoke("get_app_version").then(setAppVersion).catch(console.error);
        invoke("get_app_license").then(setAppLicense).catch(console.error);
        invoke("get_tauri_sdk_version").then(setTauriVersion).catch(console.error);
        invoke("get_webview2_version").then(setWebview2Version).catch(console.error);
        const timer = setTimeout(() => setLoaded(true), 130);
        return () => clearTimeout(timer);
    }, []);

    const openInBrowser = (url) => {
        shell.open(url);
    };

    // helper to build IconButton with consistent icon size
    const ExternalIcon = ({ alt = 'open' }) => (
        <img src={externalLink} alt={alt} style={{ width: 16, height: 16, display: 'block' }} />
    );

    return (
        <div className="about-settings">
        <div className={`about-section ${loaded ? "loaded-content" : ""}`}>
            {/* 开发者卡片 */}
            <div className={`about-card ${loaded ? "loaded-content" : ""}`}>
                <img src={Chlan6666} alt="icon" />
                <div className="about-card-content">
                    <h4>Chlna6666</h4>
                    <p>{t("AboutSection.dev.description")}</p>
                </div>
                <div className="about-card-buttons">
                    <IconButton
                        onClick={() => openInBrowser('https://afdian.com/a/Chlna6666')}
                        title={t("AboutSection.dev.sponsor")}
                        icon={<ExternalIcon alt={t("AboutSection.dev.sponsor")} />}
                    />
                </div>
            </div>

            {/* 应用信息 */}
            <div className={`about-card ${loaded ? "loaded-content" : ""}`}>
                <img src={logo} alt="icon" style={{ width: "50px", height: "50px", marginRight: "15px", borderRadius: "0%" }} />
                <div className="about-card-content">
                    <h4>{t("AboutSection.app.name")}</h4>
                    <p>{t("AboutSection.app.version", { appVersion, tauriVersion, webview2Version })}</p>
                </div>
                <div className="about-card-buttons">
                    <IconButton
                        onClick={() => openInBrowser('https://bmcbl.com')}
                        title={t("AboutSection.app.official")}
                        icon={<ExternalIcon alt={t("AboutSection.app.official")} />}
                    />
                </div>
            </div>

            {/* 鸣谢部分 */}
            <div className={`special-thanks ${loaded ? "loaded-content" : ""}`}>
                <h3>{t("AboutSection.thanks.title")}</h3>

                {/* Tauri */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={Tauri} alt="icon" />
                    <div className="special-thanks-card-content">
                        <h4>Tauri</h4>
                        <p>{t("AboutSection.thanks.tauri")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <IconButton
                            onClick={() => openInBrowser('https://v2.tauri.app/')}
                            title={t("AboutSection.common.view")}
                            icon={<ExternalIcon alt={t("AboutSection.common.view")} />}
                        />
                    </div>
                </div>

                {/* MCAPPX */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={MCAPPX} alt="icon" />
                    <div className="special-thanks-card-content">
                        <h4>MCAPPX</h4>
                        <p>{t("AboutSection.thanks.MCAPPX")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <IconButton
                            onClick={() => openInBrowser('https://www.mcappx.com/')}
                            title={t("AboutSection.common.view")}
                            icon={<ExternalIcon alt={t("AboutSection.common.view")} />}
                        />
                    </div>
                </div>

                {/* MCMrARM */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src="https://avatars.githubusercontent.com/u/5191659?v=4" alt="icon" />
                    <div className="special-thanks-card-content">
                        <h4>MCMrARM</h4>
                        <p>{t("AboutSection.thanks.mcmrarm")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <IconButton
                            onClick={() => openInBrowser('https://github.com/MCMrARM/mc-w10-versiondb')}
                            title={t("AboutSection.common.view")}
                            icon={<ExternalIcon alt={t("AboutSection.common.view")} />}
                        />
                    </div>
                </div>

                {/* Fufuha */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={Fufuha} alt="icon" />
                    <div className="special-thanks-card-content">
                        <h4>Fufuha</h4>
                        <p>{t("AboutSection.thanks.fufuha")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <IconButton
                            onClick={() => openInBrowser('https://space.bilibili.com/1798893653/')}
                            title={t("AboutSection.common.view")}
                            icon={<ExternalIcon alt={t("AboutSection.common.view")} />}
                        />
                    </div>
                </div>

                {/* Ustiniana1641 */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={Ustiniana1641} alt="icon" />
                    <div className="special-thanks-card-content">
                        <h4>Ustiniana1641</h4>
                        <p>{t("AboutSection.thanks.ustiniana1641")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <IconButton
                            onClick={() => openInBrowser('#')}
                            title={t("AboutSection.common.view")}
                            icon={<ExternalIcon alt={t("AboutSection.common.view")} />}
                        />
                    </div>
                </div>

                {/* GitHub 贡献者 */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={github} alt="icon" />
                    <div className="special-thanks-card-content">
                        <h4>{t("AboutSection.thanks.contributors")}</h4>
                        <p>{t("AboutSection.thanks.community")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <IconButton
                            onClick={() => openInBrowser('https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher/graphs/contributors')}
                            title={t("AboutSection.common.view")}
                            icon={<ExternalIcon alt={t("AboutSection.common.view")} />}
                        />
                    </div>
                </div>

                {/* 赞助者 */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={afdian} alt="icon" />
                    <div className="special-thanks-card-content">
                        <h4>{t("AboutSection.thanks.sponsors")}</h4>
                        <p>{t("AboutSection.thanks.support")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <IconButton
                            onClick={() => openInBrowser('https://bmcbl.com/#sponsors')}
                            title={t("AboutSection.common.view")}
                            icon={<ExternalIcon alt={t("AboutSection.common.view")} />}
                        />
                    </div>
                </div>

                {/* 用户 */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={logo} alt="icon" style={{ width: "40px", height: "40px", marginRight: "15px", borderRadius: "0%" }} />
                    <div className="special-thanks-card-content">
                        <h4>{t("AboutSection.thanks.users")}</h4>
                        <p>{t("AboutSection.thanks.user_support")}</p>
                    </div>
                </div>
            </div>

            {/* 法律声明 */}
            <div className={`legal-notices ${loaded ? "loaded-content" : ""}`}>
                <h3>{t("AboutSection.legal.title")}</h3>

                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4>
                            <a onClick={() => openInBrowser('https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher')}>
                                {t("AboutSection.legal.copyright.title")}
                            </a>
                        </h4>
                        <p>{t("AboutSection.legal.copyright.content")}</p>
                    </div>
                </div>

                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4>
                            <a onClick={() => openInBrowser('https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher')}>
                                {t("AboutSection.legal.agreement.title")}
                            </a>
                        </h4>
                        <p>{t("AboutSection.legal.agreement.content")}</p>
                    </div>
                </div>

                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4>
                            <a onClick={() => openInBrowser('https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher')}>
                                {t("AboutSection.legal.license.title")}
                            </a>
                        </h4>
                        <p>{appLicense}</p>
                    </div>
                </div>
            </div>
        </div>
    </div>
    );
}

export default About;
