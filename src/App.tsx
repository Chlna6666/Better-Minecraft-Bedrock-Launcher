import { useState, useEffect, useCallback } from 'react';
import { Routes, Route, useLocation } from 'react-router-dom';
import { AnimatePresence } from 'framer-motion';

// 组件与Hook导入
import { Navbar } from './components/Navbar';
import { useUpdaterWithModal } from "./hooks/useUpdaterWithModal.ts";
import { useAppConfig } from './hooks/useAppConfig';
import UpdateModal from './components/UpdateModal';
import UserAgreement from "./components/UserAgreement/UserAgreement";

import "./App.css";

// 页面导入
import { LaunchPage } from './pages/LaunchPage';
import { PageContainer } from './components/PageContainer';
import DownloadPage from "./pages/DownloadPage.tsx";
import SettingsPage from "./pages/Settings/SettingsPage.tsx";

const ListPage = () => <PageContainer title="账号列表"><p>账号管理界面...</p></PageContainer>;
const ToolsPage = () => <PageContainer title="实用工具"><p>工具箱界面...</p></PageContainer>;

function App() {
    const location = useLocation();

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
    useAppConfig();

    // 3. 更新检查逻辑
    const {
        modalOpen,
        setModalOpen, // [新增] 解构出 setModalOpen
        closeModal,
        newRelease,
        downloading,
        progressSnapshot,
        startDownload,
        cancelDownload
    } = useUpdaterWithModal({
        owner: "Chlna6666",
        repo: "Better-Minecraft-Bedrock-Launcher",
        autoCheck: true,
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
            {/* 全局协议弹窗 */}
            <UserAgreement onAccept={undefined} />

            <UpdateModal
                open={modalOpen}
                onClose={closeModal}
                release={newRelease}
                onDownload={startDownload}
                downloading={downloading}
                snapshot={progressSnapshot}
                onCancel={cancelDownload}
            />

            {/* 导航栏 - 传入更新状态和打开函数 */}
            <Navbar
                toggleTheme={toggleTheme}
                isDark={theme === 'dark'}
                hasNewVersion={!!newRelease} // [新增] 只有当 newRelease 存在时为 true
                onOpenUpdate={() => setModalOpen(true)} // [新增] 打开更新弹窗
            />

            {/* 路由容器 */}
            <AnimatePresence mode="wait">
                <Routes location={location} key={location.pathname}>
                    <Route path="/" element={<LaunchPage />} />
                    <Route path="/download" element={<DownloadPage />} />
                    <Route path="/list" element={<ListPage />} />
                    <Route path="/tools" element={<ToolsPage />} />
                    <Route path="/settings" element={<SettingsPage />} />
                </Routes>
            </AnimatePresence>
        </>
    );
}

export default App;