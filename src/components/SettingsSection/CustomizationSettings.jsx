import React, { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./CustomizationSettings.css";

function CustomizationSettings() {
    const [themeColor, setThemeColor] = useState("#90c7a8");
    const [backgroundOption, setBackgroundOption] = useState("default");
    const [localImagePath, setLocalImagePath] = useState("");
    const [networkImageUrl, setNetworkImageUrl] = useState("");
    const [showLaunchAnimation, setShowLaunchAnimation] = useState(true);

    useEffect(() => {
        async function fetchConfig() {
            try {
                const config = await invoke("get_custom_style");
                setThemeColor(config.theme_color || "#90c7a8");
                setBackgroundOption(config.background_option || "default");
                setLocalImagePath(config.local_image_path || "");
                setNetworkImageUrl(config.network_image_url || "");
                setShowLaunchAnimation(config.show_launch_animation !== undefined ? config.show_launch_animation : true);

                // Ensure the background is applied after the config is fetched
                applyBackgroundStyle(config.background_option || "default", config.local_image_path || "", config.network_image_url || "");
            } catch (error) {
                console.error('Failed to fetch config:', error);
            }
        }
        fetchConfig();
    }, []);

    const applyBackgroundStyle = (option, localPath, networkUrl) => {
        let backgroundImage = '';

        if (option === 'local' && localPath) {
            backgroundImage = `url(${localPath})`;
        } else if (option === 'network' && networkUrl) {
            backgroundImage = `url(${networkUrl})`;
        } else {
            backgroundImage = ''; // No image or default image can be set here
        }

        document.body.style.backgroundImage = backgroundImage;
        document.body.style.backgroundSize = 'cover';
        document.body.style.backgroundPosition = 'center';
        document.body.style.backgroundRepeat = 'no-repeat';
    };

    async function saveConfig(config) {
        try {
            console.log("Saving config with:", { customStyle: config });
            await invoke('set_custom_style', { customStyle: config });
            console.log('Config saved successfully');
        } catch (error) {
            console.error('Failed to save config:', error);
        }
    }

    const debounceSaveConfig = useCallback((updatedConfig, delay = 500) => {
        const handler = setTimeout(() => {
            saveConfig(updatedConfig);
        }, delay);

        return () => clearTimeout(handler);
    }, []);

    const handleColorChange = (event) => {
        const newColor = event.target.value;
        setThemeColor(newColor);
        document.documentElement.style.setProperty('--theme-color', newColor);
        debounceSaveConfig({
            theme_color: newColor,
            background_option: backgroundOption,
            local_image_path: localImagePath,
            network_image_url: networkImageUrl,
            show_launch_animation: showLaunchAnimation
        });
    };

    const handleHexInputChange = (event) => {
        const hexValue = event.target.value;
        if (/^#[0-9A-Fa-f]{6}$/.test(hexValue)) {
            setThemeColor(hexValue);
            document.documentElement.style.setProperty('--theme-color', hexValue);
            debounceSaveConfig({
                theme_color: hexValue,
                background_option: backgroundOption,
                local_image_path: localImagePath,
                network_image_url: networkImageUrl,
                show_launch_animation: showLaunchAnimation
            });
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
        applyBackgroundStyle(newBackgroundOption, localImagePath, networkImageUrl);
    };

    const handleLocalImageChange = async (event) => {
        const file = event.target.files[0];
        if (file) {
            const reader = new FileReader();
            reader.onloadend = async () => {
                const fileDataUrl = reader.result;
                setLocalImagePath(fileDataUrl);
                await saveConfig({
                    theme_color: themeColor,
                    background_option: backgroundOption,
                    local_image_path: fileDataUrl,
                    network_image_url: networkImageUrl,
                    show_launch_animation: showLaunchAnimation
                });
                applyBackgroundStyle(backgroundOption, fileDataUrl, networkImageUrl);
            };
            reader.readAsDataURL(file);
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
        applyBackgroundStyle(backgroundOption, localImagePath, newUrl);
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
        <div>
            <div className="setting-item">
                <label htmlFor="launch-animation-toggle">启动时显示启动动画:</label>
                <div className="switch">
                    <input
                        id="launch-animation-toggle"
                        className="switch-input"
                        type="checkbox"
                        checked={showLaunchAnimation}
                        onChange={handleLaunchAnimationChange}
                    />
                    <span className="slider round"></span>
                </div>
            </div>

            <div className="setting-item">
                <label htmlFor="theme-color-picker">主题颜色:</label>
                <input
                    className="hex-input"
                    type="text"
                    value={themeColor}
                    onChange={handleHexInputChange}
                    placeholder="#5a87d6"
                    maxLength="7"
                />
                <input
                    id="theme-color-picker"
                    className="color-input"
                    type="color"
                    value={themeColor}
                    onChange={handleColorChange}
                />
            </div>

            <div className="setting-item">
                <label htmlFor="background-option">自定义背景:</label>
                <select
                    id="background-option"
                    className="select-input"
                    value={backgroundOption}
                    onChange={handleBackgroundChange}
                >
                    <option value="default">默认</option>
                    <option value="local">本地图片</option>
                    <option value="network">网络图片</option>
                </select>

            </div>

            {backgroundOption === "local" && (
                <div className="setting-item">
                    <label htmlFor="local-image-path">选择本地图片:</label>
                    <div className="file-input-container">
                        <input
                            id="local-image-path"
                            className="text-input"
                            type="text"
                            value={localImagePath}
                            readOnly
                            placeholder="未选择文件"
                        />
                        <input
                            type="file"
                            accept="image/*"
                            onChange={handleLocalImageChange}
                            id="file-input"
                        />
                        <label htmlFor="file-input" className="file-button">
                            选择文件
                        </label>
                    </div>
                </div>
            )}

            {backgroundOption === "network" && (
                <div className="setting-item">
                    <label htmlFor="network-image-url">输入网络图片链接:</label>
                    <input
                        id="network-image-url"
                        className="text-input"
                        type="text"
                        value={networkImageUrl}
                        onChange={handleNetworkImageChange}
                        placeholder="输入网络图片链接"
                    />
                </div>
            )}
        </div>
    );
}

export default CustomizationSettings;
