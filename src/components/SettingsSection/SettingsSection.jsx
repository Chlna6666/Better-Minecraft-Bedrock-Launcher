import React, { useState } from "react";
import "./SettingsSection.css";
import "./Settings.css";
import GameSettings from "./GameSettings";
import CustomizationSettings from "./CustomizationSettings";
import LauncherSettings from "./LauncherSettings";
import AboutSection from "./AboutSection";
import { useTranslation } from 'react-i18next';

function SettingsSection() {
    const { t, i18n } = useTranslation();
    const [activeTab, setActiveTab] = useState("game");
    const [isVisible, setIsVisible] = useState(true);

    const handleTabChange = (newTab) => {
        if (newTab === activeTab) return;

        setIsVisible(false); // 开始淡出当前内容
        setTimeout(() => {
            setActiveTab(newTab);
            setIsVisible(true); // 淡入新内容
        }, 400); // 与动画时间相匹配
    };

    const renderContent = () => {
        switch (activeTab) {
            case "game":
                return <GameSettings />;
            case "customization":
                return <CustomizationSettings />;
            case "launcher":
                return <LauncherSettings />;
            case "about":
                return <AboutSection />;
            default:
                return null;
        }
    };

    return (
        <div className="settings-section">
            <div className="settings-header">
                <button className={`nav-button ${activeTab === "game" ? "active" : ""}`} onClick={() => handleTabChange("game")}>
                    {t('Settings.tabs.game')}
                </button>
                <button className={`nav-button ${activeTab === "customization" ? "active" : ""}`} onClick={() => handleTabChange("customization")}>
                    {t('Settings.tabs.customization')}
                </button>
                <button className={`nav-button ${activeTab === "launcher" ? "active" : ""}`} onClick={() => handleTabChange("launcher")}>
                    {t('Settings.tabs.launcher')}
                </button>
                <button className={`nav-button ${activeTab === "about" ? "active" : ""}`} onClick={() => handleTabChange("about")}>
                    {t('Settings.tabs.about')}
                </button>
            </div>

            <div className={`settings-content ${isVisible ? "bounce-in" : "bounce-out"}`}>
                {renderContent()}
            </div>
        </div>
    );
}

export default SettingsSection;
