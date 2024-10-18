import React from "react";
import "./Sidebar.css";
import launchIcon from "../../assets/feather/home.svg";
import downloadIcon from "../../assets/feather/download.svg";
import versionsIcon from "../../assets/feather/list.svg";
import settingsIcon from "../../assets/feather/settings.svg";
import toolsIcon from "../../assets/feather/tool.svg"; // 添加工具的图标

function Sidebar({ activeSection, setActiveSection }) {
    const options = [
        { label: "启动", icon: launchIcon, section: "launch" },
        { label: "下载", icon: downloadIcon, section: "download" },
        { label: "列表", icon: versionsIcon, section: "versions" },
        { label: "工具", icon: toolsIcon, section: "tools" }, // 新增的工具选项
    ];

    return (
        <div className="sidebar expanded">
            <div className="sidebar-options">
                {options.map((option) => (
                    <div
                        key={option.section}
                        className={`sidebar-option ${activeSection === option.section ? "active" : ""}`}
                        onClick={() => setActiveSection(option.section)}
                    >
                        <span className="sidebar-icon">
                            <img src={option.icon} alt={`${option.label} icon`} />
                        </span>
                        <span className="sidebar-label">{option.label}</span>
                    </div>
                ))}
            </div>
            {/* Settings option placed at the bottom */}
            <div
                className={`sidebar-option sidebar-settings ${activeSection === "settings" ? "active" : ""}`}
                onClick={() => setActiveSection("settings")}
            >
                <span className="sidebar-icon">
                    <img src={settingsIcon} alt="设置 icon" />
                </span>
                <span className="sidebar-label">设置</span>
            </div>
        </div>
    );
}

export default Sidebar;
