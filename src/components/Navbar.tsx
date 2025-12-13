import React, { useEffect, useState, useCallback } from 'react';
import { NavLink } from 'react-router-dom';
import { motion } from 'framer-motion';
import {
    Home, Download, List, Wrench, Settings,
    Sun, Moon, Minus, X
} from 'lucide-react';
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import logo from "../assets/logo.png";
import './Navbar.css';
import MusicPlayer from './MusicPlayer'; // 1. 引入播放器

interface NavbarProps {
    toggleTheme: () => void;
    isDark: boolean;
}

const navItems = [
    { path: '/', label: '启动', icon: Home },
    { path: '/download', label: '下载', icon: Download },
    { path: '/list', label: '列表', icon: List },
    { path: '/tools', label: '工具', icon: Wrench },
    { path: '/settings', label: '设置', icon: Settings },
];

export const Navbar: React.FC<NavbarProps> = ({ toggleTheme, isDark }) => {
    const [appVersion, setAppVersion] = useState("1.0.0");
    const appWindow = getCurrentWindow();

    useEffect(() => {
        invoke("get_app_version")
            .then((v) => setAppVersion(v as string))
            .catch((err) => console.error("获取版本失败:", err));
    }, []);

    const handleMinimize = useCallback(() => appWindow.minimize(), [appWindow]);
    const handleClose = useCallback(() => appWindow.close(), [appWindow]);

    return (
        <nav className="glass navbar-container" data-tauri-drag-region>
            {/* 左侧：Logo + 标题 */}
            <div className="nav-left" data-tauri-drag-region>
                <img src={logo} alt="Logo" className="nav-logo"  data-tauri-drag-region/>
                <div className="nav-title-group" data-tauri-drag-region>
                    <span className="nav-app-name" data-tauri-drag-region>BMCBL</span>
                    <span className="nav-version" data-tauri-drag-region>v{appVersion}</span>
                </div>
            </div>

            {/* 中间：胶囊导航 */}
            <div className="nav-capsule-wrapper" >
                {navItems.map((item) => (
                    <NavLink key={item.path} to={item.path} className="nav-link">
                        {({ isActive }) => (
                            <>
                                {isActive && (
                                    <motion.div
                                        className="active-bg"
                                        layoutId="nav-capsule"
                                        transition={{ type: "spring", stiffness: 300, damping: 30 }}
                                    />
                                )}
                                <span className="nav-content">
                                    <item.icon size={18} />
                                    <span className="nav-label">{item.label}</span>
                                </span>
                            </>
                        )}
                    </NavLink>
                ))}
            </div>

            {/* 右侧：工具栏 */}
            <div className="nav-right">
                {/* 2. 插入 MusicPlayer */}
                <MusicPlayer />

                {/* 主题切换 */}
                <button onClick={toggleTheme} className="icon-btn theme-btn" title="切换主题">
                    {isDark ? <Sun size={18} /> : <Moon size={18} />}
                </button>

                <div className="divider-vertical"></div>

                <button onClick={handleMinimize} className="icon-btn window-btn" title="最小化">
                    <Minus size={18} />
                </button>
                <button onClick={handleClose} className="icon-btn window-btn close-btn" title="关闭">
                    <X size={18} />
                </button>
            </div>
        </nav>
    );
};