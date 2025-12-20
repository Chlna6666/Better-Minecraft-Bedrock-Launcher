import React, { useEffect, useRef, useState, useLayoutEffect } from "react";
import { createPortal } from "react-dom";
import { motion, AnimatePresence } from "framer-motion";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from '@tauri-apps/plugin-dialog';
import { getConfig } from "../../utils/config";
import Switch from "../../components/Switch";
import Select from "../../components/Select";
import Input from "../../components/Input";
import { useTranslation } from 'react-i18next';
import { SketchPicker, ColorResult } from 'react-color';
import { useToast } from "../../components/Toast";
import { Button } from "../../components";

// ==========================================
// 辅助函数 (保持不变)
// ==========================================
function componentToHex(c: number) { const hex = c.toString(16); return hex.length === 1 ? '0' + hex : hex; }
function rgbaToHex8({ r, g, b, a }: any) { const alpha = Math.round((a ?? 1) * 255); return `#${componentToHex(r)}${componentToHex(g)}${componentToHex(b)}${componentToHex(alpha)}`.toLowerCase(); }
function hex8ToRgba(hex: string) {
    if (!hex) return { r: 0, g: 0, b: 0, a: 1 };
    const h = hex.replace('#', '');
    if (h.length === 6) {
        return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16), a: 1 };
    }
    if (h.length === 8) {
        return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16), a: parseInt(h.slice(6, 8), 16) / 255 };
    }
    return { r: 144, g: 199, b: 168, a: 1 };
}
function rgbaToCssString({ r, g, b, a }: any) { return `rgba(${r}, ${g}, ${b}, ${a})`; }
function ensureHexPrefix(s: string) { if (!s) return ''; return s.startsWith('#') ? s : `#${s}`; }

// ==========================================
// 动画变体
// ==========================================
const pageVariants = {
    initial: { opacity: 0, y: 10, scale: 0.98 },
    animate: {
        opacity: 1,
        y: 0,
        scale: 1,
        transition: { duration: 0.4, ease: [0.25, 1, 0.5, 1], staggerChildren: 0.05 }
    },
    exit: {
        opacity: 0,
        y: -10,
        scale: 0.98,
        transition: { duration: 0.2 }
    }
};

const itemVariants = {
    initial: { opacity: 0, y: 10 },
    animate: { opacity: 1, y: 0, transition: { type: "spring", stiffness: 300, damping: 30 } }
};

