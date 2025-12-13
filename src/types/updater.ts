// src/types/updater.ts

export interface ReleaseData {
    tag: string;
    name: string;
    body: string;
    published_at: string;
    prerelease: boolean;
    asset_name?: string;
    asset_url?: string;
    asset_size?: number;
}

// 新增：对应 Rust 的 TaskSnapshot
export interface TaskSnapshot {
    id: string;
    stage: string;
    total: number | null;      // 总大小 (可能未知)
    done: number;              // 已下载大小
    speed_bytes_per_sec: number; // 下载速度
    eta: string;               // 剩余时间 "HH:MM:SS" 或 "unknown"
    percent: number | null;    // 百分比
    status: string;            // "running" | "completed" | "cancelled" | "error"
    message: string | null;
}

export interface UpdaterState {
    checking: boolean;
    error: string | null;
    currentVersion: string | null;
    latestStable: ReleaseData | null;
    latestPrerelease: ReleaseData | null;
    selectedChannel: string | null;
    selectedRelease: ReleaseData | null;
    chosenRelease: ReleaseData | null;
    downloading: boolean;
    updateAvailable: boolean;
    progressSnapshot: TaskSnapshot | null;
    taskId: string | null;
}