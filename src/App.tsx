import { useState, useEffect, useCallback } from 'react';
import { Routes, Route, useLocation } from 'react-router-dom';
import { AnimatePresence } from 'framer-motion';

// 组件与Hook导入
import { Navbar } from './components/Navbar';
import { useUpdaterWithModal } from "./hooks/useUpdaterWithModal.ts";
import { useAppConfig } from './hooks/useAppConfig';
// 引入刚刚重构的更新弹窗组件 (假设放在 components 目录下)
import UpdateModal from './components/UpdateModal';
import UserAgreement from "./components/UserAgreement/UserAgreement";

import "./App.css";

// 页面导入
import { LaunchPage } from './pages/LaunchPage';
import { PageContainer } from './components/PageContainer';
import DownloadPage from "./pages/DownloadPage.tsx";



const ListPage = () => <PageContainer title="账号列表"><p>账号管理界面...</p></PageContainer>;
const ToolsPage = () => <PageContainer title="实用工具"><p>工具箱界面...</p></PageContainer>;
const SettingsPage = () => <PageContainer title="设置"><p>设置界面...</p></PageContainer>;

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

    // 3. 更新检查逻辑 (修改此处)
    // 我们需要解构 Hook 返回的数据，以便传给 UpdateModal
    // 注意：请确保你的 useUpdaterWithModal 返回了这些字段
    // 我们需要解构 Hook 返回的数据，以便传给 UpdateModal
    // 注意：请确保你的 useUpdaterWithModal 返回了这些字段
    const {
        modalOpen,      // 控制弹窗显示
        closeModal,     // 关闭弹窗函数
        newRelease,     // 新版本数据对象
        downloading,    // 是否正在下载
        progressSnapshot,       // 下载进度 (0-100)
        startDownload,  // 开始下载函数
        cancelDownload  // 取消下载函数 (可选)
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

            {/* 导航栏 */}
            <Navbar toggleTheme={toggleTheme} isDark={theme === 'dark'} />

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