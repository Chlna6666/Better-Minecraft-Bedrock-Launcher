import React, { useState } from "react";
import "./LaunchSection.css";
import MusicPlayer from "../Titlebar/MusicPlayer.jsx"; // 引入音乐播放器

function LaunchSection() {
    const [isOpen, setIsOpen] = useState(false);
    const [selectedVersion, setSelectedVersion] = useState("1.20");

    const versions = [
        "1.21.2.2",
        "1.21.0.3",
        "11.45.14"
    ];

    const toggleDropdown = () => {
        setIsOpen(!isOpen);
    };

    const handleVersionSelect = (version) => {
        setSelectedVersion(version);
        setIsOpen(false); // 选择版本后关闭下拉列表
    };

    return (
        <div className="launch-section">
            <div className={`version-selector-container ${isOpen ? "bounce-in" : ""}`}>
                <button className="start-button">
                    <div className="button-content">
                        <span>启动游戏</span>
                        <span className="version">{selectedVersion}</span>
                    </div>
                </button>
                <button className="arrow-button" onClick={toggleDropdown}>
                    <div className={`arrow ${isOpen ? "open" : ""}`}>▲</div>
                </button>
                {isOpen && (
                    <div className="version-list">
                        {versions.map((version) => (
                            <div
                                key={version}
                                className="version-item"
                                onClick={() => handleVersionSelect(version)}
                            >
                                {version}
                            </div>
                        ))}
                    </div>
                )}
            </div>
        </div>
    );
}

export default LaunchSection;
