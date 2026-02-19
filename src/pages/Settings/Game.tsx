import React, { useEffect, useState, useCallback } from "react";
import { motion } from "framer-motion";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../../utils/config";
import Switch from "../../components/Switch";
import Select from "../../components/Select";
import { useTranslation } from 'react-i18next';

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
                const config: any = await getConfig();
                const cfg = config.game || {};
                setLauncherVis(cfg.launcher_visibility ?? "keep");
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
            launcher_visibility: launcherVis,
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
