import React, { lazy } from "react";
import { useTranslation } from "react-i18next";
import Section from "../../components/Section.jsx";

const DownloadMinecraft = lazy(() => import("./DownloadMinecraft.jsx"));
const DownloadMod = lazy(() => import("./DownloadMod.jsx"));
const DownloadMcPack = lazy(() => import("./DownloadMcPack.jsx"));
const DownloadMap = lazy(() => import("./DownloadMap.jsx"));

export default function DownloadSection() {
    const { t } = useTranslation();

    const tabs = [
        { key: "minecraft", label: t("Download.minecraft"), component: DownloadMinecraft },
        { key: "mod", label: t("Download.mod"), component: DownloadMod },
        { key: "map", label: t("Download.map"), component: DownloadMcPack },
        { key: "mcpack", label: t("Download.mcpack"), component: DownloadMap },
    ];


    return (
        <Section
            id="download"
            tabs={tabs}
            defaultActive="minecraft"
            animation="bounce"
            animationDuration={320}
        />
    );
}
