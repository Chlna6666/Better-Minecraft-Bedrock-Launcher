import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Titlebar from "./components/Titlebar/Titlebar";
import Sidebar from "./components/Sidebar/Sidebar";
import Content from "./components/Content/Content";
import { logMessage } from './logger';
import "./App.css";



const appWindow = getCurrentWindow();

function App() {
    const defaultConfig = {
        theme_color: '#90c7a8',
        background_option: 'default',
        local_image_path: '',
        network_image_url: '',
        show_launch_animation: true
    };

    const [config, setConfig] = useState(defaultConfig);

    useEffect(() => {
        async function initializeConfig() {
            try {
                console.log("Attempting to load configuration...");
                const loadedConfig = await invoke("get_custom_style");
                console.log("Configuration loaded:", loadedConfig);
                logMessage('Info', `Configuration loaded: ${JSON.stringify(loadedConfig)}`);
                setConfig(loadedConfig);
                document.documentElement.style.setProperty('--theme-color', loadedConfig.theme_color|| '#90c7a8');
                applyBackgroundStyle(loadedConfig);
                await invoke("show_splashscreen");
                if (loadedConfig.show_launch_animation) {
                    await invoke("show_splashscreen");

                    // 使用 setTimeout 延迟关闭启动画面
                    const timeoutId = setTimeout(async () => {
                        await invoke("close_splashscreen");
                    }, 3000);

                    // 清理定时器
                    return () => clearTimeout(timeoutId);
                } else {
                    // 如果不显示启动动画，立即关闭启动画面
                    await invoke("close_splashscreen");
                }
            } catch (error) {
                console.error('Failed to initialize configuration:', error);
            }
        }

        initializeConfig();
    }, []); // 空依赖数组，确保只在组件挂载时运行一次

    const applyBackgroundStyle = (config) => {
        switch (config.background_option) {
            case 'local':
                document.body.style.backgroundImage = `url(${config.local_image_path})`;
                break;
            case 'network':
                document.body.style.backgroundImage = `url(${config.network_image_url})`;
                break;
            default:
                break;
        }
        document.body.style.backgroundSize = 'cover';
        document.body.style.backgroundPosition = 'center';
        document.body.style.backgroundRepeat = 'no-repeat';
    };

    const [activeSection, setActiveSection] = useState("launch");

    document.addEventListener('contextmenu', event => event.preventDefault());

    const handleMinimize = () => {
        appWindow.minimize();
    };

    const handleClose = () => {
        appWindow.close();
    };

    return (
        <>
            <Titlebar onMinimize={handleMinimize} onClose={handleClose} />
            <div className="app-container">
                <Sidebar activeSection={activeSection} setActiveSection={setActiveSection} />
                <Content activeSection={activeSection} />
            </div>
        </>
    );
}

export default App;
