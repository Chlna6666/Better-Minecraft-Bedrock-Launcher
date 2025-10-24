import React, { useEffect, useRef, useState, useLayoutEffect } from "react";
import { createPortal } from "react-dom";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from '@tauri-apps/plugin-dialog';
import { getConfig } from "../../utils/config.jsx";
import Switch from "../../components/Switch.jsx";
import Select from "../../components/Select.jsx";
import Input from "../../components/Input.jsx";
import { useTranslation } from 'react-i18next';
import { SketchPicker } from 'react-color';
import { useToast } from "../../components/Toast.jsx";
import {Button} from "../../components/index.js"; // optional, used for feedback

// Helpers
function componentToHex(c) { const hex = c.toString(16); return hex.length === 1 ? '0' + hex : hex; }
function rgbaToHex8({ r, g, b, a }) { const alpha = Math.round((a ?? 1) * 255); return `#${componentToHex(r)}${componentToHex(g)}${componentToHex(b)}${componentToHex(alpha)}`.toLowerCase(); }
function hex8ToRgba(hex) {
    if (!hex) return { r: 0, g: 0, b: 0, a: 1 };
    const h = hex.replace('#', '');
    if (h.length === 6) {
        return { r: parseInt(h.slice(0,2),16), g: parseInt(h.slice(2,4),16), b: parseInt(h.slice(4,6),16), a: 1 };
    }
    if (h.length === 8) {
        return { r: parseInt(h.slice(0,2),16), g: parseInt(h.slice(2,4),16), b: parseInt(h.slice(4,6),16), a: parseInt(h.slice(6,8),16) / 255 };
    }
    return { r: 144, g: 199, b: 168, a: 1 };
}
function rgbaToCssString({ r, g, b, a }) { return `rgba(${r}, ${g}, ${b}, ${a})`; }
function ensureHexPrefix(s) { if (!s) return ''; return s.startsWith('#') ? s : `#${s}`; }

