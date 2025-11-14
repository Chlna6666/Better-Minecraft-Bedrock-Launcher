// App.jsx
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
import { useUpdater } from "./hooks/useUpdater";
import UpdateModal from "./components/UpdateModal";

import { getConfig } from "./utils/config.jsx";
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

    const { state: updState, actions: updActions } = useUpdater({
        owner: "Chlna6666",
        repo: "Better-Minecraft-Bedrock-Launcher",
    });

    // 控制弹窗
    const [modalOpen, setModalOpen] = useState(false);
    const [selectedRelease, setSelectedRelease] = useState(null);
    // 记住已经展示过的 tag，避免每次 render 或状态变化都弹出
    const [seenTag, setSeenTag] = useState("");

    // 内容/界面状态
    const [config, setConfig] = useState(defaultConfig);
    const [activeSection, setActiveSection] = useState("launch");
    const [isDownloading, setIsDownloading] = useState(false);
    const [isPending, startTransition] = useTransition();
    const [autoCheckUpdates, setAutoCheckUpdates] = useState(true);

    // 当检测到新 release（stable 或 prerelease）且未展示过时自动打开弹窗
    useEffect(() => {
        // 如果用户在配置里关闭了自动检查，则不自动弹窗
        if (!autoCheckUpdates) return;

        const latest = updState?.latestStable ?? updState?.latestPrerelease ?? null;
        if (latest && latest.tag && latest.tag !== seenTag) {
            setSeenTag(latest.tag);
            setSelectedRelease(latest);
            setModalOpen(true);
        }
    }, [updState?.latestStable, updState?.latestPrerelease, seenTag, autoCheckUpdates]);

    // optional: 将 hook 的 downloading 状态同步到局部 state（用于禁用 UI）
    useEffect(() => {
        setIsDownloading(Boolean(updState?.downloading));
    }, [updState?.downloading]);

    const handleDetails = useCallback((release) => {
        setSelectedRelease(release);
        setModalOpen(true);
    }, []);

    const handleDownload = useCallback(
        async (release) => {
            try {
                await updActions.downloadAndApply(release);
                // 你可以在这里触发 toast 或本地通知
            } catch (e) {
                console.error("下载失败", e);
                alert("下载失败：" + (e?.toString?.() || e));
            }
        },
        [updActions]
    );

    const handleApply = useCallback(
        async (downloadedPath) => {
            try {
                // 传空 target 让后端使用当前 exe（建议后端支持）
                const resp = await updActions.applyUpdate(downloadedPath, "");
                console.log("apply_update resp:", resp);
                // 之后应退出程序以便脚本覆盖 exe
            } catch (e) {
                console.error("应用更新失败", e);
                alert("应用更新失败：" + (e?.toString?.() || e));
            }
        },
        [updActions]
    );

    // 背景样式函数（不变）
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
                const autoCheck = Boolean(
                    fullConfig?.launcher?.auto_check_updates ?? true
                );
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
                <Sidebar activeSection={activeSection} setActiveSection={onSectionChange} disableSwitch={isDownloading || isPending} />
                <Suspense fallback={<div className="loading">Loading...</div>}>
                    <LazyContent activeSection={deferredSection} disableSwitch={isDownloading || isPending} onStatusChange={setIsDownloading} />
                </Suspense>

        </>
    );
}

export default React.memo(App);
