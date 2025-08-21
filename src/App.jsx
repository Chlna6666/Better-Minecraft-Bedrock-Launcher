import React, {useState, useEffect, useCallback, useMemo, lazy, Suspense, useTransition, useDeferredValue} from "react";
import { invoke , convertFileSrc  } from "@tauri-apps/api/core";

import Titlebar from "./components/Titlebar/Titlebar";
import Sidebar from "./components/Sidebar/Sidebar";
import UserAgreement from "./components/UserAgreement/UserAgreement";

import { getConfig } from "./utils/config.jsx";
import "./App.css";


// https://launchercontent.mojang.com/v2/bedrockPatchNotes.json


const LazyContent = lazy(() => import("./components/Content/Content"));

function App() {
    const defaultConfig = useMemo(() => ({
        theme_color: '#90c7a8',
        background_option: 'default',
        local_image_path: '',
        network_image_url: '',
        show_launch_animation: true,
    }), []);

    const [config, setConfig] = useState(defaultConfig);
    const [activeSection, setActiveSection] = useState('launch');
    const [isDownloading, setIsDownloading] = useState(false);
    const [isPending, startTransition] = useTransition();

    // Batch styling changes
    const applyBackgroundStyle = useCallback(
        async ({ background_option, local_image_path, network_image_url }) => {
            const baseStyle = {
                backgroundSize: "cover",
                backgroundPosition: "center",
                backgroundRepeat: "no-repeat",
            };

            try {
                if (background_option === "local") {
                    if (local_image_path) {
                        try {
                            const fileUrl = convertFileSrc(local_image_path);
                            if (fileUrl) {
                                document.body.style.backgroundImage = `url(${fileUrl})`;
                                Object.assign(document.body.style, baseStyle);
                            }
                        } catch (e) {
                            console.warn("local 路径转换失败，不替换背景:", e);
                        }
                    }
                } else if (background_option === "network") {
                    try {
                        await new Promise((resolve, reject) => {
                            const img = new Image();
                            img.onload = resolve;
                            img.onerror = reject;
                            img.src = network_image_url;
                        });
                        document.body.style.backgroundImage = `url(${network_image_url})`;
                        Object.assign(document.body.style, baseStyle);
                    } catch (e) {
                        console.warn("network 图片加载失败，不替换背景:", e);
                    }
                } else {
                    // 其它情况：清空背景
                    document.body.style.backgroundImage = "";
                    Object.assign(document.body.style, baseStyle);
                }
            } catch (err) {
                console.error("设置背景出错:", err);
            }
        },
        []
    );

    useEffect(() => {
        let timeoutId;
        const init = async () => {
            try {
                const fullConfig = await getConfig();
                const style = fullConfig.custom_style || defaultConfig;
                setConfig(style);
                document.documentElement.style.setProperty('--theme-color', style.theme_color);
                startTransition(() => {
                    applyBackgroundStyle(style);
                });
                // 加一个短暂延迟，确保 bridge 和 DOM 就绪
                await new Promise(resolve => setTimeout(resolve, 50));
                if (style.show_launch_animation) {
                    await invoke('show_splashscreen');
                    timeoutId = setTimeout(() => {
                        invoke('close_splashscreen');
                    }, 3000);
                } else {
                    await invoke('close_splashscreen');
                }
            } catch (error) {
                console.error(error);
            }
        };
        init();
        const prevent = e => e.preventDefault();
        document.addEventListener('contextmenu', prevent);
        return () => {
            clearTimeout(timeoutId);
            document.removeEventListener('contextmenu', prevent);
        };
    }, [applyBackgroundStyle, defaultConfig]);

    const onSectionChange = useCallback((section) => {
        startTransition(() => setActiveSection(section));
    }, []);

    const deferredSection = useDeferredValue(activeSection);

    return (
    <>
        <UserAgreement/>
            <Titlebar />
            <div className="app-container">
                <Sidebar
                    activeSection={activeSection}
                    setActiveSection={onSectionChange}
                    disableSwitch={isDownloading || isPending}
                />
                <Suspense fallback={<div className="loading">Loading...</div>}>
                    <LazyContent
                        activeSection={deferredSection}
                        disableSwitch={isDownloading || isPending}
                        onStatusChange={setIsDownloading}
                    />
                </Suspense>
            </div>
        </>
    );
}

export default React.memo(App);
