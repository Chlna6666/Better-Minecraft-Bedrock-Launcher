// Settings.jsx
import React, { lazy } from "react";
import { useTranslation } from "react-i18next";
import Section from "../../components/Section.jsx";
import "./Settings.css";

// 如果组件比较重，可以用 React.lazy
const GameSettings = lazy(() => import("./Game.jsx"));
const CustomizationSettings = lazy(() => import("./Customization.jsx"));
const LauncherSettings = lazy(() => import("./Launcher.jsx"));
const AboutSection = lazy(() => import("./About.jsx"));

export default function SettingsSection() {
    const { t } = useTranslation();

    const tabs = [
        { key: "game", label: t("Settings.tabs.game"), component: GameSettings },
        { key: "customization", label: t("Settings.tabs.customization"), component: CustomizationSettings },
        { key: "launcher", label: t("Settings.tabs.launcher"), component: LauncherSettings },
        { key: "about", label: t("Settings.tabs.about"), component: AboutSection },
    ];


    return (
        <Section
            id="settings"
            tabs={tabs}
            defaultActive="game"
            animation="bounce"
            animationDuration={320}
        />
    );
}
