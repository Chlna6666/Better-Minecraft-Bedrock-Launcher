import React, { useState, useEffect, useCallback, useRef } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getConfig } from "../../utils/config";
import Switch from "../../components/Switch";
import Select from "../../components/Select";
import Slider from "../../components/Slider/Slider";
import { useTranslation } from "react-i18next";
import { SUPPORTED_LANGUAGES } from "../../locales/i18n";
import { Input } from "../../components";
import { ChevronRight } from "lucide-react"; // [新增] 引入箭头图标
import ConnectivityModal from "../../components/ConnectivityModal"; // [新增] 引入弹窗组件

// ... (Variants 保持不变)
const containerVariants = {
    hidden: { opacity: 0 },
    visible: { opacity: 1, transition: { staggerChildren: 0.06, delayChildren: 0.1 } },
    exit: { opacity: 0, transition: { duration: 0.2 } }
};
const itemVariants = {
    hidden: { opacity: 0, x: -20 },
    visible: { opacity: 1, x: 0, transition: { type: "spring", stiffness: 300, damping: 24 } }
};
const expandVariants = {
    collapsed: { height: 0, opacity: 0, transition: { duration: 0.2, ease: "easeInOut" } },
    expanded: { height: "auto", opacity: 1, transition: { duration: 0.3, ease: "easeOut" } }
};

