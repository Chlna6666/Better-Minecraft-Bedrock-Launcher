import { useState, useEffect, useMemo, useCallback } from 'react';
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event"; // [新增] 引入监听器
import { getConfig } from "../utils/config";
import defaultBgImage from '../assets/background.webp';

export interface AppStyleConfig {
    theme_color: string;
    background_option: "default" | "local" | "network";
    local_image_path: string;
    network_image_url: string;
    show_launch_animation: boolean;
}

export const useAppConfig = () => {
    // 默认配置
    const defaultConfig = useMemo<AppStyleConfig>(() => ({
        theme_color: "#90c7a8",
        background_option: "default",
        local_image_path: "",
        network_image_url: "",
        show_launch_animation: true,
    }), []);

    const [config, setConfig] = useState<AppStyleConfig>(defaultConfig);
    const [backgroundImageUrl, setBackgroundImageUrl] = useState<string>(defaultBgImage);

    // [核心逻辑] 抽离为 loadConfig 函数，方便重复调用
    const loadConfig = useCallback(async (isFirstLoad = false) => {
        try {
            // 1. 读取配置
            const fullConfig: any = await getConfig().catch(() => ({}));
            const savedStyle = fullConfig.custom_style || fullConfig.game || {};
            const style = { ...defaultConfig, ...savedStyle };

            setConfig(style);

            // [调试] 打印关键信息，按 F12 看 Console
            console.log("[AppConfig] Option:", style.background_option);
            console.log("[AppConfig] Local Path:", style.local_image_path);

            // 2. 设置主题色
            if (style.theme_color) {
                document.documentElement.style.setProperty("--accent-color", style.theme_color);
                document.documentElement.style.setProperty("--theme-color", style.theme_color);
            }

            // 3. 计算背景图片 URL
            let bgUrl = defaultBgImage;

            if (style.background_option === "local" && style.local_image_path) {
                // 将本地文件路径转换为浏览器可访问的 asset 协议 URL
                bgUrl = convertFileSrc(style.local_image_path);
            } else if (style.background_option === "network" && style.network_image_url) {
                const img = new Image();
                img.src = style.network_image_url;
                bgUrl = style.network_image_url;
            }

            console.log("[AppConfig] Final URL:", bgUrl);
            setBackgroundImageUrl(bgUrl);

            // 4. 处理启动动画 (仅在首次加载时)
            if (isFirstLoad) {
                // 稍微延时，确保 React 渲染完成
                await new Promise((r) => setTimeout(r, 50));

                if (style.show_launch_animation) {
                    await invoke("show_splashscreen").catch(() => {});
                    setTimeout(() => {
                        invoke("close_splashscreen").catch(() => {});
                    }, 3000);
                } else {
                    await invoke("close_splashscreen").catch(() => {});
                }
            }

        } catch (error) {
            console.error("[AppConfig] Load Error:", error);
            if (isFirstLoad) invoke("close_splashscreen").catch(() => {});
        }
    }, [defaultConfig]);

    // 初始化与监听
    useEffect(() => {
        // 首次加载
        loadConfig(true);

        // [新增] 监听 "refresh-config" 事件，收到后重新加载配置
        const unlistenPromise = listen('refresh-config', () => {
            console.log("[AppConfig] Received refresh signal, reloading...");
            loadConfig(false);
        });

        // 禁用右键
        const preventCtx = (e: Event) => e.preventDefault();
        document.addEventListener("contextmenu", preventCtx);

        return () => {
            unlistenPromise.then(unlisten => unlisten());
            document.removeEventListener("contextmenu", preventCtx);
        };
    }, [loadConfig]);

    return { ...config, backgroundImageUrl };
};