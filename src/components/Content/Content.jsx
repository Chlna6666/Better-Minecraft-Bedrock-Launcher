import React, { useState, useEffect, useMemo } from "react";
import LaunchSection from "../LaunchSection/LaunchSection";
import DownloadSection from "../DownloadSection/DownloadSection.jsx";
import VersionsSection from "../VersionsSection/VersionsSection";
import SettingsSection from "../SettingsSection/SettingsSection";
import "./Content.css";

function Content({ activeSection, disableSwitch,onStatusChange }) {
    const [displayedSection, setDisplayedSection] = useState(activeSection);
    const [isFading, setIsFading] = useState(false);

    useEffect(() => {
        // 如果 disableSwitch 为 true，则不做任何切换
        if (disableSwitch) return;

        if (activeSection !== displayedSection) {
            setIsFading(true);
            const timer = setTimeout(() => {
                setDisplayedSection(activeSection);
                setIsFading(false);
            }, 100);
            return () => clearTimeout(timer);
        }
    }, [activeSection, displayedSection, disableSwitch]);

    const contentClassName = useMemo(() => {
        const blurClass = activeSection === "launch" ? "no-blur" : "blur";
        const fadeClass = isFading ? "fade-out" : "fade-in";
        return `${fadeClass} content ${blurClass}`;
    }, [activeSection, isFading]);

    const renderSection = useMemo(() => {
        switch (displayedSection) {
            case "launch":
                return <LaunchSection />;
            case "download":
                return <DownloadSection
                    onStatusChange={onStatusChange}
                />;
            case "versions":
                return <VersionsSection />;
            case "settings":
                return <SettingsSection />;
            default:
                return null;
        }
    }, [displayedSection]);

    return (
        <div className={contentClassName}>
            {renderSection}
        </div>
    );
}

export default React.memo(Content);