export default function Launcher() {
    const { t, i18n } = useTranslation();
    const [loaded, setLoaded] = useState(false);
    const [debugMode, setDebugMode] = useState(false);
    const [gpuAcceleration, setGpuAcceleration] = useState(true);
    const [language, setLanguage] = useState("auto");
    const [userLanguage, setUserLanguage] = useState("auto");
    // const [customAppxApi, setCustomAppxApi] = useState(""); // 未使用的变量可以注释或删除

    // [新增] 控制连通性测试弹窗的状态
    const [showConnectivity, setShowConnectivity] = useState(false);

    // Download settings
    const [multiThread, setMultiThread] = useState(false);
    const [autoThreadCount, setAutoThreadCount] = useState(true);
    const [maxThreads, setMaxThreads] = useState(8);

    // Update settings
    const [updateChannel, setUpdateChannel] = useState("stable");
    const [autoCheckUpdates, setAutoCheckUpdates] = useState(true);

    // Proxy settings
    const [proxyType, setProxyType] = useState("system");
    const [httpProxyUrl, setHttpProxyUrl] = useState("");
    const [socksProxyUrl, setSocksProxyUrl] = useState("");
    const [curseforgeApiSource, setCurseforgeApiSource] = useState("mirror");
    const [curseforgeApiBase, setCurseforgeApiBase] = useState("https://mod.mcimirror.top/curseforge");

    useEffect(() => {
        async function fetchConfig() {
            try {
                const fullConfig: any = await getConfig();
                const launcher = fullConfig.launcher || {};
                const download = launcher.download || {};
                const proxy = download.proxy || {};

                setDebugMode(launcher.debug || false);
                setGpuAcceleration(launcher.hasOwnProperty('gpu_acceleration') ? !!launcher.gpu_acceleration : true);
                const storedLang = launcher.language || "auto";
                const normalizedLang = storedLang === "auto"
                    ? "auto"
                    : (SUPPORTED_LANGUAGES[storedLang] ? storedLang : storedLang.replace('_', '-'));
                const finalLang = normalizedLang === "auto"
                    ? "auto"
                    : (SUPPORTED_LANGUAGES[normalizedLang] ? normalizedLang : "en-US");
                setLanguage(finalLang);
                setUserLanguage(finalLang);

                setMultiThread(download.multi_thread || false);
                setAutoThreadCount(download.hasOwnProperty('auto_thread_count') ? download.auto_thread_count : true);
                setMaxThreads(download.max_threads || 8);

                const pt = (proxy.proxy_type || "system").toLowerCase();
                setProxyType(["none", "system", "http", "socks5"].includes(pt) ? pt : "system");
                setHttpProxyUrl(proxy.http_proxy_url || "");
                setSocksProxyUrl(proxy.socks_proxy_url || "");
                const source = (download.curseforge_api_source || "mirror").toLowerCase();
                setCurseforgeApiSource(["official", "mirror", "custom"].includes(source) ? source : "mirror");
                setCurseforgeApiBase(download.curseforge_api_base || "https://mod.mcimirror.top/curseforge");

                setUpdateChannel(launcher.update_channel || "stable");
                setAutoCheckUpdates(launcher.hasOwnProperty('auto_check_updates') ? launcher.auto_check_updates : true);

                setLoaded(true);
            } catch (e) { console.error(e); }
        }
        fetchConfig();
    }, []);

    useEffect(() => {
        if (!loaded) return;
        const save = async () => {
            try {
                const full: any = await invoke("get_config");
                const launcher = full.launcher || {};

                launcher.debug = debugMode;
                launcher.gpu_acceleration = gpuAcceleration;
                launcher.language = language;
                launcher.update_channel = updateChannel;
                launcher.auto_check_updates = autoCheckUpdates;
                launcher.download = launcher.download || {};
                launcher.download.multi_thread = multiThread;
                launcher.download.max_threads = maxThreads;
                launcher.download.auto_thread_count = autoThreadCount;
                launcher.download.proxy = launcher.download.proxy || {};
                launcher.download.proxy.proxy_type = proxyType;
                launcher.download.proxy.http_proxy_url = httpProxyUrl;
                launcher.download.proxy.socks_proxy_url = socksProxyUrl;
                launcher.download.curseforge_api_source = curseforgeApiSource;
                launcher.download.curseforge_api_base = curseforgeApiBase;

                await invoke("set_config", { key: "launcher", value: launcher });
                emit("refresh-config").catch(() => {});
            } catch (e) {}
        };
        save();
    }, [loaded, debugMode, gpuAcceleration, language, multiThread, autoThreadCount, maxThreads, proxyType, httpProxyUrl, socksProxyUrl, autoCheckUpdates, updateChannel, curseforgeApiSource, curseforgeApiBase]);

    // Debounce Save Helper
    const saveRef = useRef<NodeJS.Timeout | null>(null);
    const debouncedSave = useCallback((value: number) => {
        if (saveRef.current) clearTimeout(saveRef.current);
        saveRef.current = setTimeout(async () => {
            try {
                const full: any = await invoke("get_config");
                const launcher = full.launcher || {};
                launcher.download = launcher.download || {};
                launcher.download.max_threads = value;
                await invoke("set_config", { key: "launcher", value: launcher });
            } catch (e) {
                console.error("Failed to debounced save max_threads:", e);
            }
        }, 500);
    }, []);

    const handleSliderChange = (val: number) => {
        setMaxThreads(val);
        debouncedSave(val);
    };

    const handleLanguageChange = async (newLang: string) => {
        setUserLanguage(newLang);
        setLanguage(newLang);
        const target = newLang === "auto" ? await invoke("get_system_language") : newLang;
        const localeStr = String(target);
        const normalizedLocale = SUPPORTED_LANGUAGES[localeStr]
            ? localeStr
            : localeStr.replace('_', '-');
        const finalLocale = SUPPORTED_LANGUAGES[normalizedLocale] ? normalizedLocale : "en-US";
        await i18n.changeLanguage(finalLocale);
    };

    const proxyOptions = [
        { value: "none", label: t("LauncherSettings.download.proxy.none") },
        { value: "system", label: t("LauncherSettings.download.proxy.system") },
        { value: "http", label: t("LauncherSettings.download.proxy.http") },
        { value: "socks5", label: t("LauncherSettings.download.proxy.socks5") },
    ];

    const getDownloadTitle = () => {
        const key = "LauncherSettings.download.title";
        const val = t(key);
        if (val === key || typeof val === 'object') return "Download Settings";
        return val;
    };

    const fixedControlStyle = { width: '160px' };

    return (
        <motion.div
            className="settings-inner-container"
            variants={containerVariants}
            initial="hidden"
            animate="visible"
            exit="exit"
        >
            <motion.h3 variants={itemVariants} className="settings-group-title">{t("Settings.tabs.launcher")}</motion.h3>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("LauncherSettings.debug")}</label>
                <Switch checked={debugMode} onChange={() => setDebugMode(!debugMode)} />
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("LauncherSettings.gpu_acceleration")}</label>
                <Switch checked={gpuAcceleration} onChange={() => setGpuAcceleration(!gpuAcceleration)} />
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("LauncherSettings.language")}</label>
                <div style={fixedControlStyle}>
                    <Select
                        value={userLanguage}
                        onChange={(val: any) => handleLanguageChange(val)}
                        options={[
                            { value: "auto", label: t("LauncherSettings.lang_options.auto") || "Auto" },
                            ...Object.entries(SUPPORTED_LANGUAGES).map(([key, { label }]) => ({ value: key, label })),
                        ]}
                        placeholder={t("LauncherSettings.lang_placeholder")}
                        size={13}
                    />
                </div>
            </motion.div>

            {/* [新增] 服务连通性测试入口 */}
            <motion.div
                variants={itemVariants}
                className="setting-item"
                style={{ cursor: 'pointer' }} // 鼠标悬停显示手型
                onClick={() => setShowConnectivity(true)} // 点击触发弹窗
                // 添加 hover 效果增强交互感 (可选，因为 css 中 .setting-item:hover 已经有了)
            >
                <label style={{ cursor: 'pointer' }}>
                    {t("LauncherSettings.connectivity_test") || "服务连通性测试"}
                </label>
                <div style={{ display: 'flex', alignItems: 'center', color: 'var(--c-text-secondary)' }}>
                    {/* 使用箭头图标作为“按钮” */}
                    <ChevronRight size={20} />
                </div>
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item grouped">
                <div className="setting-header">
                    <label>{t("LauncherSettings.auto_check_updates")}</label>
                    <Switch
                        checked={autoCheckUpdates}
                        onChange={() => setAutoCheckUpdates(!autoCheckUpdates)}
                    />
                </div>
                <AnimatePresence initial={false}>
                    {autoCheckUpdates && (
                        <motion.div
                            variants={expandVariants}
                            initial="collapsed"
                            animate="expanded"
                            exit="collapsed"
                            style={{ overflow: 'hidden' }}
                        >
                            <div className="setting-sub-group">
                                <div className="sub-group-spacer" />
                                <div className="sub-setting-row">
                                    <label>{t("LauncherSettings.update_channel") || "Update Channel"}</label>
                                    <div className="sub-control-wrapper">
                                        <Select
                                            value={updateChannel}
                                            onChange={(val: any) => setUpdateChannel(val)}
                                            options={[
                                                { value: "stable", label: t("LauncherSettings.update_channel.stable") },
                                                { value: "nightly", label: t("LauncherSettings.update_channel.nightly") },
                                            ]}
                                            placeholder={t("LauncherSettings.update_channel.placeholder")}
                                            size={13}
                                        />
                                    </div>
                                </div>
                            </div>
                        </motion.div>
                    )}
                </AnimatePresence>
            </motion.div>

            <motion.h4 variants={itemVariants} className="settings-sub-title">
                {getDownloadTitle()}
            </motion.h4>

            {/* --- 多线程下载 --- */}
            <motion.div variants={itemVariants} className="setting-item grouped">
                <div className="setting-header">
                    <label>{t("LauncherSettings.download.multi_thread")}</label>
                    <Switch
                        checked={multiThread}
                        onChange={() => {
                            const next = !multiThread;
                            setMultiThread(next);
                            if (next) setAutoThreadCount(false);
                        }}
                    />
                </div>

                <AnimatePresence initial={false}>
                    {multiThread && (
                        <motion.div
                            variants={expandVariants}
                            initial="collapsed"
                            animate="expanded"
                            exit="collapsed"
                            style={{ overflow: 'hidden' }}
                        >
                            <div className="setting-sub-group">
                                <div className="sub-group-spacer" />
                                <div className="sub-setting-row">
                                    <label>{t("LauncherSettings.download.max_threads")}</label>
                                    <div style={{ display: 'flex', alignItems: 'center', gap: '16px' }}>
                                        <div style={{ width: '140px' }}>
                                            <Slider
                                                min={1}
                                                max={256}
                                                value={maxThreads}
                                                onChange={handleSliderChange}
                                            />
                                        </div>
                                        <div className="sub-control-wrapper" style={{ width: '56px' }}>
                                            <Input
                                                type="number"
                                                value={maxThreads}
                                                onChange={(e: any) => {
                                                    const v = Math.min(256, Math.max(1, parseInt(e.target.value, 10) || 1));
                                                    setMaxThreads(v);
                                                    debouncedSave(v);
                                                }}
                                                min={1}
                                                max={256}
                                            />
                                        </div>
                                    </div>
                                </div>
                            </div>
                        </motion.div>
                    )}
                </AnimatePresence>
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("LauncherSettings.download.auto_thread_count")}</label>
                <Switch
                    checked={autoThreadCount}
                    onChange={() => {
                        const next = !autoThreadCount;
                        setAutoThreadCount(next);
                        if (next) setMultiThread(false);
                    }}
                />
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item grouped">
                <div className="setting-header">
                    <label>{t("LauncherSettings.download.curseforge_api_source") || "CurseForge API Source"}</label>
                    <div style={fixedControlStyle}>
                        <Select
                            value={curseforgeApiSource}
                            onChange={(val: any) => {
                                const next = String(val);
                                setCurseforgeApiSource(next);
                                if (next !== "custom") {
                                    setCurseforgeApiBase(
                                        next === "official"
                                            ? "https://api.curseforge.com"
                                            : "https://mod.mcimirror.top/curseforge"
                                    );
                                } else if (!curseforgeApiBase) {
                                    setCurseforgeApiBase("https://mod.mcimirror.top/curseforge");
                                }
                            }}
                            options={[
                                { value: "official", label: t("LauncherSettings.download.curseforge_api_source.official") || "Official" },
                                { value: "mirror", label: t("LauncherSettings.download.curseforge_api_source.mirror") || "Mirror" },
                                { value: "custom", label: t("LauncherSettings.download.curseforge_api_source.custom") || "Custom" },
                            ]}
                            placeholder={t("LauncherSettings.download.curseforge_api_source.placeholder") || "Select"}
                            size={13}
                        />
                    </div>
                </div>
                <AnimatePresence initial={false}>
                    {curseforgeApiSource === "custom" && (
                        <motion.div
                            variants={expandVariants}
                            initial="collapsed"
                            animate="expanded"
                            exit="collapsed"
                            style={{ overflow: 'hidden' }}
                        >
                            <div className="setting-sub-group">
                                <div className="sub-group-spacer" />
                                <div className="sub-setting-row">
                                    <label>{t("LauncherSettings.download.curseforge_api_base") || "CurseForge API Base"}</label>
                                    <div className="sub-control-wrapper" style={{ width: '280px', maxWidth: '280px' }}>
                                        <Input
                                            type="text"
                                            value={curseforgeApiBase}
                                            onChange={(e: any) => setCurseforgeApiBase(e.target.value)}
                                            placeholder="https://mod.mcimirror.top/curseforge"
                                            style={{ width: '100%' }}
                                        />
                                    </div>
                                </div>
                            </div>
                        </motion.div>
                    )}
                </AnimatePresence>
            </motion.div>

            {/* 代理设置 */}
            <motion.div variants={itemVariants} className="setting-item grouped">
                <div className="setting-header">
                    <label>{t("LauncherSettings.download.proxy.mode") || "Proxy Mode"}</label>
                    <div style={fixedControlStyle}>
                        <Select
                            value={proxyType}
                            onChange={(val: any) => setProxyType(val)}
                            options={proxyOptions}
                            placeholder="Select proxy mode"
                            size={13}
                        />
                    </div>
                </div>

                <AnimatePresence initial={false}>
                    {(proxyType === "http" || proxyType === "socks5") && (
                        <motion.div
                            variants={expandVariants}
                            initial="collapsed"
                            animate="expanded"
                            exit="collapsed"
                            style={{ overflow: 'hidden' }}
                        >
                            <div className="setting-sub-group">
                                <div className="sub-group-spacer" />

                                {proxyType === "http" && (
                                    <div className="sub-setting-row">
                                        <label>{t("LauncherSettings.download.proxy.http_proxy_url")}</label>
                                        <div className="sub-control-wrapper" style={{ width: '220px', maxWidth: '220px' }}>
                                            <Input
                                                type="text"
                                                value={httpProxyUrl}
                                                onChange={(e: any) => setHttpProxyUrl(e.target.value)}
                                                placeholder="http(s)://host:port"
                                                style={{ width: '100%' }}
                                            />
                                        </div>
                                    </div>
                                )}

                                {proxyType === "socks5" && (
                                    <div className="sub-setting-row">
                                        <label>{t("LauncherSettings.download.proxy.socks_proxy_url")}</label>
                                        <div className="sub-control-wrapper" style={{ width: '220px', maxWidth: '220px' }}>
                                            <Input
                                                type="text"
                                                value={socksProxyUrl}
                                                onChange={(e: any) => setSocksProxyUrl(e.target.value)}
                                                placeholder="socks5://host:port"
                                                style={{ width: '100%' }}
                                            />
                                        </div>
                                    </div>
                                )}
                            </div>
                        </motion.div>
                    )}
                </AnimatePresence>
            </motion.div>

            {/* [新增] 渲染弹窗组件 */}
            {/* 把它放在最下面，使用 React Portal (组件内部已处理) */}
            <ConnectivityModal
                isOpen={showConnectivity}
                onClose={() => setShowConnectivity(false)}
            />

        </motion.div>
    );
}
