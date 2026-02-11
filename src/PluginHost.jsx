import React, { useEffect, useRef, useState, useCallback, Component, memo } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import htm from 'htm';
import { createRoot } from "react-dom/client";

const html = htm.bind(React.createElement);

// ... (PluginErrorBoundary 和 EventBus 保持不变) ...
// ----------------------------------------------------------------------
// 1. 错误边界组件
// ----------------------------------------------------------------------
class PluginErrorBoundary extends Component {
    constructor(props) {
        super(props);
        this.state = { hasError: false, error: null };
    }

    static getDerivedStateFromError(error) {
        return { hasError: true, error };
    }

    componentDidCatch(error, errorInfo) {
        console.error(`[Plugin ErrorBoundary] ${this.props.pluginName} crashed:`, error, errorInfo);
    }

    render() {
        if (this.state.hasError) {
            return React.createElement('div', {
                style: {
                    padding: '12px', border: '1px solid #ff4d4f',
                    backgroundColor: '#fff1f0', color: '#cf1322', fontSize: '12px'
                }
            }, `🔌 插件 "${this.props.pluginName}" 崩溃: ${this.state.error?.message}`);
        }
        return this.props.children;
    }
}

// ----------------------------------------------------------------------
// 2. 基础工具类
// ----------------------------------------------------------------------
class EventBus {
    constructor() { this.handlers = {}; }
    on(evt, fn) { (this.handlers[evt] ||= []).push(fn); }
    off(evt, fn) { if (this.handlers[evt]) this.handlers[evt].splice(this.handlers[evt].indexOf(fn) >>> 0, 1); }
    emit(evt, data) { (this.handlers[evt] || []).forEach(fn => fn(data)); }
}
const pluginBus = new EventBus();

