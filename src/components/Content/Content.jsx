import React, { useState, useEffect } from "react";
import LaunchSection from "../LaunchSection/LaunchSection";
import DownloadSection from "../DownloadSection/DownloadSection";
import VersionsSection from "../VersionsSection/VersionsSection";
import SettingsSection from "../SettingsSection/SettingsSection";
import "./Content.css";

function Content({ activeSection }) {
    const [fadeClass, setFadeClass] = useState("fade-in");
    const [displayedSection, setDisplayedSection] = useState(activeSection);

    useEffect(() => {
        setFadeClass("fade-out");

        const timer = setTimeout(() => {
            setDisplayedSection(activeSection); // Change the content after fade out
            setFadeClass("fade-in");
        }, 100); // Adjust to half the animation duration to match transition

        return () => clearTimeout(timer);
    }, [activeSection]);

    const contentClassName = `${fadeClass} ${activeSection === "launch" ? "content no-blur" : "content blur"}`;

    return (
        <div className={contentClassName}>
            {displayedSection === "launch" && <LaunchSection />}
            {displayedSection === "download" && <DownloadSection />}
            {displayedSection === "versions" && <VersionsSection />}
            {displayedSection === "settings" && <SettingsSection />}
        </div>
    );
}

export default Content;
