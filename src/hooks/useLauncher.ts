// src/hooks/useLauncher.ts
import { useState, useRef, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";

export interface LaunchError {
    code: string;
    message: string;
    raw?: string;
}

export const useLauncher = () => {
    const { t } = useTranslation();
    const [isLaunching, setIsLaunching] = useState(false);
    const [launchLogs, setLaunchLogs] = useState<string[]>([]); // 保留完整日志历史
    const [lastLog, setLastLog] = useState<string>("");         // 仅保留最新一行（用于 UI 显示）
    const [launchError, setLaunchError] = useState<LaunchError | null>(null);
    const unlistenRef = useRef<UnlistenFn | null>(null);

    // 格式化日志信息的帮助函数
    const formatLogPayload = (payload: any) => {
        try {
            const now = new Date().toLocaleTimeString();
            const stage = payload.stage ?? "unknown";
            const status = payload.status ?? "";
            const msg = payload.message ?? "";
            const code = payload.code ?? "";

            let line = `[${now}] [${stage}] ${status}`;
            if (msg) line += ` - ${msg}`;
            if (code) line += ` (${code})`;
            return line;
        } catch {
            return JSON.stringify(payload);
        }
    };

    // 停止监听并清理状态
    const stopListening = useCallback(async () => {
        if (unlistenRef.current) {
            try { await unlistenRef.current(); } catch (_) { /* ignore */ }
            unlistenRef.current = null;
        }
    }, []);

    // 启动监听
    const startListening = useCallback(async (onSuccess?: () => void) => {
        // 防止重复监听
        if (unlistenRef.current) return;

        try {
            const unlisten = await listen("launch-progress", (e: any) => {
                const payload = e.payload || {};
                const line = formatLogPayload(payload);

                setLastLog(line);
                setLaunchLogs(prev => [...prev, line]);

                // 错误处理
                if (payload.status === "error") {
                    setIsLaunching(false);
                    setLaunchError({
                        code: payload.code ? String(payload.code) : t("common.unknown"),
                        message: payload.message ? String(payload.message) : JSON.stringify(payload),
                        raw: JSON.stringify(payload)
                    });
                }

                // 完成处理
                if (payload.stage === "done" && payload.status === "ok") {
                    setIsLaunching(false);
                    // 延迟一小段时间以确保 UI 动画流畅
                    setTimeout(() => {
                        stopListening();
                        if (onSuccess) onSuccess();
                    }, 600);
                }
            });
            unlistenRef.current = unlisten;
        } catch (err) {
            console.error("Failed to setup launch listener:", err);
        }
    }, [stopListening]);

    /**
     * 启动核心方法
     */
    const launch = useCallback(async (folderName: string, args: string | null = null, onSuccess?: () => void) => {
        if (isLaunching) return;

        setIsLaunching(true);
        setLaunchError(null);
        setLaunchLogs([]);
        setLastLog(t("LaunchLog.preparing", { time: new Date().toLocaleTimeString(), folder: folderName }));

        await startListening(onSuccess);

        try {
            await invoke("launch_appx", {
                fileName: folderName,
                autoStart: true,
                launchArgs: args
            });
        } catch (err: any) {
            console.error("Invoke launch_appx failed:", err);

            // 统一的错误解析逻辑
            let code = "InvokeError";
            let message = String(err);

            if (typeof err === "object") {
                if (err.code) code = String(err.code);
                if (err.message) message = String(err.message);
            }

            // 尝试提取 HRESULT
            const hrMatch = message.match(/HRESULT\((0x[0-9A-Fa-f]+)\)/);
            if (hrMatch) code = hrMatch[1];

            setIsLaunching(false);
            setLaunchError({ code, message });
            setLastLog(t("LaunchLog.error", { time: new Date().toLocaleTimeString(), message }));
        }
    }, [isLaunching, startListening, t]);

    // 重置状态（用于关闭弹窗时）
    const reset = useCallback(async () => {
        await stopListening();
        setIsLaunching(false);
        setLaunchError(null);
        setLaunchLogs([]);
        setLastLog("");
    }, [stopListening]);

    // 组件卸载时自动清理
    useEffect(() => {
        return () => {
            if (unlistenRef.current) unlistenRef.current();
        };
    }, []);

    return {
        isLaunching,
        launchLogs,
        lastLog,      // 对应原代码的 launchDetails (string)
        launchError,
        launch,
        reset
    };
};
