// File: /src/pages/Manage/versionManage/api/assetApi.js
// Unified front-end API wrapper for deleting game assets
// Usage: import { deleteAsset } from './api/assetApi';
//        await deleteAsset(params);

import { invoke } from '@tauri-apps/api/core';

/**
 * deleteAsset params (standardized for new backend):
 * {
 * kind: 'gdk' | 'uwp',           // version.kind
 * userId?: string | null,        // GDK User ID
 * folder?: string | null,        // version.folder (version_name / isolation_id)
 * edition?: 'release' | 'preview' | 'education' | 'education_preview',
 * deleteType: 'maps'|'resourcePacks'|'behaviorPacks'|'skins'|'mods',
 * name: string,                  // target folder name
 * enableIsolation?: boolean      // new flag
 * }
 *
 * Returns: { success: boolean, message?: string }
 */
export async function deleteAsset(params = {}) {
    const {
        kind,
        userId = null,
        folder = null,
        edition = null,
        deleteType,
        name,
        enableIsolation = false
    } = params || {};

    if (!kind || !deleteType || !name) {
        throw new Error('deleteAsset: missing required params (kind, deleteType, name)');
    }

    // Helper: Normalize edition string to match backend enum (snake_case)
    const normalizeEdition = (e) => {
        const v = String(e || '').toLowerCase();
        if (v === 'release' || v === 'preview' || v === 'education' || v === 'education_preview') {
            return v;
        }
        if (v.includes('education')) {
            return v.includes('preview') ? 'education_preview' : 'education';
        }
        return v.includes('preview') ? 'preview' : 'release';
    };

    // Construct the payload matching the Rust struct `DeleteAssetPayload`
    // Note: The fields INSIDE the payload object must be snake_case to match Rust struct fields by default
    const payloadData = {
        build_type: String(kind || 'uwp').toLowerCase(),
        edition: normalizeEdition(edition),
        version_name: String(folder || ''),
        enable_isolation: !!enableIsolation,
        user_id: userId || null,
        delete_type: String(deleteType || ''),
        name: String(name || ''),
    };

    try {
        // Rust signature: fn delete_game_asset(payload: DeleteAssetPayload)
        // Tauri invoke: passing an object with key `payload`
        const res = await invoke('delete_game_asset', { payload: payloadData });
        return res;
    } catch (e) {
        throw new Error(`deleteAsset failed: ${e?.message || String(e)}`);
    }
}

export async function importAssets(params = {}) {
    const {
        kind,
        userId = null,
        folder = null,
        edition = null,
        enableIsolation = false,
        filePaths = [],
        overwrite = false, // [新增] 覆盖选项
        allowSharedFallback = false
    } = params || {};

    if (!filePaths || filePaths.length === 0) {
        throw new Error('No files selected');
    }

    const normalizeEdition = (e) => {
        const v = String(e || '').toLowerCase();
        if (v.includes('education')) return v.includes('preview') ? 'education_preview' : 'education';
        return v.includes('preview') ? 'preview' : 'release';
    };

    // 构造 Rust 结构体 ImportAssetPayload
    const payloadData = {
        build_type: String(kind || 'uwp').toLowerCase(),
        edition: normalizeEdition(edition),
        version_name: String(folder || ''),
        enable_isolation: !!enableIsolation,
        user_id: userId || null,
        file_paths: filePaths,
        overwrite: overwrite,
        allow_shared_fallback: !!allowSharedFallback
    };

    try {
        // 调用 import_assets
        const res = await invoke('import_assets', { payload: payloadData });
        return res;
    } catch (e) {
        throw new Error(`Import failed: ${e?.message || String(e)}`);
    }
}

// [新增] 检查导入冲突
export async function checkImportConflict(params = {}) {
    const {
        kind,
        userId = null,
        folder = null,
        edition = null,
        enableIsolation = false,
        filePath,
        allowSharedFallback = false
    } = params || {};

    if (!filePath) {
        throw new Error('No file selected');
    }

    const normalizeEdition = (e) => {
        const v = String(e || '').toLowerCase();
        if (v.includes('education')) return v.includes('preview') ? 'education_preview' : 'education';
        return v.includes('preview') ? 'preview' : 'release';
    };

    const payloadData = {
        build_type: String(kind || 'uwp').toLowerCase(),
        edition: normalizeEdition(edition),
        version_name: String(folder || ''),
        enable_isolation: !!enableIsolation,
        user_id: userId || null,
        file_path: filePath,
        allow_shared_fallback: !!allowSharedFallback
    };

    try {
        const res = await invoke('check_import_conflict', { payload: payloadData });
        return res;
    } catch (e) {
        throw new Error(`Check failed: ${e?.message || String(e)}`);
    }
}