export default function Customization() {
    const { t } = useTranslation();
    const toast = useToast();

    const [color, setColor] = useState({ r: 144, g: 199, b: 168, a: 1 });
    const [hexInput, setHexInput] = useState('#90c7a8');
    const [backgroundOption, setBackgroundOption] = useState('default');
    const [localImagePath, setLocalImagePath] = useState('');
    const [networkImageUrl, setNetworkImageUrl] = useState('');
    const [showLaunchAnimation, setShowLaunchAnimation] = useState(true);

    const [pickerOpen, setPickerOpen] = useState(false);
    const [pickerPos, setPickerPos] = useState({ top: 0, left: 0 });
    const buttonRef = useRef(null);
    const pickerRef = useRef(null);

    useEffect(() => {
        async function fetchConfig() {
            try {
                const fullConfig = await getConfig();
                const config = fullConfig.custom_style || {};
                const savedColor = config.theme_color || '#90c7a8';
                const parsed = hex8ToRgba(savedColor);
                setColor(parsed);
                setHexInput((savedColor || '#90c7a8').toLowerCase());
                setBackgroundOption(config.background_option || 'default');
                setLocalImagePath(config.local_image_path || '');
                setNetworkImageUrl(config.network_image_url || '');
                setShowLaunchAnimation(config.show_launch_animation !== undefined ? config.show_launch_animation : true);
                await applyBackgroundStyle(config.background_option || 'default', config.local_image_path || '', config.network_image_url || '');
                document.documentElement.style.setProperty('--theme-color', rgbaToCssString(parsed));
            } catch (error) {
                console.error('Failed to fetch config:', error);
            }
        }
        fetchConfig();
    }, []);

    // compute panel position relative to viewport, avoid overflow
    const computePickerPos = () => {
        const btn = buttonRef.current;
        if (!btn) return { top: 80, left: 80 };
        const rect = btn.getBoundingClientRect();
        const margin = 8;
        const approxW = 250;
        const approxH = 340;
        const vw = window.innerWidth;
        const vh = window.innerHeight;
        let left = rect.left;
        let top = rect.bottom + margin;

        if (left + approxW > vw - 8) left = Math.max(8, vw - approxW - 8);
        if (top + approxH > vh - 8) top = Math.max(8, rect.top - approxH - margin);
        return { top, left };
    };

    useLayoutEffect(() => {
        if (pickerOpen) {
            setPickerPos(computePickerPos());
        }
    }, [pickerOpen]);

    useEffect(() => {
        function onDocClick(e) {
            if (!pickerOpen) return;
            if (pickerRef.current && pickerRef.current.contains(e.target)) return;
            if (buttonRef.current && buttonRef.current.contains(e.target)) return;
            setPickerOpen(false);
        }
        function onScrollOrResize() {
            if (pickerOpen) setPickerPos(computePickerPos());
        }
        document.addEventListener('mousedown', onDocClick);
        window.addEventListener('resize', onScrollOrResize);
        window.addEventListener('scroll', onScrollOrResize, true);
        return () => {
            document.removeEventListener('mousedown', onDocClick);
            window.removeEventListener('resize', onScrollOrResize);
            window.removeEventListener('scroll', onScrollOrResize, true);
        };
    }, [pickerOpen]);

    const applyBackgroundStyle = async (option, local_image_path, networkUrl) => {
        let backgroundImage = '';
        if (option === 'local' && local_image_path) {
            try {
                // Try convertFileSrc (works for paths returned by Tauri dialog)
                const fileDataUrl = convertFileSrc(local_image_path);
                backgroundImage = `url(${fileDataUrl})`;
            } catch (err) {
                // Fallback: if path looks like a file:// URL or absolute path, try to use it directly
                if (local_image_path.startsWith('file://')) {
                    backgroundImage = `url(${local_image_path})`;
                } else {
                    // Leave blank; caller should handle error notification
                    throw new Error('无法预览本地文件（请使用选择文件按钮或粘贴由应用返回的路径）');
                }
            }
        } else if (option === 'network' && networkUrl) {
            backgroundImage = `url(${networkUrl})`;
        } else {
            backgroundImage = '';
        }

        document.body.style.backgroundImage = backgroundImage;
        document.body.style.backgroundSize = backgroundImage ? 'cover' : '';
        document.body.style.backgroundPosition = backgroundImage ? 'center' : '';
        document.body.style.backgroundRepeat = backgroundImage ? 'no-repeat' : '';
    };

    async function saveConfig(customStyle) {
        try {
            await invoke('set_config', { key: 'custom_style', value: customStyle });
        } catch (error) {
            console.error('Failed to save config:', error);
            throw error;
        }
    }

    const handlePickerChange = (newColor) => {
        const { r, g, b, a } = newColor.rgb;
        setColor({ r, g, b, a });
        const hex8 = rgbaToHex8({ r, g, b, a });
        setHexInput(hex8);
        document.documentElement.style.setProperty('--theme-color', rgbaToCssString({ r, g, b, a }));
        // Save config (fire-and-forget)
        saveConfig({
            theme_color: hex8,
            background_option: backgroundOption,
            local_image_path: localImagePath,
            network_image_url: networkImageUrl,
            show_launch_animation: showLaunchAnimation
        }).catch(() => {});
    };

    const handleHexInputChange = (e) => {
        const val = e.target.value.trim().toLowerCase();
        setHexInput(val);
        const cleaned = val.replace('#', '');
        if (/^[0-9a-f]{6}$/.test(cleaned) || /^[0-9a-f]{8}$/.test(cleaned)) {
            const hex = ensureHexPrefix(`#${cleaned}`);
            const rgba = hex8ToRgba(hex);
            setColor(rgba);
            document.documentElement.style.setProperty('--theme-color', rgbaToCssString(rgba));
            saveConfig({
                theme_color: hex,
                background_option: backgroundOption,
                local_image_path: localImagePath,
                network_image_url: networkImageUrl,
                show_launch_animation: showLaunchAnimation
            }).catch(() => {});
        }
    };

    const handleColorButtonClick = () => {
        setPickerOpen(v => !v);
    };

    // Background select
    const backgroundOptions = [
        { label: t("CustomizationSettings.background_options.default"), value: 'default' },
        { label: t("CustomizationSettings.background_options.local"), value: 'local' },
        { label: t("CustomizationSettings.background_options.network"), value: 'network' },
    ];

    const handleBackgroundSelect = async (valOrEvent) => {
        let newVal = valOrEvent;
        try {
            if (valOrEvent && typeof valOrEvent === 'object') {
                if ('value' in valOrEvent) newVal = valOrEvent.value;
                else if (valOrEvent.target && 'value' in valOrEvent.target) newVal = valOrEvent.target.value;
            }
        } catch (e) {}
        setBackgroundOption(newVal);

        const updated = {
            theme_color: rgbaToHex8(color),
            background_option: newVal,
            local_image_path: localImagePath,
            network_image_url: networkImageUrl,
            show_launch_animation: showLaunchAnimation
        };

        try {
            await saveConfig(updated);
            await applyBackgroundStyle(newVal, localImagePath, networkImageUrl);
            toast?.success(t('common.save_success') || '已保存');
        } catch (err) {
            console.error('apply background failed', err);
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (err?.message || err));
        }
    };

    // Select file dialog
    const handleLocalImageChange = async () => {
        try {
            const selected = await open({
                filters: [{ name: 'Image', extensions: ['jpg', 'jpeg', 'png', 'gif', 'webp'] }],
                multiple: false,
            });
            if (!selected) return;
            const path = Array.isArray(selected) ? selected[0] : selected;
            setLocalImagePath(path);

            const updated = {
                theme_color: rgbaToHex8(color),
                background_option: 'local',
                local_image_path: path,
                network_image_url: networkImageUrl,
                show_launch_animation: showLaunchAnimation
            };
            await saveConfig(updated);
            await applyBackgroundStyle('local', path, networkImageUrl);
            setBackgroundOption('local');
            toast?.success(t('common.save_success') || '已保存');
        } catch (error) {
            console.error('Failed to handle local image change:', error);
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (error?.message || error));
        }
    };

    // Allow editing the local path input
    const handleLocalPathChange = (e) => {
        setLocalImagePath(e.target.value);
    };

    const applyLocalPathInput = async (path) => {
        const trimmed = (path || '').trim();
        if (!trimmed) {
            toast?.error(t('CustomizationSettings.enter_path') || '请填写本地图片路径');
            return;
        }

        try {
            const updated = {
                theme_color: rgbaToHex8(color),
                background_option: 'local',
                local_image_path: trimmed,
                network_image_url: networkImageUrl,
                show_launch_animation: showLaunchAnimation
            };
            await saveConfig(updated);
            await applyBackgroundStyle('local', trimmed, networkImageUrl);
            setBackgroundOption('local');
            toast?.success(t('common.save_success') || '已保存');
        } catch (err) {
            console.error('Failed to apply local path:', err);
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (err?.message || err));
        }
    };

    // Drag & drop support for local image input
    const handleLocalDrop = async (e) => {
        e.preventDefault();
        e.stopPropagation();
        const dt = e.dataTransfer;
        if (!dt || !dt.files || dt.files.length === 0) return;
        // In Tauri this File object often contains a `path` property. Fallback to name if not present.
        const file = dt.files[0];
        const path = file?.path || file?.name;
        if (!path) return;
        setLocalImagePath(path);
        await applyLocalPathInput(path);
    };
    const handleLocalDragOver = (e) => {
        e.preventDefault();
    };

    const handleNetworkImageChange = async (event) => {
        const newUrl = event.target.value;
        setNetworkImageUrl(newUrl);
        try {
            const updated = {
                theme_color: rgbaToHex8(color),
                background_option: backgroundOption,
                local_image_path: localImagePath,
                network_image_url: newUrl,
                show_launch_animation: showLaunchAnimation
            };
            await saveConfig(updated);
            await applyBackgroundStyle(backgroundOption, localImagePath, newUrl);
            toast?.success(t('common.saved') || 'save_success');
        } catch (err) {
            console.error('Failed to apply network image url', err);
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (err?.message || err));
        }
    };

    const handleLaunchAnimationChange = async () => {
        const newValue = !showLaunchAnimation;
        setShowLaunchAnimation(newValue);
        try {
            await saveConfig({
                theme_color: rgbaToHex8(color),
                background_option: backgroundOption,
                local_image_path: localImagePath,
                network_image_url: networkImageUrl,
                show_launch_animation: newValue
            });
            toast?.success(t('common.save_success') || '已保存');
        } catch (err) {
            console.error('Failed to save launch animation', err);
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (err?.message || err));
        }
    };

    // Picker portal
    const pickerPortal = pickerOpen ? createPortal(
        <div
            ref={pickerRef}
            style={{
                position: 'fixed',
                top: `${pickerPos.top}px`,
                left: `${pickerPos.left}px`,
                zIndex: 9999,
                boxShadow: '0 6px 18px rgba(0,0,0,0.16)'
            }}
        >
            <SketchPicker color={color} onChange={handlePickerChange} disableAlpha={false} />
        </div>,
        document.body
    ) : null;

    return (
        <div className="custom-settings">
            <div className="setting-item" style={{ overflow: 'visible' }}>
                <label>{t("CustomizationSettings.launch_animation")}</label>
                <Switch checked={showLaunchAnimation} onChange={handleLaunchAnimationChange} />
            </div>

            <div className="setting-item">
                <label>{t("CustomizationSettings.theme_color")}</label>

                <Input
                    type="text"
                    className="input-with-swatch"
                    value={hexInput}
                    onChange={handleHexInputChange}
                    style={{ width: '150px' }}
                    inputStyle={{ height: '29px' }}
                    suffix={
                        <button
                            ref={buttonRef}
                            type="button"
                            onMouseDown={(e) => e.preventDefault()}
                            onClick={handleColorButtonClick}
                            aria-label="打开颜色选择器"
                            className="color-swatch-btn"
                            style={{
                                width: 23,
                                height: 23,
                                padding: 0,
                                borderRadius: 6,
                                border: '0px',
                                background: rgbaToCssString(color),
                                boxShadow: 'inset 0 0 0 1px rgba(0,0,0,0.04)',
                                cursor: 'pointer',
                            }}
                        />
                    }
                />
                {pickerPortal}
            </div>

            <div className="setting-item">
                <label>{t("CustomizationSettings.custom_background")}</label>
                <Select
                    value={backgroundOption}
                    onChange={handleBackgroundSelect}
                    options={backgroundOptions}
                    disabled={false}
                    optionKey="value"
                    size={13}
                />
            </div>

            {backgroundOption === "local" && (
                <div className="setting-item">
                    <label>{t("CustomizationSettings.local_image")}</label>
                    <div
                        className="file-input-container"
                        onDrop={handleLocalDrop}
                        onDragOver={handleLocalDragOver}
                        style={{ display: 'flex', gap: 8, alignItems: 'center' }}
                    >
                        <Input
                            className="text-input-wrapper"
                            inputClassName="text-input"
                            type="text"
                            value={localImagePath}
                            placeholder={t("CustomizationSettings.no_file")}
                            onChange={handleLocalPathChange}
                            onBlur={() => applyLocalPathInput(localImagePath)}
                            onKeyDown={(e) => { if (e.key === 'Enter') applyLocalPathInput(localImagePath); }}
                            style={{ flex: 1 }}
                            inputStyle={{ height: '29px' }}
                        />
                        <Button
                            type="button"
                            onClick={handleLocalImageChange}
                            variant="ghost"      // 可选：'primary' | 'ghost' | 'danger' | 'success'
                            size="md"            // 可选：'sm' | 'md' | 'lg'
                            ariaLabel={t("CustomizationSettings.select_file")}
                        >
                            {t("CustomizationSettings.select_file")}
                        </Button>

                    </div>
                </div>
            )}

            {backgroundOption === "network" && (
                <div className="setting-item">
                    <label>{t("CustomizationSettings.network_image")}</label>
                    <Input
                        className="text-input-wrapper"
                        inputClassName="text-input"
                        type="text"
                        value={networkImageUrl}
                        onChange={(e) => setNetworkImageUrl(e.target.value)}
                        onBlur={handleNetworkImageChange}
                        placeholder={t("CustomizationSettings.network_placeholder")}
                        inputStyle={{ height: '29px' }}
                    />
                </div>
            )}
        </div>
    );
}
