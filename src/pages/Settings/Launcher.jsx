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
    const [autoCheckUpdates, setAutoCheckUpdates] = useState(true);


    // --- 新的代理状态（基于 ProxyConfig） ---
    // proxyType: "none" | "system" | "http" | "socks5"
    const [proxyType, setProxyType] = useState("system");
    const [httpProxyUrl, setHttpProxyUrl] = useState("");
    const [socksProxyUrl, setSocksProxyUrl] = useState("");

    const DEFAULT_PROXY = "system";

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
                setMultiThread(download.multi_thread || false);
                setAutoThreadCount(download.auto_thread_count || true);
                setMaxThreads(download.max_threads || 8);

                // 新的代理读取：proxy.proxy_type 以及 url 字段
                const pt = (proxy.proxy_type || "system").toLowerCase();
                setProxyType(["none","system","http","socks5"].includes(pt) ? pt : DEFAULT_PROXY);
                setHttpProxyUrl(proxy.http_proxy_url || "");
                setSocksProxyUrl(proxy.socks_proxy_url || "");

                setAutoCheckUpdates(launcher.auto_check_updates || true);

                setLoaded(true);
            } catch (e) {
                console.error("Failed to load launcher config:", e);
            }
        }
        fetchConfig();
    }, []);

    useEffect(() => {
        if (!loaded) return;

        (async () => {
            try {
                // 先拿到完整的配置
                const full = await invoke("get_config");
                const launcher = full.launcher || {};

                // 我们把要修改的字段写入 launcher（保留 launcher 里其他字段）
                launcher.debug = debugMode;
                launcher.language = language;
                launcher.custom_appx_api = customAppxApi;
                launcher.auto_check_updates = autoCheckUpdates;

                launcher.download = launcher.download || {};
                launcher.download.multi_thread = multiThread;
                launcher.download.max_threads = maxThreads;
                launcher.download.auto_thread_count = autoThreadCount;

                launcher.download.proxy = launcher.download.proxy || {};
                launcher.download.proxy.proxy_type = proxyType;
                launcher.download.proxy.http_proxy_url = httpProxyUrl;
                launcher.download.proxy.socks_proxy_url = socksProxyUrl;

                // 最后一次性写回完整的 launcher 对象（包含后端原本有的字段）
                await invoke("set_config", { key: "launcher", value: launcher });
            } catch (e) {
                console.error("Failed to save launcher config:", e);
            }
        })();
    }, [
        loaded, debugMode, language, customAppxApi, multiThread, autoThreadCount, maxThreads,
        proxyType, httpProxyUrl, socksProxyUrl, autoCheckUpdates
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

    // ---- language change handler ----
    const handleLanguageChange = async (newLang) => {
        setUserLanguage(newLang);
        setLanguage(newLang);
        const target = newLang === "auto" ? await invoke("get_system_language") : newLang;
        await i18n.changeLanguage(target);
        try {
            const full = await invoke("get_config");
            const launcher = full.launcher || {};
            launcher.language = newLang;
            await invoke("set_config", { key: "launcher", value: launcher });
        } catch (e) {
            console.error("Failed to persist language:", e);
        }
    };

    const handleToggle = (setter, value) => setter(!value);

    const proxyOptions = [
        { value: "none", label: t("LauncherSettings.download.proxy.none")},
        { value: "system", label: t("LauncherSettings.download.proxy.system")},
        { value: "http", label: t("LauncherSettings.download.proxy.http")},
        { value: "socks5", label: t("LauncherSettings.download.proxy.socks5")},
    ];


    const handleProxyTypeChange = (value) => {
        setProxyType(value);
    };

    return (
        <div className="launch-settings">
            <div className="setting-item">
                <label>{t("LauncherSettings.debug")}</label>
                <Switch checked={debugMode} onChange={() => handleToggle(setDebugMode, debugMode)} />
            </div>

            <div className="setting-item">
                <label>{t("LauncherSettings.auto_check_updates")}</label>
                <Switch
                    checked={autoCheckUpdates}
                    onChange={() => setAutoCheckUpdates(!autoCheckUpdates)}
                />
            </div>


            <div className="setting-item">
                <label>{t("LauncherSettings.language")}</label>
                <Select
                    value={userLanguage}
                    onChange={handleLanguageChange}
                    options={[
                        { value: "auto", label: t("LauncherSettings.lang_options.auto") || "Auto" },
                        ...Object.entries(SUPPORTED_LANGUAGES).map(([key, { label }]) => ({ value: key, label })),
                    ]}
                    placeholder={t("LauncherSettings.lang_placeholder")}
                    inputStyle={{ height: '29px' }}
                    size={13}
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

            {/* --- 新：代理选择器 --- */}
            <div className="setting-item">
                <label>{t("LauncherSettings.download.proxy.mode") || "Proxy Mode"}</label>
                <Select
                    value={proxyType}
                    onChange={handleProxyTypeChange}
                    options={proxyOptions}
                    placeholder={t("LauncherSettings.download.proxy.mode_placeholder") || "Select proxy mode"}
                    inputStyle={{ height: '29px' }}
                    size={13}
                />
            </div>

            {/* 根据 proxyType 显示对应 URL 输入 */}
            {proxyType === "http" && (
                <div className="setting-item">
                    <label>{t("LauncherSettings.download.proxy.http_proxy_url")}</label>
                    <Input
                        type="text"
                        value={httpProxyUrl}
                        onChange={(e) => setHttpProxyUrl(e.target.value)}
                        placeholder="http(s)://host:port"
                        inputStyle={{ height: '29px' }}
                    />
                </div>
            )}

            {proxyType === "socks5" && (
                <div className="setting-item">
                    <label>{t("LauncherSettings.download.proxy.socks_proxy_url")}</label>
                    <Input
                        type="text"
                        value={socksProxyUrl}
                        onChange={(e) => setSocksProxyUrl(e.target.value)}
                        placeholder="socks5://host:port"
                        inputStyle={{ height: '29px' }}
                    />
                </div>
            )}
        </div>
    );
}

export default Launcher;
