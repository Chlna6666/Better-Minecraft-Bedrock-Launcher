import { useState, useEffect, useCallback, useTransition, useMemo } from 'react';
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { getConfig } from "../utils/config";

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
    const [, startTransition] = useTransition();

    // 应用背景图片的逻辑
    const applyBackground = useCallback((style: AppStyleConfig) => {
        const baseStyle = {
            backgroundSize: "cover",
            backgroundPosition: "center",
            backgroundRepeat: "no-repeat",
            backgroundAttachment: "fixed",
            transition: "background-image 0.5s ease"
        };

        const setBg = (url: string) => {
            document.body.style.backgroundImage = `url(${url})`;
            Object.assign(document.body.style, baseStyle);
        };

        try {
            if (style.background_option === "local" && style.local_image_path) {
                const fileUrl = convertFileSrc(style.local_image_path);
                setBg(fileUrl);
            } else if (style.background_option === "network" && style.network_image_url) {
                const img = new Image();
                img.src = style.network_image_url;
                img.onload = () => setBg(style.network_image_url);
            } else {
                document.body.style.backgroundImage = "";
            }
        } catch (e) {
            console.warn("Background apply failed:", e);
        }
    }, []);

    // 初始化逻辑
    useEffect(() => {
        let timeoutId: NodeJS.Timeout;

        const init = async () => {
            try {
                const fullConfig = await getConfig().catch(() => ({}));
                const style = { ...defaultConfig, ...fullConfig.custom_style };

                setConfig(style);

                // 设置主题色
                if (style.theme_color) {
                    document.documentElement.style.setProperty("--accent-color", style.theme_color);
                }

                // 设置背景 (使用 startTransition 降低优先级，避免阻塞 UI)
                startTransition(() => {
                    applyBackground(style);
                });

                // 处理 Splashscreen
                await new Promise((r) => setTimeout(r, 50));
                if (style.show_launch_animation) {
                    await invoke("show_splashscreen").catch(() => {});
                    timeoutId = setTimeout(() => {
                        invoke("close_splashscreen").catch(() => {});
                    }, 3000);
                } else {
                    await invoke("close_splashscreen").catch(() => {});
                }
            } catch (error) {
                console.error("App Init Error:", error);
                invoke("close_splashscreen").catch(() => {});
            }
        };

        init();

        // 禁用右键
        const preventCtx = (e: Event) => e.preventDefault();
        document.addEventListener("contextmenu", preventCtx);

        return () => {
            clearTimeout(timeoutId);
            document.removeEventListener("contextmenu", preventCtx);
        };
    }, [defaultConfig, applyBackground]);

    return config;
};