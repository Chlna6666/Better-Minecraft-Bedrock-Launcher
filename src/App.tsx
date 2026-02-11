import React, { Suspense, lazy, useState, useEffect, useCallback } from 'react';
import { Routes, Route, useLocation, Navigate } from 'react-router-dom';
import { listen } from "@tauri-apps/api/event";

// 组件与Hook导入
import { Navbar } from './pages/Titlebar/Navbar.tsx';
import { useUpdaterWithModal } from "./hooks/useUpdaterWithModal.ts";
import { useAppConfig } from './hooks/useAppConfig';
import { getConfig } from "./utils/config";
const UpdateModal = lazy(() => import('./components/UpdateModal'));
const UserAgreement = lazy(() => import("./components/UserAgreement/UserAgreement"));
import BackgroundLayer from "./components/BackgroundLayer"; // [1] 新增引入

import "./App.css";

// 页面导入
import { LaunchPage } from './pages/LaunchPage';
const DownloadPage = lazy(() => import("./pages/DownloadPage.tsx"));
const SettingsPage = lazy(() => import("./pages/Settings/SettingsPage.tsx"));
const ManagePage = lazy(() => import("./pages/Manage/ManagePage.tsx"));
const CurseForgeModPage = lazy(() => import("./pages/Download/CurseForge/CurseForgeModPage.tsx"));
import { useTranslation } from "react-i18next";

const ToolsLayout = lazy(() => import("./pages/Tools/ToolsLayout.tsx"));
const ToolsHome = lazy(() => import("./pages/Tools/ToolsHome.tsx"));
const ToolsOnlinePage = lazy(() => import("./pages/Tools/ToolsOnlinePage.tsx"));

function App() {
    const location = useLocation();
    const { t } = useTranslation();

    // 1. 初始化主题状态
    const [theme, setTheme] = useState<'light' | 'dark'>(() => {
        const savedTheme = localStorage.getItem('app-theme');
        if (savedTheme === 'light' || savedTheme === 'dark') {
            return savedTheme;
        }
        if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
            return 'dark';
        }
        return 'light';
    });

    // 2. 初始化核心配置
    // [2] 修改：解构出 backgroundImageUrl
    const { backgroundImageUrl } = useAppConfig();

    const [autoCheckUpdates, setAutoCheckUpdates] = useState<boolean | null>(null);

    const loadLauncherUpdateConfig = useCallback(async () => {
        const fullConfig: any = await getConfig().catch(() => ({}));
        const launcher = fullConfig.launcher || {};
        const next = launcher.hasOwnProperty('auto_check_updates') ? !!launcher.auto_check_updates : true;
        setAutoCheckUpdates(next);
    }, []);

    useEffect(() => {
        loadLauncherUpdateConfig();
        const unlistenPromise = listen('refresh-config', () => {
            loadLauncherUpdateConfig();
        });
        return () => {
            unlistenPromise.then(unlisten => unlisten());
        };
    }, [loadLauncherUpdateConfig]);

    // 3. 更新检查逻辑
    const {
        modalOpen,
        setModalOpen,
        closeModal,
        newRelease,
        downloading,
        progressSnapshot,
        startDownload,
        cancelDownload
    } = useUpdaterWithModal({
        owner: "Chlna6666",
        repo: "Better-Minecraft-Bedrock-Launcher",
        autoCheck: autoCheckUpdates ?? false,
    });

    // 4. 主题切换函数
    const toggleTheme = useCallback(() => {
        setTheme(prev => {
            const newTheme = prev === 'light' ? 'dark' : 'light';
            localStorage.setItem('app-theme', newTheme);
            return newTheme;
        });
    }, []);

    // 5. 监听 theme 变化并应用到 DOM
    useEffect(() => {
        document.documentElement.setAttribute('data-theme', theme);
    }, [theme]);

    return (
        <>
            {/* [3] 新增：全局背景层
                放在这里可以保证无论路由怎么切换，背景都始终存在且不重绘
             */}
            <BackgroundLayer url={backgroundImageUrl} />

            {/* 全局协议弹窗 */}
            <Suspense fallback={null}>
                <UserAgreement onAccept={undefined} />
            </Suspense>

            {modalOpen && (
                <Suspense fallback={null}>
                    <UpdateModal
                        open={modalOpen}
                        onClose={closeModal}
                        release={newRelease}
                        onDownload={startDownload}
                        downloading={downloading}
                        snapshot={progressSnapshot}
                        onCancel={cancelDownload}
                    />
                </Suspense>
            )}

            {/* 导航栏 - 传入更新状态和打开函数 */}
            {!location.pathname.startsWith('/curseforge/mod/') && (
                <Navbar
                    toggleTheme={toggleTheme}
                    isDark={theme === 'dark'}
                    hasNewVersion={!!newRelease}
                    onOpenUpdate={() => setModalOpen(true)}
                />
            )}

            {/* 路由容器 */}
            <Suspense fallback={null}>
                <Routes location={location} key={location.pathname}>
                    <Route path="/" element={<LaunchPage />} />
                    <Route path="/download" element={<DownloadPage />} />
                    <Route path="/list" element={<ManagePage />} />
                    <Route path="/tools" element={<ToolsLayout />}>
                        <Route index element={<ToolsHome />} />
                        <Route path="online" element={<ToolsOnlinePage />} />
                    </Route>
                    <Route path="/online" element={<Navigate to="/tools/online" replace />} />
                    <Route path="/settings" element={<SettingsPage />} />
                    <Route path="/curseforge/mod/:id" element={<CurseForgeModPage />} />
                </Routes>
            </Suspense>
        </>
    );
}

export default App;
