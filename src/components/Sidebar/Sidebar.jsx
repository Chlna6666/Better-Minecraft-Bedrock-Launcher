import React, { useCallback } from "react";

import "./Sidebar.css";
import launchIcon from "../../assets/feather/home.svg";
import downloadIcon from "../../assets/feather/download.svg";
import versionsIcon from "../../assets/feather/list.svg";
import toolsIcon from "../../assets/feather/tool.svg";
import settingsIcon from "../../assets/feather/settings.svg";
import { useTranslation } from 'react-i18next';


/**
 * @param {{
 *  activeSection: string,
 *  setActiveSection: (section: string) => void,
 *  disableSwitch?: boolean // 当正在下载或弹窗显示时，禁止切换
 * }} props
 */
function Sidebar({ activeSection, setActiveSection, disableSwitch = false }) {
    const { t, i18n } = useTranslation();
    // 稳定的点击回调，避免重建
    const handleClick = useCallback(
        (section) => () => {
            // 禁止切换时直接返回
            if (disableSwitch) return;
            if (section !== activeSection) {
                setActiveSection(section);
            }
        },
        [activeSection, setActiveSection, disableSwitch]
    );
    const OPTIONS = [
        { label: t('Sidebar.launch'), icon: launchIcon,   section: 'launch'   },
        { label: t('Sidebar.download'), icon: downloadIcon, section: 'download' },
        { label: t('Sidebar.versions'), icon: versionsIcon, section: 'versions' },
        { label: t('Sidebar.tools'),    icon: toolsIcon,    section: 'tools'    },
    ];

    return (
        <div className={`sidebar expanded${disableSwitch ? " disabled" : ""}`}>
            <div className="sidebar-options">
                {OPTIONS.map(({ label, icon, section }) => {
                    const isActive = activeSection === section;
                    const optionClass =
                        "sidebar-option" +
                        (isActive ? " active" : "") +
                        (disableSwitch ? " disabled" : "");

                    return (
                        <div
                            key={section}
                            className={optionClass}
                            onClick={handleClick(section)}
                        >
              <span className="sidebar-icon">
                <img src={icon} alt={`${label} icon`} />
              </span>
                            <span className="sidebar-label">{label}</span>
                        </div>
                    );
                })}
            </div>

            {/* Settings 固定在底部 */}
            <div
                className={
                    "sidebar-option sidebar-settings" +
                    (activeSection === "settings" ? " active" : "") +
                    (disableSwitch ? " disabled" : "")
                }
                onClick={handleClick("settings")}
            >
        <span className="sidebar-icon">
          <img src={settingsIcon} alt="设置 icon" />
        </span>
                <span className="sidebar-label">{t('Sidebar.settings')}</span>
            </div>
        </div>
    );
}

export default React.memo(Sidebar);
