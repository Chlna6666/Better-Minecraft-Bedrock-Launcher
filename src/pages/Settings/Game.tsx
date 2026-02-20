import React, { useEffect, useState, useCallback } from "react";
import { motion } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../../utils/config";
import Switch from "../../components/Switch";
import Select from "../../components/Select";
import { useTranslation } from 'react-i18next';
import SettingText from "./SettingText";

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
    const [launcherVis, setLauncherVis] = useState("keep");
    const [keepDownloadedGamePackage, setKeepDownloadedGamePackage] = useState(false);
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
                const config: any = await getConfig();
                const cfg = config.game || {};
                setLauncherVis(cfg.launcher_visibility ?? "keep");
                setKeepDownloadedGamePackage(
                    cfg.keep_downloaded_game_package ?? cfg.keep_appx_after_install ?? false
                );
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
            launcher_visibility: launcherVis,
            keep_downloaded_game_package: keepDownloadedGamePackage,
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
                <SettingText
                    title={t("GameSettings.launcher_visibility")}
                    desc={(() => {
                        const key = "GameSettings.launcher_visibility_desc";
                        const val = t(key);
                        return val === key ? undefined : val;
                    })()}
                />
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
                <SettingText
                    title={t("GameSettings.uwp_minimize_fix")}
                    desc={(() => {
                        const key = "GameSettings.uwp_minimize_fix_desc";
                        const val = t(key);
                        return val === key ? undefined : val;
                    })()}
                />
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
                <SettingText
                    title={t("GameSettings.keep_downloaded_game_package")}
                    desc={(() => {
                        const key = "GameSettings.keep_downloaded_game_package_desc";
                        const val = t(key);
                        return val === key ? undefined : val;
                    })()}
                />
                <Switch
                    checked={keepDownloadedGamePackage}
                    onChange={() => {
                        const next = !keepDownloadedGamePackage;
                        setKeepDownloadedGamePackage(next);
                        updateConfig({ keep_downloaded_game_package: next });
                    }}
                />
            </motion.div>

            <motion.div variants={itemVariants} className="setting-item">
                <SettingText
                    title={t("GameSettings.modify_appx_manifest")}
                    desc={(() => {
                        const key = "GameSettings.modify_appx_manifest_desc";
                        const val = t(key);
                        return val === key ? undefined : val;
                    })()}
                />
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
