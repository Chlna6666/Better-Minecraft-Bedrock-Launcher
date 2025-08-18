import React, {useState, useEffect, useCallback, useRef} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../../utils/config.jsx";
import Switch from "../UI/Switch.jsx";
import {useTranslation} from "react-i18next";
import {SUPPORTED_LANGUAGES} from "../../i18n/i18n.js";


function LauncherSettings() {
    const { t, i18n } = useTranslation();
    const [loaded, setLoaded] = useState(false);
    const [debugMode, setDebugMode] = useState(false);
    const [language, setLanguage] = useState("auto");
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

    const selectProxyMode = useCallback((mode) => {
        if (mode === "none") {
            // 如果当前已经是“none”，再次点击就切换到默认代理
            if (disableAllProxy) {
                selectProxyMode(DEFAULT_PROXY);
            } else {
                // 正常进入 none
                setDisableAllProxy(true);
                setUseSystemProxy(false);
                setEnableHttpProxy(false);
                setEnableSocksProxy(false);
                setEnableCustomProxy(false);
            }
            return;
        }
        // 任何具体模式，都先关闭 none，再打开对应模式
        setDisableAllProxy(false);
        setUseSystemProxy(mode === "system");
        setEnableHttpProxy(mode === "http");
        setEnableSocksProxy(mode === "socks");
        setEnableCustomProxy(mode === "custom");
    }, [disableAllProxy,
        setDisableAllProxy,
        setUseSystemProxy,
        setEnableHttpProxy,
        setEnableSocksProxy,
        setEnableCustomProxy
    ]);

    useEffect(() => {
        async function fetchConfig() {
            try {
                const fullConfig = await getConfig();
                const launcher = fullConfig.launcher || {};
                const download = launcher.download || {};
                const proxy = download.proxy || {};

                setDebugMode(launcher.debug || false);
                setLanguage(launcher.language || "auto");
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



    // 监听所有状态变化后保存配置
    useEffect(() => {
        if (!loaded) return; // 避免首次加载时保存默认值

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

        async function saveLauncherConfig() {
            try {
                await invoke("set_config", { key: "launcher", value: updated });
            } catch (e) {
                console.error("Failed to save launcher config:", e);
            }
        }


        saveLauncherConfig();
    }, [
        loaded, debugMode, language, customAppxApi, multiThread, autoThreadCount, maxThreads,
        disableAllProxy, useSystemProxy, enableHttpProxy, httpProxyUrl,
        enableSocksProxy, socksProxyUrl, enableCustomProxy, customProxyUrl
    ]);

    const [userLanguage, setUserLanguage] = useState("auto");

    // 首次加载：读配置 + 切换语言
    useEffect(() => {
        (async () => {
            const full = await invoke("get_config");
            const lang = full.launcher?.language || "auto";
            setUserLanguage(lang);

            // 只要是 “auto” 就去调后端命令拿系统语言
            const target =
                lang === "auto"
                    ? await invoke("get_system_language")
                    : lang;
            await i18n.changeLanguage(target);
        })();
    }, []);

    // 防抖：500ms 内多次调用只执行最后一次
    const saveRef = useRef(null);
    const debouncedSave = useCallback((value) => {
        if (saveRef.current) clearTimeout(saveRef.current);
        saveRef.current = setTimeout(async () => {
            // 调用后端保存 max_threads 字段
            const full = await invoke("get_config");
            const launcher = full.launcher || {};
            launcher.download = launcher.download || {};
            launcher.download.max_threads = value;
            await invoke("set_config", { key: "launcher", value: launcher });
        }, 500);
    }, []);

    // 计算 CSS 变量
    const sliderMin = 1;
    const sliderMax = 256;
    const pct = ((maxThreads - sliderMin) / (sliderMax - sliderMin)) * 100;

    // 首次从配置加载
    useEffect(() => {
        (async () => {
            const full = await invoke("get_config");
            const v = full.launcher?.download?.max_threads ?? 8;
            setMaxThreads(v);
        })();
    }, []);

    const handleSliderChange = (e) => {
        const v = Math.min(sliderMax, Math.max(sliderMin, parseInt(e.target.value, 10) || sliderMin));
        setMaxThreads(v);
        debouncedSave(v);
    };

    // 下拉改变时
    const handleChange = async (e) => {
        const lang = e.target.value;
        setUserLanguage(lang);

        const target =
            lang === "auto"
                ? await invoke("get_system_language")
                : lang;
        await i18n.changeLanguage(target);

        // 写回完整 launcher 对象
        const full = await invoke("get_config");
        const launcher = full.launcher || {};
        launcher.language = lang;
        await invoke("set_config", { key: "launcher", value: launcher });
    };
    const handleToggle = (setter, value) => setter(!value);

    const handleToggleDisableAllProxy = () => {
        const newValue = !disableAllProxy;
        setDisableAllProxy(newValue);
        if (newValue) {
            // 自动关闭其他代理
            setUseSystemProxy(false);
            setEnableHttpProxy(false);
            setEnableSocksProxy(false);
            setEnableCustomProxy(false);
        }
    };


    return (
        <>
            <div className="setting-item">
                <label>{t("LauncherSettings.debug")}</label>
                <Switch checked={debugMode} onChange={() => handleToggle(setDebugMode, debugMode)} />
            </div>
            <div className="setting-item">
                <label>{t("LauncherSettings.language")}</label>
                <select className="select-input" value={userLanguage} onChange={handleChange}>
                    <option value="auto">{t("LauncherSettings.lang_options.auto")}</option>
                    {Object.entries(SUPPORTED_LANGUAGES).map(([key, {label}]) => (
                        <option key={key} value={key}>{label}</option>
                    ))}
                </select>
            </div>
            <div className="setting-item">
                <label>{t("LauncherSettings.custom_appx_api")}</label>
                <input
                    className="text-input"
                    type="text"
                    value={customAppxApi}
                    onChange={(e) => handleChange(setCustomAppxApi, e)}
                    placeholder="https://..."
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
                        <input
                            type="number"
                            className="slider-value"
                            min={1}
                            max={256}
                            value={maxThreads}
                            onChange={(e) => {
                                const v = Math.min(
                                    256,
                                    Math.max(1, parseInt(e.target.value, 10) || 1)
                                );
                                setMaxThreads(v);
                                debouncedSave(v);
                            }}
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
                        // 如果开启自动线程数，就关闭多线程
                        if (next) {
                            setMultiThread(false);
                        }
                    }}
                />
            </div>



            {/* 关闭所有代理 */}
            <div className="setting-item">
                <label>{t("LauncherSettings.download.proxy.disable_all_proxy")}</label>
                <Switch
                    checked={disableAllProxy}
                    onChange={() => selectProxyMode("none")}
                />
            </div>

            {/* 只有当不关闭所有代理时，才允许选择其中一种 */}
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
                            <input
                                className="text-input"
                                type="text"
                                value={httpProxyUrl}
                                onChange={(e) => handleChange(setHttpProxyUrl, e)}
                                placeholder="http://..."
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
                            <input
                                className="text-input"
                                type="text"
                                value={socksProxyUrl}
                                onChange={(e) => handleChange(setSocksProxyUrl, e)}
                                placeholder="socks5://..."
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
                            <input
                                className="text-input"
                                type="text"
                                value={customProxyUrl}
                                onChange={(e) => handleChange(setCustomProxyUrl, e)}
                                placeholder="..."
                            />
                        </div>
                    )}
                </>
            )}
        </>
    );
}

export default LauncherSettings;
