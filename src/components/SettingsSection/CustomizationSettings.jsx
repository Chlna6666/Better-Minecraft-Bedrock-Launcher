import React, {useCallback, useEffect, useState} from "react";
import {convertFileSrc, invoke} from "@tauri-apps/api/core";
import {open} from '@tauri-apps/plugin-dialog';
import {getConfig} from "../../utils/config.jsx";
import Switch from "../UI/Switch.jsx";
import { useTranslation } from 'react-i18next';


function CustomizationSettings() {
    const { t, i18n } = useTranslation();
    const [themeColor, setThemeColor] = useState("#90c7a8");
    const [backgroundOption, setBackgroundOption] = useState("default");
    const [localImagePath, setLocalImagePath] = useState("");
    const [networkImageUrl, setNetworkImageUrl] = useState("");
    const [showLaunchAnimation, setShowLaunchAnimation] = useState(true);

    useEffect(() => {
        async function fetchConfig() {
            try {
                const fullConfig = await getConfig();
                const config = fullConfig.custom_style || {};

                setThemeColor(config.theme_color || "#90c7a8");
                setBackgroundOption(config.background_option || "default");
                setLocalImagePath(config.local_image_path || "");
                setNetworkImageUrl(config.network_image_url || "");
                setShowLaunchAnimation(config.show_launch_animation !== undefined ? config.show_launch_animation : true);

                await applyBackgroundStyle(
                    config.background_option || "default",
                    config.local_image_path || "",
                    config.network_image_url || ""
                );
            } catch (error) {
                console.error('Failed to fetch config:', error);
            }
        }

        fetchConfig();
    }, []);

    const applyBackgroundStyle = async (option, local_image_path, networkUrl) => {
        let backgroundImage = '';
        if (option === 'local' && local_image_path) {
            const fileDataUrl = convertFileSrc(local_image_path);
            backgroundImage = `url(${fileDataUrl})`;
        } else if (option === 'network' && networkUrl) {
            backgroundImage = `url(${networkUrl})`;
        }
        document.body.style.backgroundImage = backgroundImage;
        document.body.style.backgroundSize = 'cover';
        document.body.style.backgroundPosition = 'center';
        document.body.style.backgroundRepeat = 'no-repeat';
    }

    async function saveConfig(customStyle) {
        try {
            await invoke("set_config", { key: "custom_style", value: customStyle });
        } catch (error) {
            console.error("Failed to save config:", error);
        }
    }

    const handleColorChange = (event) => {
        const newColor = event.target.value;
        setThemeColor(newColor);
        document.documentElement.style.setProperty('--theme-color', newColor);
        saveConfig({ theme_color: newColor, background_option: backgroundOption, local_image_path: localImagePath, network_image_url: networkImageUrl, show_launch_animation: showLaunchAnimation });
    };

    const handleHexInputChange = (event) => {
        const hexValue = event.target.value;
        if (/^#[0-9A-Fa-f]{6}$/.test(hexValue)) {
            setThemeColor(hexValue);
            document.documentElement.style.setProperty('--theme-color', hexValue);
            saveConfig({ theme_color: hexValue, background_option: backgroundOption, local_image_path: localImagePath, network_image_url: networkImageUrl, show_launch_animation: showLaunchAnimation });
        }
    };

    const handleBackgroundChange = async (event) => {
        const newBackgroundOption = event.target.value;
        setBackgroundOption(newBackgroundOption);
        await saveConfig({
            theme_color: themeColor,
            background_option: newBackgroundOption,
            local_image_path: localImagePath,
            network_image_url: networkImageUrl,
            show_launch_animation: showLaunchAnimation
        });
        await applyBackgroundStyle(newBackgroundOption, localImagePath, networkImageUrl);
    };

    const handleLocalImageChange = async () => {
        try {
            const selected = await open({
                filters: [{ name: 'Image', extensions: ['jpg', 'jpeg', 'png', 'gif', 'webp'] }],
                multiple: false,
            });
            if (!selected) return;
            setLocalImagePath(selected);
            const updatedConfig = {
                theme_color: themeColor,
                background_option: backgroundOption,
                local_image_path: selected,
                network_image_url: networkImageUrl,
                show_launch_animation: showLaunchAnimation
            };
            await saveConfig(updatedConfig);
            await applyBackgroundStyle(backgroundOption, selected, networkImageUrl);
        } catch (error) {
            console.error('Failed to handle local image change:', error);
        }
    };

    const handleNetworkImageChange = async (event) => {
        const newUrl = event.target.value;
        setNetworkImageUrl(newUrl);
        await saveConfig({
            theme_color: themeColor,
            background_option: backgroundOption,
            local_image_path: localImagePath,
            network_image_url: newUrl,
            show_launch_animation: showLaunchAnimation
        });
        await applyBackgroundStyle(backgroundOption, localImagePath, newUrl);
    };

    const handleLaunchAnimationChange = async () => {
        const newValue = !showLaunchAnimation;
        setShowLaunchAnimation(newValue);
        await saveConfig({
            theme_color: themeColor,
            background_option: backgroundOption,
            local_image_path: localImagePath,
            network_image_url: networkImageUrl,
            show_launch_animation: newValue
        });
    };

    return (
        <>
            <div className="setting-item">
                <label>{t("CustomizationSettings.launch_animation")}</label>
                <Switch
                    checked={showLaunchAnimation}
                    onChange={handleLaunchAnimationChange}
                />
            </div>

            <div className="setting-item">
                <label>{t("CustomizationSettings.theme_color")}</label>
                <input
                    className="hex-input"
                    type="text"
                    value={themeColor}
                    onChange={handleHexInputChange}
                    placeholder="#5a87d6"
                    maxLength="7"
                />
                <input
                    className="color-input"
                    type="color"
                    value={themeColor}
                    onChange={handleColorChange}
                />
            </div>

            <div className="setting-item">
                <label>{t("CustomizationSettings.custom_background")}</label>
                <select
                    className="select-input"
                    value={backgroundOption}
                    onChange={handleBackgroundChange}
                >
                    <option value="default">{t("CustomizationSettings.background_options.default")}</option>
                    <option value="local">{t("CustomizationSettings.background_options.local")}</option>
                    <option value="network">{t("CustomizationSettings.background_options.network")}</option>
                </select>
            </div>

            {backgroundOption === "local" && (
                <div className="setting-item">
                    <label>{t("CustomizationSettings.local_image")}</label>
                    <div className="file-input-container">
                        <input
                            className="text-input"
                            type="text"
                            value={localImagePath}
                            readOnly
                            placeholder={t("CustomizationSettings.no_file")}
                        />
                        <button type="button" className="file-button" onClick={handleLocalImageChange}>
                            {t("CustomizationSettings.select_file")}
                        </button>
                    </div>
                </div>
            )}

            {backgroundOption === "network" && (
                <div className="setting-item">
                    <label>{t("CustomizationSettings.network_image")}</label>
                    <input
                        className="text-input"
                        type="text"
                        value={networkImageUrl}
                        onChange={handleNetworkImageChange}
                        placeholder={t("CustomizationSettings.network_placeholder")}
                    />
                </div>
            )}
        </>
    );
}

export default CustomizationSettings;
