import React, {useState, useEffect, useCallback, useRef} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../../utils/config.jsx";
import Switch from "../../components/Switch.jsx";
import Select from "../../components/Select.jsx"; // <- 自定义 Select
import {useTranslation} from "react-i18next";
import {SUPPORTED_LANGUAGES} from "../../i18n/i18n.js";
import {Input} from "../../components/index.js";

function Launcher() {
    const { t, i18n } = useTranslation();
    const [loaded, setLoaded] = useState(false);
    const [debugMode, setDebugMode] = useState(false);
    const [language, setLanguage] = useState("auto"); // persisted value (auto or lang key)
    const [userLanguage, setUserLanguage] = useState("auto"); // UI selection
    const [customAppxApi, setCustomAppxApi] = useState("");
    const [multiThread, setMultiThread] = useState(false);
    const [autoThreadCount, setAutoThreadCount] = useState(true);
    const [maxThreads, setMaxThreads] = useState(8);

    const [disableAllProxy, setDisableAllProxy] = useState(false);
    const [useSystemProxy, setUseSystemProxy] = useState(true);
    const [enableHttpProxy, setEnableHttpProxy] = useState(false);
    const [httpProxyUrl, setHttpProxyUrl] = useState("");
    const [enableSocksProxy, setEnableSocksProxy] = useState(false);
    const [socksProxyUrl, setSocksProxyUrl] = useState("");
    const [enableCustomProxy, setEnableCustomProxy] = useState(false);
    const [customProxyUrl, setCustomProxyUrl] = useState("");

    const DEFAULT_PROXY = "system";

    // helper to switch proxy mode (keeps previous behavior)
    const selectProxyMode = useCallback((mode) => {
        if (mode === "none") {
            if (disableAllProxy) {
                selectProxyMode(DEFAULT_PROXY);
            } else {
                setDisableAllProxy(true);
                setUseSystemProxy(false);
                setEnableHttpProxy(false);
                setEnableSocksProxy(false);
                setEnableCustomProxy(false);
            }
            return;
        }
        setDisableAllProxy(false);
        setUseSystemProxy(mode === "system");
        setEnableHttpProxy(mode === "http");
        setEnableSocksProxy(mode === "socks");
        setEnableCustomProxy(mode === "custom");
    }, [disableAllProxy]);

    // load config once
    useEffect(() => {
        async function fetchConfig() {
            try {
                const fullConfig = await getConfig();
                const launcher = fullConfig.launcher || {};
                const download = launcher.download || {};
                const proxy = download.proxy || {};

                setDebugMode(launcher.debug || false);
                setLanguage(launcher.language || "auto");
                setUserLanguage(launcher.language || "auto");
                setCustomAppxApi(launcher.custom_appx_api || "");
                setMultiThread(download.multi_thread ?? false);
                setAutoThreadCount(download.auto_thread_count ?? true);
                setMaxThreads(download.max_threads || 8);
                setDisableAllProxy(proxy.disable_all_proxy ?? false);
                setUseSystemProxy(proxy.use_system_proxy ?? true);
                setEnableHttpProxy(proxy.enable_http_proxy ?? false);
                setHttpProxyUrl(proxy.http_proxy_url || "");
                setEnableSocksProxy(proxy.enable_socks_proxy ?? false);
                setSocksProxyUrl(proxy.socks_proxy_url || "");
                setEnableCustomProxy(proxy.enable_custom_proxy ?? false);
                setCustomProxyUrl(proxy.custom_proxy_url || "");

                setLoaded(true);
            } catch (e) {
                console.error("Failed to load launcher config:", e);
            }
        }
        fetchConfig();
    }, []);

    // save whole launcher config whenever relevant states change (保留你原来的自动保存逻辑)
    useEffect(() => {
        if (!loaded) return;

        const updated = {
            debug: debugMode,
            language,
            custom_appx_api: customAppxApi,
            download: {
                multi_thread: multiThread,
                max_threads: maxThreads,
                auto_thread_count: autoThreadCount,
                proxy: {
                    disable_all_proxy: disableAllProxy,
                    use_system_proxy: useSystemProxy,
                    enable_http_proxy: enableHttpProxy,
                    http_proxy_url: httpProxyUrl,
                    enable_socks_proxy: enableSocksProxy,
                    socks_proxy_url: socksProxyUrl,
                    enable_custom_proxy: enableCustomProxy,
                    custom_proxy_url: customProxyUrl,
                },
            },
        };

        (async () => {
            try {
                await invoke("set_config", { key: "launcher", value: updated });
            } catch (e) {
                console.error("Failed to save launcher config:", e);
            }
        })();
    }, [
        loaded, debugMode, language, customAppxApi, multiThread, autoThreadCount, maxThreads,
        disableAllProxy, useSystemProxy, enableHttpProxy, httpProxyUrl,
        enableSocksProxy, socksProxyUrl, enableCustomProxy, customProxyUrl
    ]);

    // 初次加载时：从后端取系统语言并切换（保留你原来逻辑）
    useEffect(() => {
        (async () => {
            try {
                const full = await invoke("get_config");
                const lang = full.launcher?.language || "auto";
                setUserLanguage(lang);

                const target =
                    lang === "auto"
                        ? await invoke("get_system_language")
                        : lang;
                await i18n.changeLanguage(target);
            } catch (e) {
                console.error("Failed to initialize language:", e);
            }
        })();
    }, []);

    // 防抖保存 max_threads（保留）
    const saveRef = useRef(null);
    const debouncedSave = useCallback((value) => {
        if (saveRef.current) clearTimeout(saveRef.current);
        saveRef.current = setTimeout(async () => {
            try {
                const full = await invoke("get_config");
                const launcher = full.launcher || {};
                launcher.download = launcher.download || {};
                launcher.download.max_threads = value;
                await invoke("set_config", { key: "launcher", value: launcher });
            } catch (e) {
                console.error("Failed to debounced save max_threads:", e);
            }
        }, 500);
    }, []);

    // slider helper
    const sliderMin = 1;
    const sliderMax = 256;
    const pct = ((maxThreads - sliderMin) / (sliderMax - sliderMin)) * 100;

    useEffect(() => {
        (async () => {
            try {
                const full = await invoke("get_config");
                const v = full.launcher?.download?.max_threads ?? 8;
                setMaxThreads(v);
            } catch (e) {
                // ignore
            }
        })();
    }, []);

    const handleSliderChange = (e) => {
        const v = Math.min(sliderMax, Math.max(sliderMin, parseInt(e.target.value, 10) || sliderMin));
        setMaxThreads(v);
        debouncedSave(v);
    };

    // ---- 这里是关键：用自定义 Select 的回调（接收 value，而不是 event） ----
    const handleLanguageChange = async (newLang) => {
        // newLang 是 'auto' 或语言 key（与你传入的 options.value 对应）
        setUserLanguage(newLang);
        setLanguage(newLang); // 更新要持久化的值（会被自动保存 effect 捕获）

        // 切换 i18n：auto 时获取系统语言
        const target = newLang === "auto" ? await invoke("get_system_language") : newLang;
        await i18n.changeLanguage(target);

        // 立即同步写回后端（选做，auto-save effect 也会写）
        try {
            const full = await invoke("get_config");
            const launcher = full.launcher || {};
            launcher.language = newLang;
            await invoke("set_config", { key: "launcher", value: launcher });
        } catch (e) {
            console.error("Failed to persist language:", e);
        }
    };

    // helper to toggle setters (保持你原来的行为)
    const handleToggle = (setter, value) => setter(!value);

    const handleToggleDisableAllProxy = () => {
        const newValue = !disableAllProxy;
        setDisableAllProxy(newValue);
        if (newValue) {
            setUseSystemProxy(false);
            setEnableHttpProxy(false);
            setEnableSocksProxy(false);
            setEnableCustomProxy(false);
        }
    };

    // 构造 Select 的 options（包含 auto）
    const languageOptions = [
        { value: "auto", label: t("LauncherSettings.lang_options.auto") || "Auto" },
        ...Object.entries(SUPPORTED_LANGUAGES).map(([key, { label }]) => ({ value: key, label })),
    ];

    return (
        <div className="launch-settings">
            <div className="setting-item">
                <label>{t("LauncherSettings.debug")}</label>
                <Switch checked={debugMode} onChange={() => handleToggle(setDebugMode, debugMode)} />
            </div>

            <div className="setting-item">
                <label>{t("LauncherSettings.language")}</label>
                <Select
                    value={userLanguage}
                    onChange={handleLanguageChange}
                    options={languageOptions}
                    placeholder={t("LauncherSettings.lang_placeholder")}
                    inputStyle={{ height: '29px' }}
                    size={13}
                />
            </div>

            <div className="setting-item">
                <label>{t("LauncherSettings.custom_appx_api")}</label>
                {/* 修复 Input onChange：直接使用 setter */}
                <Input
                    type="text"
                    value={customAppxApi}
                    onChange={(e) => setCustomAppxApi(e.target.value)}
                    placeholder="https://..."
                    inputStyle={{ height: '29px' }}
                    size={12}
                />
            </div>

            <div className="setting-item">
                <label>{t("LauncherSettings.download.multi_thread")}</label>
                <Switch
                    checked={multiThread}
                    onChange={() => {
                        const next = !multiThread;
                        setMultiThread(next);
                        if (next) {
                            setAutoThreadCount(false);
                        }
                    }}
                />
            </div>

            {multiThread && (
                <div className="setting-item">
                    <label>{t("LauncherSettings.download.max_threads")}</label>
                    <div className="slider-container">
                        <input
                            type="range"
                            className="range-input"
                            min={1}
                            max={256}
                            value={maxThreads}
                            onChange={handleSliderChange}
                            style={{ "--slider-percent": `${pct}%` }}
                        />
                        <Input
                            type="number"
                            value={maxThreads}
                            onChange={(e) => {
                                const v = Math.min(
                                    256,
                                    Math.max(1, parseInt(e.target.value, 10) || 1)
                                );
                                setMaxThreads(v);
                                debouncedSave(v);
                            }}
                            style={{ width: '60px' }}
                            min={1}
                            max={256}
                            inputStyle={{ height: '29px' }}

                        />
                    </div>
                </div>
            )}

            <div className="setting-item">
                <label>{t("LauncherSettings.download.auto_thread_count")}</label>
                <Switch
                    checked={autoThreadCount}
                    onChange={() => {
                        const next = !autoThreadCount;
                        setAutoThreadCount(next);
                        if (next) {
                            setMultiThread(false);
                        }
                    }}
                />
            </div>

            <div className="setting-item">
                <label>{t("LauncherSettings.download.proxy.disable_all_proxy")}</label>
                <Switch
                    checked={disableAllProxy}
                    onChange={() => selectProxyMode("none")}
                />
            </div>

            {!disableAllProxy && (
                <>
                    <div className="setting-item">
                        <label>{t("LauncherSettings.download.proxy.use_system_proxy")}</label>
                        <Switch
                            checked={useSystemProxy}
                            onChange={() => selectProxyMode("system")}
                        />
                    </div>

                    <div className="setting-item">
                        <label>{t("LauncherSettings.download.proxy.enable_http_proxy")}</label>
                        <Switch
                            checked={enableHttpProxy}
                            onChange={() => selectProxyMode("http")}
                        />
                    </div>
                    {enableHttpProxy && (
                        <div className="setting-item">
                            <label>{t("LauncherSettings.download.proxy.http_proxy_url")}</label>
                            <Input
                                type="text"
                                value={httpProxyUrl}
                                onChange={(e) => setHttpProxyUrl(e.target.value)}
                                placeholder="https://..."
                                inputStyle={{ height: '29px' }}
                            />
                        </div>
                    )}

                    <div className="setting-item">
                        <label>{t("LauncherSettings.download.proxy.enable_socks_proxy")}</label>
                        <Switch
                            checked={enableSocksProxy}
                            onChange={() => selectProxyMode("socks")}
                        />
                    </div>
                    {enableSocksProxy && (
                        <div className="setting-item">
                            <label>{t("LauncherSettings.download.proxy.socks_proxy_url")}</label>
                            <Input
                                type="text"
                                value={socksProxyUrl}
                                onChange={(e) => setSocksProxyUrl(e.target.value)}
                                placeholder="socks5://..."
                                inputStyle={{ height: '29px' }}
                            />
                        </div>
                    )}

                    <div className="setting-item">
                        <label>{t("LauncherSettings.download.proxy.enable_custom_proxy")}</label>
                        <Switch
                            checked={enableCustomProxy}
                            onChange={() => selectProxyMode("custom")}
                        />
                    </div>
                    {enableCustomProxy && (
                        <div className="setting-item">
                            <label>{t("LauncherSettings.download.proxy.custom_proxy_url")}</label>
                            <Input
                                type="text"
                                value={customProxyUrl}
                                onChange={(e) => setCustomProxyUrl(e.target.value)}
                                placeholder="..."
                                inputStyle={{ height: '29px' }}
                            />
                        </div>
                    )}
                </>
            )}
        </div>
    );
}

export default Launcher;
