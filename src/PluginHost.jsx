import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export default function PluginHost({ children }) {
    const [plugins, setPlugins] = useState([]);
    const containerRefs = useRef({}); // 记录每个插件的真实 DOM 节点

    useEffect(() => {
        (async () => {
            console.log("[PluginHost] 开始加载插件列表...");
            let manifests;
            try {
                manifests = await invoke("get_plugins_list");
                console.log("[PluginHost] 插件清单：", manifests);
            } catch (e) {
                console.error("[PluginHost] 获取插件清单失败：", e);
                return;
            }
            const loaded = [];
            for (const manifest of manifests) {
                console.log(`[PluginHost] 尝试加载插件: ${manifest.name}, 入口: ${manifest.entry}`);
                try {
                    const code = await invoke("load_plugin_script", {
                        pluginName: manifest.name,
                        entryPath: manifest.entry,
                    });
                    console.log(`[PluginHost] 插件 ${manifest.name} 脚本加载成功，代码长度: ${code.length}`);
                    const blob = new Blob([code], { type: "text/javascript" });
                    const url = URL.createObjectURL(blob);
                    let mod;
                    try {
                        mod = await import(/* @vite-ignore */ url);
                        console.log(`[PluginHost] 插件 ${manifest.name} import 成功`);
                    } catch (importErr) {
                        console.error(`[PluginHost] 插件 ${manifest.name} import 失败:`, importErr);
                        continue;
                    } finally {
                        URL.revokeObjectURL(url);
                    }
                    loaded.push({ manifest, pluginFunc: mod.default });
                } catch (e) {
                    console.error(`[PluginHost] 插件 ${manifest.name} 加载失败:`, e);
                }
            }
            console.log("[PluginHost] 所有插件加载完毕:", loaded.map(p => p.manifest.name));
            setPlugins(loaded);
        })();
    }, []);

    useEffect(() => {
        plugins.forEach(({ manifest, pluginFunc }) => {
            const node = containerRefs.current[manifest.name];
            if (typeof pluginFunc === "function" && node) {
                console.log(`[PluginHost] 挂载插件: ${manifest.name}`);
                node.innerHTML = ""; // 清空旧内容
                try {
                    pluginFunc(node, invoke);   // **这里 node 是真实 DOM 节点**
                    console.log(`[PluginHost] 插件 ${manifest.name} 挂载成功`);
                } catch (e) {
                    console.error(`[PluginHost] 插件 ${manifest.name} 执行挂载失败:`, e);
                }
            } else {
                console.warn(`[PluginHost] 插件 ${manifest.name} 未能挂载：pluginFunc 或节点不存在`);
            }
        });
    }, [plugins]);

    return (
        <>
            {/* 插件挂载区 */}
            {plugins.map(({ manifest }) => (
                <div
                    key={manifest.name}
                    ref={el => {
                        if (el) containerRefs.current[manifest.name] = el;
                    }}
                />
            ))}
            {/* 渲染子组件（即App等） */}
            {children}
        </>
    );
}