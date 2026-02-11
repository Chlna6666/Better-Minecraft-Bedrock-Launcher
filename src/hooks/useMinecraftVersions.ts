import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../utils/config";
import { useToast } from "../components/Toast.tsx";
import { useTranslation } from "react-i18next";

const CACHE_KEY = "appx_api_cache";
const CACHE_TTL = 1000 * 60 * 60 * 12; // 12小时

// 辅助：版本比较
export function compareVersion(v1: string, v2: string) {
    const parts1 = String(v1).split(".").map(Number);
    const parts2 = String(v2).split(".").map(Number);
    const len = Math.max(parts1.length, parts2.length);
    for (let i = 0; i < len; i++) {
        const num1 = parts1[i] || 0;
        const num2 = parts2[i] || 0;
        if (num1 < num2) return -1;
        if (num1 > num2) return 1;
    }
    return 0;
}

// 辅助：读取缓存 (提取到组件外，或者是纯函数)
const loadCacheFromStorage = () => {
    try {
        const raw = localStorage.getItem(CACHE_KEY);
        if (!raw) return null;
        const obj = JSON.parse(raw);
        // 检查过期
        if (Date.now() - obj.ts > (obj.ttl || CACHE_TTL)) {
            localStorage.removeItem(CACHE_KEY);
            return null;
        }
        return obj;
    } catch (e) {
        return null;
    }
};

export function useMinecraftVersions() {
    const { t } = useTranslation();
    // 1. 懒初始化 State：直接从缓存取值，避免 Mount 后的第一次 Re-render
    const [versions, setVersions] = useState<any[]>(() => {
        const cache = loadCacheFromStorage();
        return cache?.parsed || [];
    });

    // 2. 初始化 Ref，保持与 State 同步，用于逻辑判断
    const cachedVersions = useRef<any[]>(versions);

    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const fetchLockRef = useRef(false);
    const toast = useToast();

    // 3. 保存缓存
    const saveCache = useCallback((rawBody: string, parsed: any[], rawCreationTime: any) => {
        const obj = {
            ts: Date.now(),
            raw: rawBody,
            parsed,
            ttl: CACHE_TTL,
            rawCreationTime
        };
        localStorage.setItem(CACHE_KEY, JSON.stringify(obj));
    }, []);

    // 4. 核心获取逻辑 (使用 useCallback 稳定引用)
    const fetchVersions = useCallback(async (forceRefresh = false) => {
        // [关键修复] 如果正在请求，阻断
        if (fetchLockRef.current) return;

        // [关键修复] 如果不是强制刷新，且内存/Ref中已有数据，直接阻断
        // 这样进入页面时，因为 useState 已经懒加载了缓存，这里会直接 return
        // 从而避免了 "进入页面 -> Loading true -> Loading false" 的闪烁
        if (!forceRefresh && cachedVersions.current.length > 0) {
            return;
        }

        fetchLockRef.current = true;
        setLoading(true);
        setError(null);

        // 如果是强制刷新，或者是首次加载且无缓存，才尝试读取本地（如果是强制刷新，这步其实可以跳过，但为了 CreationTime 对比保留）
        const localCache = loadCacheFromStorage();

        try {
            const config = await getConfig().catch(() => ({}));
            const api = config?.launcher?.custom_appx_api || "https://api.chlna6666.com/mcappx";
            const defaultUA = config?.launcher?.user_agent || "BMCBL";

            let allowedHosts: string[] = [];
            try { const u = new URL(api); if (u.hostname) allowedHosts.push(u.hostname); } catch {}

            // 发起请求
            const backendRes = await invoke("fetch_remote", {
                url: api,
                options: {
                    method: "GET",
                    headers: { "User-Agent": defaultUA, "Accept": "application/json, text/*" },
                    timeout_ms: 20000,
                    allow_redirects: true,
                    allowed_hosts: allowedHosts.length ? allowedHosts : undefined,
                }
            }) as any;

            if (!backendRes?.body) throw new Error("Empty response");

            let data;
            try { data = JSON.parse(backendRes.body); } catch (e) { throw new Error("Invalid JSON"); }

            // 检查 CreationTime 决定是否使用缓存 (防止覆盖更新的本地缓存)
            const apiCreationTime = data?.CreationTime;
            if (!forceRefresh && localCache?.rawCreationTime && apiCreationTime) {
                const localTs = Date.parse(localCache.rawCreationTime);
                const apiTs = Date.parse(apiCreationTime);

                // 如果 API 数据比本地还旧或一样，停止处理，使用本地
                if (!isNaN(localTs) && !isNaN(apiTs) && apiTs <= localTs) {
                    if (localCache.parsed) {
                        setVersions(localCache.parsed);
                        cachedVersions.current = localCache.parsed;
                    }
                    setLoading(false);
                    fetchLockRef.current = false;
                    return;
                }
            }

            // --- 数据解析 (保持你的原逻辑) ---
            let src = data;
            if (data && typeof data === 'object') {
                const keys = Object.keys(data);
                for (const key of keys) {
                    if (data[key] && typeof data[key] === 'object' && !key.includes('Time')) {
                        src = data[key];
                        break;
                    }
                }
            }

            const parsed: any[] = [];
            Object.entries(src).forEach(([versionKey, item]: [string, any]) => {
                if (!item || typeof item !== "object") return;

                const typeStr = item.Type || "";
                const typeMap: Record<string, number> = { "Release": 0, "Beta": 1, "Preview": 2 };
                let typeNum = typeMap[typeStr] ?? 2;

                const chosenVar = item.Variations?.find((v: any) => String(v.Arch).toLowerCase() === "x64");
                if (!chosenVar) return;

                let packageId = chosenVar.MetaData?.[0] || chosenVar.MetaData || chosenVar.MD5 || item.ID || "";
                const buildType = item.BuildType || "";
                const archivalStatus = chosenVar.ArchivalStatus ?? item.ArchivalStatus ?? null;
                const metaPresent = !!(chosenVar.MetaData && (Array.isArray(chosenVar.MetaData) ? chosenVar.MetaData[0] : chosenVar.MetaData));
                const md5 = chosenVar.MD5 || null;

                parsed.push([versionKey, packageId, typeNum, typeStr, buildType, archivalStatus, metaPresent, md5]);
            });

            parsed.sort((a, b) => compareVersion(b[0], a[0]));
            // -------------------------------

            // 更新状态
            setVersions(parsed);
            cachedVersions.current = parsed;
            saveCache(backendRes.body, parsed, apiCreationTime);

            if (forceRefresh) {
                toast?.success(t("MinecraftVersions.list_refreshed"));
            }

        } catch (e: any) {
            console.error(e);
            // 失败回退逻辑：如果之前没数据，尝试用缓存（虽然初始化已经做了，这里是双重保险）
            if (versions.length === 0 && localCache?.parsed) {
                setVersions(localCache.parsed);
                cachedVersions.current = localCache.parsed;
                toast?.info(t("MinecraftVersions.fallback_cache"));
            } else if (forceRefresh) {
                // 只有手动刷新失败才弹窗报错，避免自动刷新失败打扰用户
                toast?.error(t("MinecraftVersions.refresh_failed", { message: e.message }));
                setError(e.message);
            }
        } finally {
            setLoading(false);
            fetchLockRef.current = false;
        }
    }, [saveCache, toast, versions.length, t]); // 依赖项

    // 5. Mount 时尝试获取（如果缓存为空或过期）
    useEffect(() => {
        fetchVersions(false);
    }, [fetchVersions]);

    return { versions, loading, error, reload: () => fetchVersions(true) };
}
