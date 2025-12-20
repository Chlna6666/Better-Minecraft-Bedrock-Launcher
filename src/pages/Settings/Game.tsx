import React, { useEffect, useState, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../../utils/config";
import Switch from "../../components/Switch";
import Select from "../../components/Select";
import { useTranslation } from 'react-i18next';
import { Input } from "../../components";

// 动画配置
const pageVariants = {
    initial: { opacity: 0, y: 10 },
    animate: {
        opacity: 1,
        y: 0,
        transition: { duration: 0.3, ease: "easeOut", staggerChildren: 0.05 }
    },
    exit: { opacity: 0, y: -10 }
};

const itemVariants = {
    initial: { opacity: 0, y: 10 },
    animate: { opacity: 1, y: 0 }
};

export default function Game() {
    const { t } = useTranslation();
    const [injectOnLaunch, setInjectOnLaunch] = useState(true);
    const [lockMouse, setLockMouse] = useState(false);
    const [unlockHotkey, setUnlockHotkey] = useState("ALT");
    const [launcherVis, setLauncherVis] = useState("keep");
    const [reducePixels, setReducePixels] = useState(0);
    const [keepAppx, setKeepAppx] = useState(false);
    const [modifyAppxManifest, setModifyAppxManifest] = useState(true);
    const [uwpMinimizeFix, setUwpMinimizeFix] = useState(true);

    const VISIBILITY_OPTIONS = [
        { value: "minimize", label: t("GameSettings.visibility.minimize") },
        { value: "close", label: t("GameSettings.visibility.close") },
        { value: "keep", label: t("GameSettings.visibility.keep") },
    ];

    const HOTKEY_OPTIONS = [
        { value: "ALT", label: "ALT" },
        { value: "CTRL", label: "CTRL" },
        { value: "SHIFT", label: "SHIFT" },
        { value: "LWIN", label: "LWIN" },
        { value: "RWIN", label: "RWIN" }
    ];

    useEffect(() => {
        (async () => {
            try {
                const config: any = await getConfig();
                const cfg = config.game || {};
                setInjectOnLaunch(cfg.inject_on_launch ?? true);
                setLockMouse(cfg.lock_mouse_on_launch ?? false);
                setUnlockHotkey(cfg.unlock_mouse_hotkey ?? "ALT");
                setLauncherVis(cfg.launcher_visibility ?? "keep");
                setReducePixels(cfg.reduce_pixels ?? 0);
                setKeepAppx(cfg.keep_appx_after_install ?? false);
                setModifyAppxManifest(cfg.modify_appx_manifest ?? true);
                setUwpMinimizeFix(cfg.uwp_minimize_fix ?? true);
            } catch (e) {
                console.error("Failed to load game config", e);
            }
        })();
    }, []);

    const saveGameConfig = useCallback(async (updated: any) => {
        try {
            await invoke("set_config", { key: "game", value: updated });
        } catch (e) {
            console.error("Failed to save game config", e);
        }
    }, []);

    const updateConfig = (updatedFields: any) => {
        const updated = {
            inject_on_launch: injectOnLaunch,
            lock_mouse_on_launch: lockMouse,
            unlock_mouse_hotkey: unlockHotkey,
            launcher_visibility: launcherVis,
            reduce_pixels: reducePixels,
            keep_appx_after_install: keepAppx,
            modify_appx_manifest: modifyAppxManifest,
            uwp_minimize_fix: uwpMinimizeFix,
            ...updatedFields,
        };
        saveGameConfig(updated);
    };

    return (
        <motion.div
            className="settings-inner-container"
            variants={pageVariants}
            initial="initial"
            animate="animate"
            exit="exit"
        >
            <motion.h3 className="settings-group-title">{t("Settings.tabs.game")}</motion.h3>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("GameSettings.inject_dll")}</label>
                <Switch
                    checked={injectOnLaunch}
                    onChange={() => {
                        const next = !injectOnLaunch;
                        setInjectOnLaunch(next);
                        updateConfig({ inject_on_launch: next });
                    }}
                />
            </motion.div>

            {/* --- 分组卡片 --- */}
            {launcherVis !== "close" && (
                <motion.div
                    variants={itemVariants}
                    className="setting-item grouped"
                >
                    <div className="setting-header">
                        <label>{t("GameSettings.lock_mouse")}</label>
                        <Switch
                            checked={lockMouse}
                            onChange={() => {
                                const next = !lockMouse;
                                setLockMouse(next);
                                updateConfig({ lock_mouse_on_launch: next });
                            }}
                        />
                    </div>

                    <AnimatePresence initial={false}>
                        {lockMouse && (
                            <motion.div
                                initial={{ height: 0, opacity: 0 }}
                                animate={{
                                    height: "auto",
                                    opacity: 1,
                                    transition: {
                                        height: { duration: 0.3, ease: "easeOut" },
                                        opacity: { duration: 0.2, delay: 0.1 }
                                    }
                                }}
                                exit={{
                                    height: 0,
                                    opacity: 0,
                                    transition: {
                                        height: { duration: 0.2, ease: "easeInOut" },
                                        opacity: { duration: 0.1 }
                                    }
                                }}
                                style={{ overflow: "hidden" }}
                            >
                                <div className="setting-sub-group">
                                    <div className="sub-group-spacer" />

                                    <div className="sub-setting-row">
                                        <label>{t("GameSettings.reduce_pixels")}</label>
                                        {/* [修改] 增加包裹层，统一宽度管理 */}
                                        <div className="sub-control-wrapper">
                                            <Input
                                                type="number"
                                                value={reducePixels}
                                                onChange={(e: any) => {
                                                    const val = parseInt(e.target.value, 10) || 0;
                                                    setReducePixels(val);
                                                    updateConfig({ reduce_pixels: val });
                                                }}
                                                // 确保 Input 自身占满包裹层
                                                style={{ width: '100%', textAlign: 'right' }}
                                            />
                                        </div>
                                    </div>

                                    <div className="sub-setting-row">
                                        <label>{t("GameSettings.unlock_hotkey")}</label>
                                        {/* [修改] 增加包裹层，统一宽度管理 */}
                                        <div className="sub-control-wrapper">
                                            <Select
                                                value={unlockHotkey}
                                                onChange={(val: any) => {
                                                    setUnlockHotkey(val);
                                                    updateConfig({ unlock_mouse_hotkey: val });
                                                }}
                                                options={HOTKEY_OPTIONS}
                                                size={14}
                                            />
                                        </div>
                                    </div>
                                </div>
                            </motion.div>
                        )}
                    </AnimatePresence>
                </motion.div>
            )}

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("GameSettings.launcher_visibility")}</label>
                <Select
                    value={launcherVis}
                    onChange={(val: any) => {
                        setLauncherVis(val);
                        updateConfig({ launcher_visibility: val });
                    }}
                    options={VISIBILITY_OPTIONS}
                    dropdownMatchButton={false}
                    size={13}
                />
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("GameSettings.uwp_minimize_fix")}</label>
                <Switch
                    checked={uwpMinimizeFix}
                    onChange={() => {
                        const next = !uwpMinimizeFix;
                        setUwpMinimizeFix(next);
                        updateConfig({ uwp_minimize_fix: next });
                    }}
                />
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("GameSettings.keep_appx_after_install")}</label>
                <Switch
                    checked={keepAppx}
                    onChange={() => {
                        const next = !keepAppx;
                        setKeepAppx(next);
                        updateConfig({ keep_appx_after_install: next });
                    }}
                />
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item">
                <label>{t("GameSettings.modify_appx_manifest")}</label>
                <Switch
                    checked={modifyAppxManifest}
                    onChange={() => {
                        const next = !modifyAppxManifest;
                        setModifyAppxManifest(next);
                        updateConfig({ modify_appx_manifest: next });
                    }}
                />
            </motion.div>
        </motion.div>
    );
}