export default function Customization() {
    const { t } = useTranslation();
    const toast = useToast();

    // ==========================================
    // State
    // ==========================================
    const [color, setColor] = useState({ r: 144, g: 199, b: 168, a: 1 });
    const [hexInput, setHexInput] = useState('#90c7a8');
    const [backgroundOption, setBackgroundOption] = useState('default');
    const [localImagePath, setLocalImagePath] = useState('');
    const [networkImageUrl, setNetworkImageUrl] = useState('');
    const [showLaunchAnimation, setShowLaunchAnimation] = useState(true);

    const [pickerOpen, setPickerOpen] = useState(false);
    const [pickerPos, setPickerPos] = useState({ top: 0, left: 0 });
    const buttonRef = useRef<HTMLButtonElement>(null);
    const pickerRef = useRef<HTMLDivElement>(null);

    // ==========================================
    // Effects
    // ==========================================
    useEffect(() => {
        async function fetchConfig() {
            try {
                const fullConfig: any = await getConfig();
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
        function onDocClick(e: MouseEvent) {
            if (!pickerOpen) return;
            if (pickerRef.current && pickerRef.current.contains(e.target as Node)) return;
            if (buttonRef.current && buttonRef.current.contains(e.target as Node)) return;
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

    // ==========================================
    // Logic Functions
    // ==========================================
    const applyBackgroundStyle = async (option: string, local_image_path: string, networkUrl: string) => {
        let backgroundImage = '';
        if (option === 'local' && local_image_path) {
            try {
                const fileDataUrl = convertFileSrc(local_image_path);
                backgroundImage = `url(${fileDataUrl})`;
            } catch (err) {
                if (local_image_path.startsWith('file://')) {
                    backgroundImage = `url(${local_image_path})`;
                } else {
                    throw new Error('无法预览本地文件');
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

    async function saveConfig(customStyle: any) {
        try {
            await invoke('set_config', { key: 'custom_style', value: customStyle });
        } catch (error) {
            console.error('Failed to save config:', error);
            throw error;
        }
    }

    const handlePickerChange = (newColor: ColorResult) => {
        const { r, g, b, a } = newColor.rgb;
        setColor({ r, g, b, a });
        const hex8 = rgbaToHex8({ r, g, b, a });
        setHexInput(hex8);
        document.documentElement.style.setProperty('--theme-color', rgbaToCssString({ r, g, b, a }));
        saveConfig({
            theme_color: hex8,
            background_option: backgroundOption,
            local_image_path: localImagePath,
            network_image_url: networkImageUrl,
            show_launch_animation: showLaunchAnimation
        }).catch(() => { });
    };

    const handleHexInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
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
            }).catch(() => { });
        }
    };

    const backgroundOptions = [
        { label: t("CustomizationSettings.background_options.default"), value: 'default' },
        { label: t("CustomizationSettings.background_options.local"), value: 'local' },
        { label: t("CustomizationSettings.background_options.network"), value: 'network' },
    ];

    const handleBackgroundSelect = async (valOrEvent: any) => {
        let newVal = valOrEvent;
        try {
            if (valOrEvent && typeof valOrEvent === 'object') {
                if ('value' in valOrEvent) newVal = valOrEvent.value;
                else if (valOrEvent.target && 'value' in valOrEvent.target) newVal = valOrEvent.target.value;
            }
        } catch (e) { }
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
        } catch (err: any) {
            console.error('apply background failed', err);
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (err?.message || err));
        }
    };

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
        } catch (error: any) {
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (error?.message || error));
        }
    };

    const applyLocalPathInput = async (path: string) => {
        const trimmed = (path || '').trim();
        if (!trimmed) return;
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
        } catch (err: any) {
            toast?.error((t('common.save_failed') || '保存失败') + ': ' + (err?.message || err));
        }
    };


    return (
        <motion.div
            className="settings-inner-container"
            variants={pageVariants}
            initial="initial"
            animate="animate"
            exit="exit"
        >
            <motion.h3 className="settings-group-title">{t("Settings.tabs.customization")}</motion.h3>

            {/* 1. 启动动画 */}
            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("CustomizationSettings.launch_animation")}</label>
                <Switch checked={showLaunchAnimation} onChange={async () => {
                    const newValue = !showLaunchAnimation;
                    setShowLaunchAnimation(newValue);
                    await saveConfig({
                        theme_color: rgbaToHex8(color),
                        background_option: backgroundOption,
                        local_image_path: localImagePath,
                        network_image_url: networkImageUrl,
                        show_launch_animation: newValue
                    });
                }} />
            </motion.div>

            {/* 2. 主题色 */}
            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("CustomizationSettings.theme_color")}</label>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                    <button
                        ref={buttonRef}
                        type="button"
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={() => setPickerOpen(v => !v)}
                        className="color-swatch-btn"
                        style={{
                            width: 32,
                            height: 32,
                            padding: 0,
                            borderRadius: 8,
                            border: '2px solid rgba(255,255,255,0.5)',
                            background: rgbaToCssString(color),
                            boxShadow: '0 2px 5px rgba(0,0,0,0.1)',
                            cursor: 'pointer',
                        }}
                    />
                    <Input
                        type="text"
                        value={hexInput}
                        onChange={handleHexInputChange}
                        style={{ width: '100px' }}
                    />
                </div>
                {pickerOpen && createPortal(
                    <div ref={pickerRef} style={{ position: 'fixed', top: `${pickerPos.top}px`, left: `${pickerPos.left}px`, zIndex: 9999 }}>
                        <SketchPicker color={color} onChange={handlePickerChange} disableAlpha={false} />
                    </div>,
                    document.body
                )}
            </motion.div>

            {/* 3. 背景设置 (分组样式) */}
            <motion.div variants={itemVariants} className="setting-item grouped">
                {/* 组头：背景源选择 */}
                <div className="setting-header">
                    <label>{t("CustomizationSettings.custom_background")}</label>
                    <Select
                        value={backgroundOption}
                        onChange={handleBackgroundSelect}
                        options={backgroundOptions}
                        optionKey="value"
                        size={13}
                    />
                </div>

                {/* 子组：具体设置 (本地路径 或 网络链接) */}
                {(backgroundOption === 'local' || backgroundOption === 'network') && (
                    <div className="setting-sub-group">
                        <div className="sub-group-spacer"></div>

                        {/* 本地图片设置行 */}
                        {backgroundOption === 'local' && (
                            <div className="sub-setting-row">
                                <label>{t("CustomizationSettings.local_image")}</label>
                                {/* 这里不使用 .sub-control-wrapper 因为需要更宽的空间 */}
                                <div style={{ display: 'flex', gap: 8, flex: 1, justifyContent: 'flex-end', alignItems: 'center' }}>
                                    <Input
                                        type="text"
                                        value={localImagePath}
                                        placeholder={t("CustomizationSettings.no_file")}
                                        onChange={(e) => setLocalImagePath(e.target.value)}
                                        onBlur={() => applyLocalPathInput(localImagePath)}
                                        style={{ width: '100%', minWidth: 0, flex: 1 }}
                                    />
                                    <Button type="button" onClick={handleLocalImageChange} variant="ghost" size="sm">
                                        {t("CustomizationSettings.select_file")}
                                    </Button>
                                </div>
                            </div>
                        )}

                        {/* 网络图片设置行 */}
                        {backgroundOption === 'network' && (
                            <div className="sub-setting-row">
                                <label>{t("CustomizationSettings.network_image")}</label>
                                <div style={{ display: 'flex', flex: 1, justifyContent: 'flex-end' }}>
                                    <Input
                                        type="text"
                                        value={networkImageUrl}
                                        onChange={(e) => setNetworkImageUrl(e.target.value)}
                                        onBlur={(e) => applyBackgroundStyle(backgroundOption, localImagePath, e.target.value)}
                                        placeholder={t("CustomizationSettings.network_placeholder")}
                                        style={{ width: '100%', minWidth: 0, flex: 1 }}
                                    />
                                </div>
                            </div>
                        )}
                    </div>
                )}
            </motion.div>
        </motion.div>
    );
}