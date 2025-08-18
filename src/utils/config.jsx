import { invoke } from "@tauri-apps/api/core";

export async function getConfig() {
    try {
        const fullConfig = await invoke("get_config");
        return fullConfig;
    } catch (error) {
        console.error("Failed to fetch config:", error);
        throw error;
    }
}

/**
 * 通用保存配置字段
 * @param {string} key - 配置字段路径，如 "custom_style.theme_color"
 * @param {any} value - 要保存的值
 * @returns {Promise<void>}
 */
export async function setConfig(key, value) {
    try {
        await invoke("set_config", { key, value });
    } catch (error) {
        console.error(`Failed to set config [${key}]:`, error);
        throw error;
    }
}