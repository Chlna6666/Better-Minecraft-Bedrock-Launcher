import React, { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { useTranslation } from 'react-i18next';
import "./DownloadSection.css";

import DownloadMinecraft from "./DownloadMinecraft.jsx";
import DownloadMod from "./DownloadMod.jsx";
import DownloadMcPack from "./DownloadMcPack.jsx";
import DownloadMap from "./DownloadMap.jsx";

function DownloadSection({ onStatusChange }) {
    const { t } = useTranslation();
    const [activeTab, setActiveTab] = useState("DownloadMinecraft");
    const [isVisible, setIsVisible] = useState(true);
    const [isChildDownloading, setIsChildDownloading] = useState(false);

    const timerRef = useRef(null);
    const contentRef = useRef(null);

    // 将子组件下载状态上报给父组件
    useEffect(() => {
        onStatusChange && onStatusChange(isChildDownloading);
    }, [isChildDownloading, onStatusChange]);

    // 清理定时器（组件卸载）
    useEffect(() => {
        return () => {
            if (timerRef.current) {
                clearTimeout(timerRef.current);
                timerRef.current = null;
            }
        };
    }, []);

    // 平滑切换 tab 的 handler，使用 timerRef 来控制并清理定时器
    const handleTabChange = useCallback(
        (newTab) => {
            if (newTab === activeTab || isChildDownloading) return;
            setIsVisible(false);
            if (timerRef.current) {
                clearTimeout(timerRef.current);
            }
            timerRef.current = setTimeout(() => {
                setActiveTab(newTab);
                setIsVisible(true);
                timerRef.current = null;
            }, 400);
        },
        [activeTab, isChildDownloading]
    );

    // 为了解决 "Unable to preventDefault inside passive event listener"（来自父容器上的 onWheel），
    // 我们在这里为 contentRef 添加一个非被动的 wheel 监听器并阻止滚轮（当 isChildDownloading 时）。
    useEffect(() => {
        const el = contentRef.current;
        if (!el) return;
        const onWheel = (e) => {
            if (isChildDownloading) {
                e.preventDefault();
            }
        };
        el.addEventListener("wheel", onWheel, { passive: false });
        return () => el.removeEventListener("wheel", onWheel, { passive: false });
    }, [isChildDownloading]);

    const renderContent = useMemo(() => {
        const commonProps = { onStatusChange: setIsChildDownloading };
        switch (activeTab) {
            case "DownloadMinecraft":
                return <DownloadMinecraft {...commonProps} />;
            case "DownloadMod":
                return <DownloadMod {...commonProps} />;
            case "DownloadMcPack":
                return <DownloadMcPack {...commonProps} />;
            case "DownloadMap":
                return <DownloadMap {...commonProps} />;
            default:
                return null;
        }
    }, [activeTab]);

    return (
        <div
            className="settings-section"
            style={{
                // 下载时禁止滚动（额外保险：overflow hidden）
                overflow: isChildDownloading ? "hidden" : "auto",
                pointerEvents: isChildDownloading ? "none" : "auto",
            }}
        >
            <div className="settings-header">
                {[
                    "DownloadMinecraft",
                    "DownloadMod",
                    "DownloadMap",
                    "DownloadMcPack",
                ].map((tab) => (
                    <button
                        key={tab}
                        className={`nav-button ${activeTab === tab ? "active" : ""}`}
                        onClick={() => handleTabChange(tab)}
                        disabled={isChildDownloading}
                    >
                        {tab === "DownloadMinecraft" && t('Download.minecraft')}
                        {tab === "DownloadMod"       && t('Download.mod')}
                        {tab === "DownloadMap"       && t('Download.map')}
                        {tab === "DownloadMcPack"    && t('Download.mcpack')}
                    </button>
                ))}
            </div>

            <div
                ref={contentRef}
                className={`download-content ${isVisible ? "bounce-in" : "bounce-out"}`}
            >
                {renderContent}
            </div>
        </div>
    );
}

export default React.memo(DownloadSection);
