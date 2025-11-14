import React, {
    useState,
    useEffect,
    useCallback,
    useMemo,
    lazy,
    Suspense,
    useTransition,
    useDeferredValue,
} from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";

import Titlebar from "./pages/Titlebar/Titlebar";
import Sidebar from "./pages/Sidebar/Sidebar";
import UserAgreement from "./pages/UserAgreement/UserAgreement";
import UpdateModal from "./components/UpdateModal";

import { getConfig } from "./utils/config.jsx";

import { useUpdaterWithModal } from "./hooks/useUpdaterWithModal";

import "./App.css";

const LazyContent = lazy(() => import("./pages/Content/Content"));

function App() {
    const defaultConfig = useMemo(
        () => ({
            theme_color: "#90c7a8",
            background_option: "default",
            local_image_path: "",
            network_image_url: "",
            show_launch_animation: true,
        }),
        []
    );

    // 这里调用封装后的hook，自动管理弹窗
    const {
        state: updState,
        actions: updActions,
        modalOpen,
        setModalOpen,
        selectedRelease,
    } = useUpdaterWithModal({
        owner: "Chlna6666",
        repo: "Better-Minecraft-Bedrock-Launcher",
        autoCheck: true,
    });

    const [config, setConfig] = useState(defaultConfig);
    const [activeSection, setActiveSection] = useState("launch");
    const [isDownloading, setIsDownloading] = useState(false);
    const [isPending, startTransition] = useTransition();
    const [autoCheckUpdates, setAutoCheckUpdates] = useState(true);

    // 同步下载状态，禁用 UI
    useEffect(() => {
        setIsDownloading(Boolean(updState?.downloading));
    }, [updState?.downloading]);

    // 下载更新处理
    const handleDownload = useCallback(
        async (release) => {
            try {
                await updActions.downloadAndApply(release);
                // 这里可以触发通知
            } catch (e) {
                console.error("下载失败", e);
                alert("下载失败：" + (e?.toString?.() || e));
            }
        },
        [updActions]
    );

    // 应用更新处理（如果需要）
    const handleApply = useCallback(
        async (downloadedPath) => {
            try {
                const resp = await updActions.applyUpdate(downloadedPath, "");
                console.log("apply_update resp:", resp);
            } catch (e) {
                console.error("应用更新失败", e);
                alert("应用更新失败：" + (e?.toString?.() || e));
            }
        },
        [updActions]
    );

    // 背景样式函数
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
                    document.body.style.backgroundImage = "";
                    Object.assign(document.body.style, baseStyle);
                }
            } catch (err) {
                console.error("设置背景出错:", err);
            }
        },
        []
    );

    // 初始化配置
    useEffect(() => {
        let timeoutId;
        const init = async () => {
            try {
                const fullConfig = await getConfig();
                const style = fullConfig.custom_style || defaultConfig;
                setConfig(style);
                document.documentElement.style.setProperty("--theme-color", style.theme_color);
                startTransition(() => {
                    applyBackgroundStyle(style);
                });
                const autoCheck = Boolean(fullConfig?.launcher?.auto_check_updates ?? true);
                setAutoCheckUpdates(autoCheck);
                await new Promise((resolve) => setTimeout(resolve, 50));
                if (style.show_launch_animation) {
                    await invoke("show_splashscreen");
                    timeoutId = setTimeout(() => {
                        invoke("close_splashscreen");
                    }, 3000);
                } else {
                    await invoke("close_splashscreen");
                }
            } catch (error) {
                console.error(error);
            }
        };
        init();
        const prevent = (e) => e.preventDefault();
        document.addEventListener("contextmenu", prevent);
        return () => {
            clearTimeout(timeoutId);
            document.removeEventListener("contextmenu", prevent);
        };
    }, [applyBackgroundStyle, defaultConfig, startTransition]);

    const onSectionChange = useCallback(
        (section) => {
            startTransition(() => setActiveSection(section));
        },
        [startTransition]
    );

    const deferredSection = useDeferredValue(activeSection);

    return (
        <>
            <UserAgreement />
            <Titlebar />
            <UpdateModal
                open={modalOpen}
                onClose={() => setModalOpen(false)}
                release={selectedRelease}
                onDownload={handleDownload}
                downloading={updState?.downloading}
                downloadResult={updState?.downloadResult}
                onApply={handleApply}
            />
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
        </>
    );
}

export default React.memo(App);
