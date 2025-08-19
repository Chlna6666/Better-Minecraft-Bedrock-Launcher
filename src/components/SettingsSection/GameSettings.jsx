import React, { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../../utils/config.jsx";
import Switch from "../UI/Switch.jsx";
import { useTranslation } from 'react-i18next';

const HOTKEY_OPTIONS = ["ALT", "CTRL", "SHIFT", "LWIN", "RWIN"];

function GameSettings() {
    const { t, i18n } = useTranslation();
    const [injectOnLaunch, setInjectOnLaunch] = useState(true);
    const [injectDelay, setInjectDelay] = useState(0);
    const [lockMouse, setLockMouse] = useState(false);
    const [unlockHotkey, setUnlockHotkey] = useState("ALT");
    const [launcherVis, setLauncherVis] = useState("keep");
    const [reducePixels, setReducePixels] = useState(0); // 新增 reducePixels 状态
    const [keepAppx, setKeepAppx] = useState(false);
    const [modifyAppxManifest, setModifyAppxManifest] = useState(true);


    const VISIBILITY_OPTIONS = [
        { value: "minimize", label: t("GameSettings.visibility.minimize") },
        { value: "close", label: t("GameSettings.visibility.close") },
        { value: "keep", label: t("GameSettings.visibility.keep") },
    ];

    useEffect(() => {
        (async () => {
            try {
                const config = await getConfig();
                const cfg = config.game || {};
                setInjectOnLaunch(cfg.inject_on_launch ?? true);
                setInjectDelay(cfg.inject_delay ?? 0);
                setLockMouse(cfg.lock_mouse_on_launch ?? false);
                setUnlockHotkey(cfg.unlock_mouse_hotkey ?? "ALT");
                setLauncherVis(cfg.launcher_visibility ?? "keep");
                setReducePixels(cfg.reduce_pixels ?? 0); // 加载 reduce_pixels 配置
                setKeepAppx(cfg.keep_appx_after_install ?? false);
                setModifyAppxManifest(cfg.modify_appx_manifest ?? true);


            } catch (e) {
                console.error("Failed to load game config", e);
            }
        })();
    }, []);

    const saveGameConfig = useCallback(async (updated) => {
        try {
            await invoke("set_config", { key: "game", value: updated });
        } catch (e) {
            console.error("Failed to save game config", e);
        }
    }, []);

    const updateConfig = (updatedFields) => {
        const updated = {
            inject_on_launch: injectOnLaunch,
            inject_delay: injectDelay,
            lock_mouse_on_launch: lockMouse,
            unlock_mouse_hotkey: unlockHotkey,
            launcher_visibility: launcherVis,
            reduce_pixels: reducePixels, // 更新 reduce_pixels 配置
            keep_appx_after_install: keepAppx,
            modify_appx_manifest: modifyAppxManifest,
            ...updatedFields,
        };
        saveGameConfig(updated);
    };

    return (
        <div className="game-settings section">
            {/* 注入开关 */}
            <div className="setting-item">
                <label>{t("GameSettings.inject_dll")}</label>
                <Switch
                    checked={injectOnLaunch}
                    onChange={() => {
                        const next = !injectOnLaunch;
                        setInjectOnLaunch(next);
                        updateConfig({ inject_on_launch: next });
                    }}
                />
            </div>

            {/* 延迟输入框 */}
            {injectOnLaunch && (
                <div className="setting-item">
                    <label>{t("GameSettings.inject_delay")}</label>
                    <input
                        type="number"
                        min="0"
                        value={injectDelay}
                        onChange={e => {
                            const val = parseInt(e.target.value, 10) || 0;
                            setInjectDelay(val);
                            updateConfig({ inject_delay: val });
                        }}
                        className="text-input"
                    />
                </div>
            )}

            {/* 锁鼠标开关及热键选择 */}
            {launcherVis !== "close" && (
                <>
                    <div className="setting-item">
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

                    {lockMouse && (
                        <>
                            {/* 自定义裁剪像素输入框 */}
                            <div className="setting-item">
                                <label>{t("GameSettings.reduce_pixels")}</label>
                                <input
                                    type="number"
                                    min="0"
                                    value={reducePixels}
                                    onChange={e => {
                                        const val = parseInt(e.target.value, 10) || 0;
                                        setReducePixels(val);
                                        updateConfig({ reduce_pixels: val });
                                    }}
                                    className="text-input"
                                />
                            </div>

                            {/* 解锁热键选择 */}
                            <div className="setting-item">
                                <label>{t("GameSettings.unlock_hotkey")}</label>
                                <select
                                    className="text-input"
                                    value={unlockHotkey}
                                    onChange={e => {
                                        const val = e.target.value;
                                        setUnlockHotkey(val);
                                        updateConfig({ unlock_mouse_hotkey: val });
                                    }}
                                >
                                    {HOTKEY_OPTIONS.map(k => (
                                        <option key={k} value={k}>{k}</option>
                                    ))}
                                </select>
                            </div>
                        </>
                    )}
                </>
            )}

            {/* 只保留这一处 launcher_visibility */}
            <div className="setting-item">
                <label>{t("GameSettings.launcher_visibility")}</label>
                <select
                    className="text-input"
                    value={launcherVis}
                    onChange={e => {
                        const val = e.target.value;
                        setLauncherVis(val);
                        updateConfig({ launcher_visibility: val });
                    }}
                >
                    {VISIBILITY_OPTIONS.map(({ value, label }) => (
                        <option key={value} value={value}>{label}</option>
                    ))}
                </select>
            </div>
            {/* APPX 配置 */}
            <div className="setting-item">
                <label>{t("GameSettings.keep_appx_after_install")}</label>
                <Switch
                    checked={keepAppx}
                    onChange={() => {
                        const next = !keepAppx;
                        setKeepAppx(next);
                        updateConfig({ keep_appx_after_install: next });
                    }}
                />
            </div>

            {/* 修改 AppxManifest 开关 */}
            <div className="setting-item">
                <label>{t("GameSettings.modify_appx_manifest")}</label>
                <Switch
                    checked={modifyAppxManifest}
                    onChange={() => {
                        const next = !modifyAppxManifest;
                        setModifyAppxManifest(next);
                        updateConfig({ modify_appx_manifest: next });
                    }}
                />
            </div>


        </div>
    );
}

export default GameSettings;
