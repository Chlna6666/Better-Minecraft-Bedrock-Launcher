import React, { useEffect, useState } from "react";
import './AboutSection.css';
import * as shell from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';

import Chlan6666 from "../../assets/img/about/Chlna6666.jpg";
import github from "../../assets/img/about/github.png";
import Tauri from "../../assets/img/about/Tauri.png";
import Fufuha from "../../assets/img/about/Fufuha.jpg";
import afdian from "../../assets/img/about/afdian.png";
import logo from "../../assets/logo.png";


function AboutSection() {
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

    return (
        <div className={`about-section ${loaded ? "loaded-content" : ""}`}>
            {/* 开发者卡片 */}
            <div className={`about-card ${loaded ? "loaded-content" : ""}`}>
                <img src={Chlan6666} alt="icon" />
                <div className="about-card-content">
                    <h4>Chlna6666</h4>
                    <p>{t("AboutSection.dev.description")}</p>
                </div>
                <div className="about-card-buttons">
                    <button onClick={() => openInBrowser('https://afdian.com/a/Chlna6666')}>
                        {t("AboutSection.dev.sponsor")}
                    </button>
                </div>
            </div>

            {/* 应用信息 */}
            <div className={`about-card ${loaded ? "loaded-content" : ""}`}>
                <img src={logo} alt="icon" style={{width: "50px", height: "50px", marginRight: "15px", borderRadius: "0%"}}/>
                <div className="about-card-content">
                    <h4>{t("AboutSection.app.name")}</h4>
                    <p>{t("AboutSection.app.version", { appVersion, tauriVersion, webview2Version })}</p>
                </div>
                <div className="about-card-buttons">
                    <button onClick={() => openInBrowser('https://bmcbl.com')}>
                        {t("AboutSection.app.official")}
                    </button>
                </div>
            </div>

            {/* 鸣谢部分 */}
            <div className={`special-thanks ${loaded ? "loaded-content" : ""}`}>
                <h3>{t("AboutSection.thanks.title")}</h3>

                {/* Tauri */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={Tauri} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>Tauri</h4>
                        <p>{t("AboutSection.thanks.tauri")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://v2.tauri.app/')}>
                            {t("AboutSection.common.view")}
                        </button>
                    </div>
                </div>

                {/* MCMrARM */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src="https://avatars.githubusercontent.com/u/5191659?v=4" alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>MCMrARM</h4>
                        <p>{t("AboutSection.thanks.mcmrarm")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://github.com/MCMrARM/mc-w10-versiondb')}>
                            {t("AboutSection.common.view")}
                        </button>
                    </div>
                </div>

                {/* Fufuha */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={Fufuha} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>Fufuha</h4>
                        <p>{t("AboutSection.thanks.fufuha")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://space.bilibili.com/1798893653/')}>{t("AboutSection.common.view")}</button>
                    </div>
                </div>

                {/* GitHub 贡献者 */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={github} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>{t("AboutSection.thanks.contributors")}</h4>
                        <p>{t("AboutSection.thanks.community")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher/graphs/contributors')}>
                            {t("AboutSection.common.view")}
                        </button>
                    </div>
                </div>

                {/* 赞助者 */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={afdian} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>{t("AboutSection.thanks.sponsors")}</h4>
                        <p>{t("AboutSection.thanks.support")}</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://bmcbl.com/#sponsors')}>
                            {t("AboutSection.common.view")}
                        </button>
                    </div>
                </div>

                {/* 用户 */}
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={logo} alt="icon" style={{width: "50px", height: "50px", marginRight: "15px", borderRadius: "0%"}}/>
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
                        <h4><a onClick={() => openInBrowser('https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher')}>{t("AboutSection.legal.copyright.title")}</a></h4>
                        <p>{t("AboutSection.legal.copyright.content")}</p>
                    </div>
                </div>

                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4><a onClick={() => openInBrowser('https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher')}>{t("AboutSection.legal.agreement.title")}</a></h4>
                        <p>{t("AboutSection.legal.agreement.content")}</p>
                    </div>
                </div>

                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4><a onClick={() => openInBrowser('https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher')}>{t("AboutSection.legal.license.title")}</a></h4>
                        <p>{appLicense}</p>
                    </div>
                </div>
            </div>
        </div>
    );
}

export default AboutSection;
