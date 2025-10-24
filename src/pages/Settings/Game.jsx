import React, { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../../utils/config.jsx";
import Switch from "../../components/Switch.jsx";
import Select from "../../components/Select.jsx";
import { useTranslation } from 'react-i18next';
import {Input} from "../../components/index.js";

const HOTKEY_OPTIONS = ["ALT", "CTRL", "SHIFT", "LWIN", "RWIN"];

function Game() {
    const { t, i18n } = useTranslation();
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

    useEffect(() => {
        (async () => {
            try {
                const config = await getConfig();
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
        <div className="game-settings">
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
                                <Input
                                    type="number"
                                    value={reducePixels}
                                    onChange={e => {
                                        const val = parseInt(e.target.value, 10) || 0;
                                        setReducePixels(val);
                                        updateConfig({ reduce_pixels: val });
                                    }}
                                    style={{ width: '80px' }}
                                    inputStyle={{ height: '29px' }}

                                />
                            </div>

                            {/* 解锁热键选择（替换为组件） */}
                            <div className="setting-item">
                                <label>{t("GameSettings.unlock_hotkey")}</label>
                                <Select
                                    value={unlockHotkey}
                                    onChange={val => {
                                        setUnlockHotkey(val);
                                        updateConfig({ unlock_mouse_hotkey: val });
                                    }}
                                    options={HOTKEY_OPTIONS}
                                    size={14}
                                />
                            </div>
                        </>
                    )}
                </>
            )}

            <div className="setting-item">
                <label>{t("GameSettings.launcher_visibility")}</label>
                <Select
                    value={launcherVis}
                    onChange={val => {
                        setLauncherVis(val);
                        updateConfig({ launcher_visibility: val });
                    }}
                    options={VISIBILITY_OPTIONS}
                    dropdownMatchButton={false}
                    size={13}
                />
            </div>

            {/* UWP 最小化修复开关 */}
            <div className="setting-item">
                <label>{t("GameSettings.uwp_minimize_fix")}</label>
                <Switch
                    checked={uwpMinimizeFix}
                    onChange={() => {
                        const next = !uwpMinimizeFix;
                        setUwpMinimizeFix(next);
                        updateConfig({ uwp_minimize_fix: next });
                    }}
                />
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

export default Game;
