/* src/utils/fileActions.ts */
import { invoke } from '@tauri-apps/api/core';

/**
 * 调用后端的 open_path 指令打开文件夹
 * 后端已处理 Windows 下的路径前缀兼容性问题
 */
export async function openFolder(path: string): Promise<void> {
    if (!path) {
        throw new Error("路径为空");
    }

    const normalizedPath = path.trim().replace(/\//g, '\\');

    console.log("[fileActions] Invoking open_path with:", normalizedPath);

    try {
        await invoke('open_path', { path: normalizedPath });
    } catch (e: any) {
        console.error("Failed to open path:", e);
        throw new Error(typeof e === 'string' ? e : (e.message || "无法打开文件夹"));
    }
}