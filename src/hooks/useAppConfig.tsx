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
            // [修复] 必须加引号 "${url}"，防止路径中有空格导致 CSS 失效
            document.body.style.backgroundImage = `url("${url}")`;
            Object.assign(document.body.style, baseStyle);
        };

        const clearBg = () => {
            document.body.style.backgroundImage = "";
            // 如果是默认，可能需要重置为默认背景色，这里视情况而定
        };

        try {
            console.log("Applying background:", style.background_option); // [调试]

            if (style.background_option === "local" && style.local_image_path) {
                // Tauri 2.0 资源协议转换
                const fileUrl = convertFileSrc(style.local_image_path);
                console.log("Local URL:", fileUrl); // [调试]
                setBg(fileUrl);
            } else if (style.background_option === "network" && style.network_image_url) {
                // 网络图片预加载，防止闪烁
                const img = new Image();
                img.src = style.network_image_url;
                img.onload = () => setBg(style.network_image_url);
            } else {
                clearBg();
            }
        } catch (e) {
            console.warn("Background apply failed:", e);
            clearBg();
        }
    }, []);

    // 初始化逻辑
    useEffect(() => {
        let timeoutId: NodeJS.Timeout;

        const init = async () => {
            try {
                const fullConfig: any = await getConfig().catch(() => ({}));
                // 确保这里读取的结构和你保存的结构一致
                // 如果 config.json 里直接是平铺的，就用 fullConfig，如果是嵌套在 custom_style 里，就用 fullConfig.custom_style
                const savedStyle = fullConfig.custom_style || fullConfig.game || {};
                const style = { ...defaultConfig, ...savedStyle };

                setConfig(style);

                // 设置主题色
                if (style.theme_color) {
                    document.documentElement.style.setProperty("--accent-color", style.theme_color);
                    // 如果用了我们之前的 CSS 变量，可能还需要设置这个：
                    document.documentElement.style.setProperty("--theme-color", style.theme_color);
                }

                // 设置背景
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

        const preventCtx = (e: Event) => e.preventDefault();
        document.addEventListener("contextmenu", preventCtx);

        return () => {
            clearTimeout(timeoutId);
            document.removeEventListener("contextmenu", preventCtx);
        };
    }, [defaultConfig, applyBackground]);

    return config;
};