import React, { lazy } from "react";
import { useTranslation } from "react-i18next";
import Section from "../../components/Section.jsx";

// 如果组件比较重，可以用 React.lazy
const VersionManager = lazy(() => import("./VersionManager.jsx"));
const McPackManager = lazy(() => import("./McPackManager.jsx"));
const McMapManager = lazy(() => import("./McMapManager.jsx"));


export default function ManageSection() {
    const { t } = useTranslation();

    const tabs = [
        { key: "game", label: t("GameManager.game"), component: VersionManager },
        { key: "mcpack", label: t("GameManager.mcpack"), component: McPackManager },
        { key: "map", label: t("GameManager.map"), component: McMapManager },
    ];


    return (
        <Section
            id="manager"
            tabs={tabs}
            defaultActive="game"
            animation="bounce"
            animationDuration={320}
        />
    );
}
