import React, { useEffect, useLayoutEffect, useMemo, useRef, useState, useCallback } from 'react';
import { NavLink } from 'react-router-dom';
import { useLocation } from 'react-router-dom';
import {
    Home, Download, List, Wrench, Settings,
    Sun, Moon, Minus, X
} from 'lucide-react';
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";

import logo from "../../assets/logo.png";
import './Navbar.css';
import MusicPlayer from './MusicPlayer.tsx';

interface NavbarProps {
    toggleTheme: () => void;
    isDark: boolean;
    hasNewVersion?: boolean;
    onOpenUpdate?: () => void;
}

const navItems = [
    { path: '/', labelKey: 'Sidebar.launch', icon: Home },
    { path: '/download', labelKey: 'Sidebar.download', icon: Download },
    { path: '/list', labelKey: 'Sidebar.versions', icon: List },
    { path: '/tools', labelKey: 'Sidebar.tools', icon: Wrench },
    { path: '/settings', labelKey: 'Sidebar.settings', icon: Settings },
];

export const Navbar: React.FC<NavbarProps> = ({ toggleTheme, isDark, hasNewVersion, onOpenUpdate }) => {
    const { t } = useTranslation();
    const [appVersion, setAppVersion] = useState("1.0.0");
    const appWindow = getCurrentWindow();
    const location = useLocation();

    const capsuleRef = useRef<HTMLDivElement | null>(null);
    const linkRefs = useRef<Record<string, HTMLAnchorElement | null>>({});
    const [pillStyle, setPillStyle] = useState<{ left: number; width: number; opacity: number }>({
        left: 0,
        width: 0,
        opacity: 0,
    });

    const activePath = useMemo(() => {
        const current = location.pathname || '/';
        const match = navItems.find((n) => (n.path === '/' ? current === '/' : current.startsWith(n.path)));
        return match?.path ?? '/';
    }, [location.pathname]);

    const updateActivePill = useCallback(() => {
        const wrapper = capsuleRef.current;
        const el = linkRefs.current[activePath];
        if (!wrapper || !el) return;

        const wrapperRect = wrapper.getBoundingClientRect();
        const elRect = el.getBoundingClientRect();
        setPillStyle({
            left: Math.round(elRect.left - wrapperRect.left),
            width: Math.round(elRect.width),
            opacity: 1,
        });
    }, [activePath]);

    useEffect(() => {
        invoke("get_app_version")
            .then((v) => setAppVersion(v as string))
            .catch((err) => console.error("获取版本失败:", err));
    }, []);

    useLayoutEffect(() => {
        updateActivePill();
    }, [updateActivePill]);

    useEffect(() => {
        const wrapper = capsuleRef.current;
        if (!wrapper) return;

        let rafId = 0;
        const schedule = () => {
            if (rafId) cancelAnimationFrame(rafId);
            rafId = requestAnimationFrame(() => updateActivePill());
        };

        const ro = new ResizeObserver(schedule);
        ro.observe(wrapper);
        window.addEventListener('resize', schedule);

        // In case layout changes after initial mount (e.g., fonts, media queries)
        schedule();

        return () => {
            window.removeEventListener('resize', schedule);
            ro.disconnect();
            if (rafId) cancelAnimationFrame(rafId);
        };
    }, [updateActivePill]);

    const handleMinimize = useCallback(() => appWindow.minimize(), [appWindow]);
    const handleClose = useCallback(() => appWindow.close(), [appWindow]);

    return (
        <nav className="glass navbar-container" data-tauri-drag-region>
            {/* 左侧：Logo + 标题 */}
            <div className="nav-left" data-tauri-drag-region>
                <img src={logo} alt="Logo" className="nav-logo" data-tauri-drag-region />

                <div className="nav-title-group" data-tauri-drag-region>
                    <span className="nav-app-name" data-tauri-drag-region>BMCBL</span>
                    <span className="nav-version" data-tauri-drag-region>v{appVersion}</span>
                </div>

                {/* 更新提示 */}
                <button
                    className={`nav-update-capsule ${hasNewVersion ? 'is-visible' : ''}`}
                    onClick={onOpenUpdate}
                    title={t("Navbar.update_available")}
                    style={{ WebkitAppRegion: 'no-drag' } as any}
                    aria-hidden={!hasNewVersion}
                    tabIndex={hasNewVersion ? 0 : -1}
                >
                    <span className="update-dot"></span>
                    <span className="update-text">{t("Navbar.new_badge")}</span>
                </button>
            </div>

            {/* 中间：胶囊导航 */}
            <div className="nav-capsule-wrapper" ref={capsuleRef}>
                <div
                    className="nav-active-pill"
                    style={{
                        left: pillStyle.left,
                        width: pillStyle.width,
                        opacity: pillStyle.opacity,
                    }}
                />
                {navItems.map((item) => (
                    <NavLink
                        key={item.path}
                        to={item.path}
                        className="nav-link"
                        ref={(el) => {
                            linkRefs.current[item.path] = el;
                        }}
                    >
                        <span className="nav-content">
                            <item.icon size={18} />
                            <span className="nav-label">{t(item.labelKey)}</span>
                        </span>
                    </NavLink>
                ))}
            </div>

            {/* 右侧：工具栏 */}
            <div className="nav-right">
                <MusicPlayer />

                <button onClick={toggleTheme} className="nav-icon-btn theme-btn" title={t("Navbar.toggle_theme")}>
                    {isDark ? <Sun size={18} /> : <Moon size={18} />}
                </button>

                <div className="divider-vertical"></div>

                <button onClick={handleMinimize} className="nav-icon-btn window-btn" title={t("Navbar.minimize")}>
                    <Minus size={18} />
                </button>
                <button onClick={handleClose} className="nav-icon-btn window-btn close-btn" title={t("common.close")}>
                    <X size={18} />
                </button>
            </div>
        </nav>
    );
};
