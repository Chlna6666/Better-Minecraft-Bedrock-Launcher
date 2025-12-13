// File: /src/pages/Manage/versionManage/api/assetApi.js
// Unified front-end API wrapper for deleting game assets (maps, templates, packs, skins, mods)
// Usage: import { deleteAsset } from './api/assetApi';
//        await deleteAsset(params);

import { invoke } from '@tauri-apps/api/core';

/**
 * deleteAsset params (all fields required unless noted):
 * {
 *   kind: 'gdk' | 'uwp',           // 版本类型
 *   userId?: string | number|null, // 当 kind==='gdk' 且需要针对某个用户时传入（gdk 用户 id 或 null）
 *   folder?: string | null,        // 传入版本文件夹名（即 VersionManagePage 的 folder）
 *   edition?: 'release' | 'preview' | string, // 版本通道（用于 GDK 路径拼接）
 *   deleteType: 'maps'|'mapTemplates'|'resourcePacks'|'behaviorPacks'|'skins'|'mods',
 *   name: string                   // 要删除的文件/目录名称（仅文件夹名即可）
 * }
 *
 * Returns: { success: boolean, message?: string, details?: any }
 */
export async function deleteAsset(params = {}) {
    const {
        kind,
        userId = null,
        folder = null,
        edition = null,
        deleteType,
        name,
    } = params || {};

    if (!kind || !deleteType || !name) {
        throw new Error('deleteAsset: missing required params (kind, deleteType, name)');
    }

    // Normalize payload to backend expected keys
    const payload = {
        version_type: String(kind || '').toLowerCase(),
        user_id: userId ?? null,
        folder: folder ?? null,
        edition: edition ?? null,
        delete_type: String(deleteType || ''),
        name: String(name || ''),
    };

    try {
        const res = await invoke('delete_game_asset', payload);
        // expected backend to return { success: true } or throw
        return res;
    } catch (e) {
        // rethrow with more context
        throw new Error(`deleteAsset failed: ${e?.message || String(e)}`);
    }
}


/* --------------------------
   Example front-end usage (not part of module):

   import { deleteAsset } from './api/assetApi';

   // GDK map delete (per-user)
   await deleteAsset({
     kind: 'gdk',
     userId: 12345,
     folder: '1.19.2-GDK',
     edition: 'release',
     deleteType: 'maps',
     name: 'My Cool World'
   });

   // UWP resource pack delete (no userId needed)
   await deleteAsset({
     kind: 'uwp',
     folder: null,
     edition: null,
     deleteType: 'resourcePacks',
     name: 'MyResourcePack'
   });

   --------------------------
   Backend notes (Tauri Rust command name: `delete_game_asset`)
   - For GDK: backend should prefer versions_root like Path::new("./BMCBL/versions") + folder when version isolation is implemented.
     If not implemented, fallback to std::env::var("APPDATA") to locate the games/com.mojang paths. For GDK per your spec:
       versions_root + folder + (Minecraft Bedrock|Minecraft Bedrock Preview mapping) + \\Users\\<user_id_or_folder>\\games\\com.mojang\\<delete-dir>
     mapping table (deleteType -> directory):
       maps -> minecraftWorlds
       mapTemplates -> world_templates
       skins -> skin_packs
       behaviorPacks -> behavior_packs (note: some shared paths are under Users/Shared)
       resourcePacks -> resource_packs (shared location)
       mods -> versions_root + folder + '/mods/'  (mods handled specially; may be skipped)

   - For UWP: use LocalState under Packages/Microsoft.MinecraftUWP_8wekyb3d8bbwe/LocalState/games/com.mojang
     For preview UWP use Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe.

   - Backend must validate inputs and ensure path traversal safety (reject ../ sequences), and only delete directories matching the provided name.
   - Backend should return JSON { success: bool, message?: string }.

   -------------------------- */