const joinPath = (root, relative) => {
    if (!root) return '';
    if (!relative) return root;
    let cleanRoot = root.replace(/\\/g, '/').replace(/\/$/, '');
    let cleanRelative = relative.replace(/\\/g, '/').replace(/^\.\//, '').replace(/^\//, '');
    return `${cleanRoot}/${cleanRelative}`;
};

// ----------------------------------------------------------------------
// 新增：独立的插件容器组件 (解决 Ref 抖动问题)
// ----------------------------------------------------------------------
const PluginContainer = memo(({ name, onRef }) => {
    const elRef = useRef(null);

    useEffect(() => {
        // 组件挂载时注册 DOM
        if (elRef.current) {
            onRef(name, elRef.current);
        }
        // 组件卸载时注销 DOM
        return () => {
            onRef(name, null);
        };
    }, [name, onRef]);

    // 使用 display: contents 避免影响布局，但作为一个稳定的 React 节点存在
    return <div ref={elRef} data-plugin-host={name} style={{ display: 'contents' }} />;
});

// ----------------------------------------------------------------------
// 3. PluginHost 主组件
// ----------------------------------------------------------------------

export default function PluginHost({ children, autoReloadKey, concurrency = 4 }) {
    const [manifests, setManifests] = useState([]);

    const createdUrlsRef = useRef(new Set());
    const containerRefs = useRef({});
    const cleanupRef = useRef({});
    const moduleCacheRef = useRef(new Map());
    const loadingTasksRef = useRef(new Map());
    const pluginAPIRef = useRef({});
    const nodeReadyResolversRef = useRef({});
    const pluginRootsRef = useRef({});

    // 稳定的 Ref 处理回调
    const handlePluginRef = useCallback((name, el) => {
        if (el) {
            containerRefs.current[name] = el;
            const waiter = nodeReadyResolversRef.current[name];
            if (waiter) {
                try { if(waiter.timer) clearTimeout(waiter.timer); waiter.resolve(el); } catch(e){}
                delete nodeReadyResolversRef.current[name];
            }
        } else {
            delete containerRefs.current[name];
        }
    }, []);

    const safeUnmountRoot = useCallback((name) => {
        const root = pluginRootsRef.current[name];
        if (!root) return;
        delete pluginRootsRef.current[name];

        // 保持之前的修复：延迟卸载内部 Root
        setTimeout(() => {
            try { root.unmount(); } catch (e) {
                console.warn(`[PluginHost] Safe unmount warning for ${name}:`, e);
            }
        }, 0);
    }, []);

    const cleanupAll = useCallback(() => {
        Object.keys(pluginRootsRef.current).forEach(name => safeUnmountRoot(name));

        for (const [, task] of loadingTasksRef.current) task.cancelled = true;
        loadingTasksRef.current.clear();

        Object.values(nodeReadyResolversRef.current).forEach(({ resolve, timer }) => {
            try { if (timer) clearTimeout(timer); resolve(null); } catch (e) {}
        });
        nodeReadyResolversRef.current = {};

        // 立即执行清理函数，确保 Observers 和 Listeners 断开
        Object.values(cleanupRef.current).forEach(fn => {
            try { fn(); } catch (e) {}
        });
        cleanupRef.current = {};
        pluginAPIRef.current = {};

        try {
            createdUrlsRef.current.forEach(u => { try { URL.revokeObjectURL(u); } catch (e) {} });
        } finally {
            createdUrlsRef.current.clear();
        }
        moduleCacheRef.current.clear();
    }, [safeUnmountRoot]);

    const waitForNode = useCallback((name, timeout = 3000) => {
        const existing = containerRefs.current[name];
        if (existing) return Promise.resolve(existing);

        // 避免重复创建 Promise
        if (nodeReadyResolversRef.current[name]) return nodeReadyResolversRef.current[name].promise;

        let resolveFn, rejectFn;
        const p = new Promise((resolve, reject) => { resolveFn = resolve; rejectFn = reject; });
        const timer = setTimeout(() => {
            const r = nodeReadyResolversRef.current[name];
            if (r) { r.resolve(null); delete nodeReadyResolversRef.current[name]; }
        }, timeout);
        nodeReadyResolversRef.current[name] = { promise: p, resolve: resolveFn, reject: rejectFn, timer };
        return p;
    }, []);

    const workerPool = useCallback(async (tasks, workerCount) => {
        if (!tasks || tasks.length === 0) return;
        let i = 0;
        const workers = Array.from({ length: Math.max(1, Math.min(workerCount, tasks.length)) }, async () => {
            while (true) {
                const idx = i++;
                if (idx >= tasks.length) break;
                try { await tasks[idx](); } catch (e) { }
            }
        });
        await Promise.all(workers);
    }, []);

    // --- 核心挂载逻辑 ---
    const mountPlugin = useCallback(async (manifest, module) => {
        const name = manifest.name;
        // ... (变量初始化)
        const loadedShadowStyles = [];
        const loadedGlobalStyles = [];
        const activeObservers = [];
        let rafId = null;
        const pendingImageTasks = new Set();

        const token = loadingTasksRef.current.get(name);
        if (token?.cancelled) return;

        const node = await waitForNode(name);
        if (!node || loadingTasksRef.current.get(name)?.cancelled) {
            loadingTasksRef.current.delete(name);
            return;
        }

        try {
            if (pluginRootsRef.current[name]) {
                try { pluginRootsRef.current[name].unmount(); } catch (e) {}
                delete pluginRootsRef.current[name];
            }

            let shadowRoot = node.shadowRoot;
            if (!shadowRoot) {
                shadowRoot = node.attachShadow({ mode: 'open' });
            }
            // 清空前确保没有残留
            shadowRoot.innerHTML = '';

            // 这是一个微小的延迟，让浏览器有机会处理 DOM 状态
            await new Promise(res => setTimeout(res, 0));

            let reactRoot = null;

            const domUtils = {
                observeElement: (selector, callback, options = {}) => {
                    const handleMutations = (mutations) => {
                        for (const m of mutations) {
                            if (m.type === 'childList') {
                                m.addedNodes.forEach(n => {
                                    if (n instanceof Element && n.matches(selector)) callback(n);
                                    if (n instanceof Element && n.querySelectorAll) n.querySelectorAll(selector).forEach(callback);
                                });
                            } else if (m.type === 'attributes') {
                                if (m.target.matches(selector)) callback(m.target);
                            }
                        }
                    };
                    const observer = new MutationObserver(handleMutations);
                    // 注意：这里观察的是 document.body，如果插件未清理，这是崩溃的主要原因
                    observer.observe(document.body, { childList: true, subtree: true, attributes: !!options.attributes, attributeFilter: options.attributeFilter });
                    activeObservers.push(observer);
                    document.querySelectorAll(selector).forEach(callback);
                },
                replaceImage: (selector, newSrc) => {
                    document.querySelectorAll(selector).forEach(img => img.src = newSrc);
                }
            };

            const context = {
                html,
                React,
                utils: domUtils,
                listen: (event, handler) => listen(event, handler),
                render: (component) => {
                    if (!reactRoot) {
                        reactRoot = createRoot(shadowRoot);
                        pluginRootsRef.current[name] = reactRoot;
                    }
                    reactRoot.render(
                        React.createElement(PluginErrorBoundary, { pluginName: name }, component)
                    );
                },
                invoke,
                log: async (level, ...args) => {
                    const message = `[${manifest.name}] ` + args.map(a => String(a)).join(' ');
                    console[level]?.(message);
                    try { await invoke('log', { level, message }); } catch {}
                },
                on: (evt, handler) => pluginBus.on(`${name}:${evt}`, handler),
                off: (evt, handler) => pluginBus.off(`${name}:${evt}`, handler),
                emit: (evt, data) => pluginBus.emit(`${name}:${evt}`, data),
                getLocalResourceUrl: (localPath) => {
                    try {
                        const root = manifest.root_path;
                        if (!root) return '';
                        return convertFileSrc(joinPath(root, localPath));
                    } catch (e) { return ''; }
                },
                loadStyle: (localPath, options = {}) => {
                    try {
                        const isGlobal = options === true || options?.global === true;
                        const root = manifest.root_path;
                        if (!root) return;
                        const assetUrl = convertFileSrc(joinPath(root, localPath));
                        const link = document.createElement('link');
                        link.rel = 'stylesheet';
                        link.href = assetUrl;
                        link.dataset.plugin = name;

                        if (isGlobal) {
                            document.head.appendChild(link);
                            loadedGlobalStyles.push(link);
                        } else {
                            shadowRoot.appendChild(link);
                            loadedShadowStyles.push(link);
                        }
                    } catch (e) {
                        console.error(`[Plugin ${name}] loadStyle failed:`, e);
                    }
                },
            };

            const pluginFunc = module?.default;
            if (!pluginFunc || typeof pluginFunc !== 'function') throw new Error('插件未导出默认函数');

            const maybeCleanup = pluginFunc(node, context);
            const pluginCleanup = maybeCleanup instanceof Promise ? await maybeCleanup : maybeCleanup;

            cleanupRef.current[name] = () => {
                // 1. 优先断开观察者，防止 DOM 变动导致 React 找不到节点
                activeObservers.forEach(obs => obs.disconnect());
                activeObservers.length = 0;

                if (typeof pluginCleanup === 'function') { try { pluginCleanup(); } catch(e) {} }
                if (rafId) { cancelAnimationFrame(rafId); rafId = null; }
                pendingImageTasks.clear();

                modifiedElementsMap.clear();
                loadedShadowStyles.forEach(el => { try { el.remove(); } catch (e) {} });
                loadedGlobalStyles.forEach(el => { try { el.remove(); } catch (e) {} });

                safeUnmountRoot(name);
            };
            pluginAPIRef.current[name] = module;
            console.log(`插件 ${name} 挂载成功`);

        } catch (e) {
            console.error(`插件 ${name} 挂载异常：`, e);
        } finally {
            loadingTasksRef.current.delete(name);
        }
    }, [waitForNode, safeUnmountRoot]);

    // ... (loadModuleForManifest 和 loadPlugins 保持不变) ...
    const loadModuleForManifest = useCallback(async (manifest, { useCache = true, cancelToken } = {}) => {
        let entryPath = manifest.entry;
        if (manifest.type === 'native' || manifest.type === 'dll') {
            entryPath = manifest.ui_entry || 'index.js';
        }

        const cacheKey = `${manifest.name}::${entryPath}`;
        if (useCache && moduleCacheRef.current.has(cacheKey)) return moduleCacheRef.current.get(cacheKey);

        const code = await invoke('load_plugin_script', { pluginName: manifest.name, entryPath });
        if (cancelToken?.cancelled) throw new Error('cancelled');

        const blob = new Blob([code], { type: 'text/javascript' });
        const url = URL.createObjectURL(blob);
        createdUrlsRef.current.add(url);
        try {
            const mod = await import(/* @vite-ignore */ url);
            moduleCacheRef.current.set(cacheKey, mod);
            return mod;
        } finally {
            try { URL.revokeObjectURL(url); } catch (e) {}
            createdUrlsRef.current.delete(url);
        }
    }, []);

    const loadPlugins = useCallback(async (opts = { clearCache: false }) => {
        const manifestsList = await invoke('get_plugins_list').catch(e => []);
        if (!Array.isArray(manifestsList)) return;
        cleanupAll();
        if (opts.clearCache) moduleCacheRef.current.clear();
        setManifests(manifestsList);

        const tasks = manifestsList.map(manifest => async () => {
            if (loadingTasksRef.current.has(manifest.name)) return;
            const token = { cancelled: false };
            loadingTasksRef.current.set(manifest.name, token);
            try {
                const mod = await loadModuleForManifest(manifest, { useCache: true, cancelToken: token });
                if (!token.cancelled) await mountPlugin(manifest, mod);
            } catch(e) {
                console.error(`Failed to load ${manifest.name}:`, e);
                loadingTasksRef.current.delete(manifest.name);
            }
        });
        await workerPool(tasks, concurrency);
    }, [cleanupAll, loadModuleForManifest, mountPlugin, workerPool, concurrency]);

    useEffect(() => {
        let mounted = true;
        (async () => { if (mounted) await loadPlugins({ clearCache: true }); })();
        return () => { mounted = false; };
    }, [loadPlugins, autoReloadKey]);

    useEffect(() => () => cleanupAll(), [cleanupAll]);

    // 修复点：用 div 包裹 map，并使用 memo 组件
    return (
        <div style={{ display: 'contents' }}>
            {manifests.map((manifest) => (
                <PluginContainer
                    key={manifest.name}
                    name={manifest.name}
                    onRef={handlePluginRef}
                />
            ))}
            {children}
        </div>
    );
}