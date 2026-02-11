// src/types/manage.ts

export interface MinecraftVersion {
    uuid: string;
    name: string;
    version: string;
    path: string;
    // 其他字段...
}

export interface AssetFile {
    name: string;
    path: string;
    size?: string;
    image?: string; // 对于地图或材质包可能有图标
    description?: string;
    isEnabled?: boolean; // 针对材质包或模组
}

export type AssetType = 'map' | 'resource_pack' | 'behavior_pack' | 'dll_mod';