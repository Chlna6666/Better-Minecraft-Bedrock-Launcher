import React, { useEffect, useRef, useState, useCallback, Component } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import htm from 'htm';
import { createRoot } from "react-dom/client";

const html = htm.bind(React.createElement);

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

    // 清理所有资源
    const cleanupAll = useCallback(() => {
        Object.values(pluginRootsRef.current).forEach(root => {
            try { root.unmount(); } catch(e){}
        });
        pluginRootsRef.current = {};

        for (const [, task] of loadingTasksRef.current) {
            task.cancelled = true;
        }
        loadingTasksRef.current.clear();

        Object.values(nodeReadyResolversRef.current).forEach(({ resolve, timer }) => {
            try { if (timer) clearTimeout(timer); resolve(null); } catch (e) {}
        });
        nodeReadyResolversRef.current = {};

        Object.values(cleanupRef.current).forEach(fn => {
            try { fn(); } catch (e) {}
        });
        cleanupRef.current = {};
        pluginAPIRef.current = {};

        try {
            createdUrlsRef.current.forEach(u => {
                try { URL.revokeObjectURL(u); } catch (e) {}
            });
        } finally {
            createdUrlsRef.current.clear();
        }
        moduleCacheRef.current.clear();
    }, []);

    const waitForNode = useCallback((name, timeout = 3000) => {
        const existing = containerRefs.current[name];
        if (existing) return Promise.resolve(existing);
        const existingResolver = nodeReadyResolversRef.current[name];
        if (existingResolver) return existingResolver.promise;
        let resolveFn, rejectFn;
        const p = new Promise((resolve, reject) => { resolveFn = resolve; rejectFn = reject; });
        const timer = setTimeout(() => {
            const r = nodeReadyResolversRef.current[name];
            if (r) { r.resolve(null); delete nodeReadyResolversRef.current[name]; }
        }, timeout);
        nodeReadyResolversRef.current[name] = { promise: p, resolve: resolveFn, reject: rejectFn, timer };
        p.finally(() => { const r = nodeReadyResolversRef.current[name]; if (r && r.timer) clearTimeout(r.timer); }).catch(()=>{});
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
        // 分别存储 Shadow DOM 样式和全局样式
        const loadedShadowStyles = [];
        const loadedGlobalStyles = [];
        const activeObservers = [];
        const modifiedElementsMap = new Map();
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
                pluginRootsRef.current[name].unmount();
                delete pluginRootsRef.current[name];
            }

            // 1. 初始化 Shadow DOM (用于隔离组件 UI)
            let shadowRoot = node.shadowRoot;
            if (!shadowRoot) {
                shadowRoot = node.attachShadow({ mode: 'open' });
            }
            shadowRoot.innerHTML = ''; // Reset

            await new Promise(res => setTimeout(res, 0));

            let reactRoot = null;

            // DOM 工具 (保持监听全局 DOM)
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
                    const obsOptions = {
                        childList: true, subtree: true, attributes: !!options.attributes,
                        attributeFilter: options.attributeFilter || (options.attributes ? [] : undefined)
                    };
                    const observer = new MutationObserver(handleMutations);
                    observer.observe(document.body, obsOptions);
                    activeObservers.push(observer);
                    document.querySelectorAll(selector).forEach(callback);
                },
                replaceImage: (selector, newSrc) => {
                    const flushTasks = () => {
                        rafId = null;
                        if (pendingImageTasks.size === 0) return;
                        for (const img of pendingImageTasks) {
                            if (!document.body.contains(img)) continue;
                            if (img.src === newSrc) continue;
                            try {
                                if (!modifiedElementsMap.has(img)) {
                                    modifiedElementsMap.set(img, { src: img.src, srcset: img.getAttribute('srcset') });
                                }
                                img.src = newSrc;
                                img.removeAttribute('srcset');
                            } catch (e) {}
                        }
                        pendingImageTasks.clear();
                    };
                    const scheduleTask = (img) => {
                        if (img.src === newSrc) return;
                        pendingImageTasks.add(img);
                        if (!rafId) rafId = requestAnimationFrame(flushTasks);
                    };
                    const handleMutations = (mutations) => {
                        for (const m of mutations) {
                            if (m.type === 'childList') {
                                for (let i = 0; i < m.addedNodes.length; i++) {
                                    const n = m.addedNodes[i];
                                    if (n.nodeType !== 1) continue;
                                    if (n.matches(selector)) scheduleTask(n);
                                    if (['DIV','HEADER','NAV','MAIN'].includes(n.tagName)) {
                                        const found = n.querySelectorAll(selector);
                                        for (let j = 0; j < found.length; j++) scheduleTask(found[j]);
                                    }
                                }
                            } else if (m.type === 'attributes') {
                                if (m.target.matches(selector) && m.target.src !== newSrc) scheduleTask(m.target);
                            }
                        }
                    };
                    const observer = new MutationObserver(handleMutations);
                    observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['src', 'srcset'] });
                    activeObservers.push(observer);
                    document.querySelectorAll(selector).forEach(scheduleTask);
                }
            };

            const context = {
                html,
                React,
                utils: domUtils,

                // 渲染到 Shadow DOM，但也允许通过 Portal 渲染到外面
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

                // 🌟🌟🌟 [修复重点] 支持全局样式注入 🌟🌟🌟
                // 用法：context.loadStyle('style.css', { global: true })
                loadStyle: (localPath, options = {}) => {
                    try {
                        // 兼容旧写法：loadStyle('path') -> 默认 Shadow
                        // 新写法：loadStyle('path', { global: true }) -> 注入到 Head
                        const isGlobal = options === true || options?.global === true;

                        const root = manifest.root_path;
                        if (!root) return;
                        const assetUrl = convertFileSrc(joinPath(root, localPath));
                        const link = document.createElement('link');
                        link.rel = 'stylesheet';
                        link.href = assetUrl;
                        link.dataset.plugin = name;

                        if (isGlobal) {
                            // 全局：影响整个页面 (Tampermonkey 模式)
                            document.head.appendChild(link);
                            loadedGlobalStyles.push(link);
                        } else {
                            // 默认：隔离 (Widget 模式)
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

            // 清理逻辑
            cleanupRef.current[name] = () => {
                if (typeof pluginCleanup === 'function') { try { pluginCleanup(); } catch(e) {} }

                if (rafId) { cancelAnimationFrame(rafId); rafId = null; }
                pendingImageTasks.clear();

                activeObservers.forEach(obs => obs.disconnect());
                activeObservers.length = 0;

                for (const [el, original] of modifiedElementsMap) {
                    if (document.contains(el)) {
                        if (original.src !== undefined) el.src = original.src;
                        if (original.srcset !== undefined && original.srcset !== null) el.setAttribute('srcset', original.srcset);
                        else el.removeAttribute('srcset');
                    }
                }
                modifiedElementsMap.clear();

                // 移除 Shadow DOM 中的样式
                loadedShadowStyles.forEach(el => { try { el.remove(); } catch (e) {} });
                loadedShadowStyles.length = 0;

                // 移除全局 Head 中的样式
                loadedGlobalStyles.forEach(el => { try { el.remove(); } catch (e) {} });
                loadedGlobalStyles.length = 0;

                if (pluginRootsRef.current[name]) {
                    try { pluginRootsRef.current[name].unmount(); } catch(e) {}
                    delete pluginRootsRef.current[name];
                }
            };
            pluginAPIRef.current[name] = module;
            console.log(`插件 ${name} 挂载成功`);

        } catch (e) {
            console.error(`插件 ${name} 挂载异常：`, e);
        } finally {
            loadingTasksRef.current.delete(name);
        }
    }, [waitForNode]);

    // ... (loadModuleForManifest, loadPlugins, useEffects 保持不变)
    // 为了节省篇幅，这里复用你之前的 loadModuleForManifest 和 loadPlugins 逻辑
    // 它们不需要修改

    // --- 为了完整性，这里补充 loadModuleForManifest 和 loadPlugins 的最小代码块 ---
    const loadModuleForManifest = useCallback(async (manifest, { useCache = true, cancelToken } = {}) => {
        const cacheKey = `${manifest.name}::${manifest.entry || ''}`;
        if (useCache && moduleCacheRef.current.has(cacheKey)) return moduleCacheRef.current.get(cacheKey);
        const code = await invoke('load_plugin_script', { pluginName: manifest.name, entryPath: manifest.entry });
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

        // 简化版调度逻辑，同之前
        const tasks = manifestsList.map(manifest => async () => {
            // ... 这里的逻辑和你之前的一样 ...
            const token = { cancelled: false };
            loadingTasksRef.current.set(manifest.name, token);
            try {
                const mod = await loadModuleForManifest(manifest, { useCache: true, cancelToken: token });
                if (!token.cancelled) await mountPlugin(manifest, mod);
            } catch(e) { loadingTasksRef.current.delete(manifest.name); }
        });
        await workerPool(tasks, concurrency);
    }, [cleanupAll, loadModuleForManifest, mountPlugin, workerPool, concurrency]);

    useEffect(() => {
        let mounted = true;
        (async () => { if (mounted) await loadPlugins({ clearCache: true }); })();
        return () => { mounted = false; };
    }, [loadPlugins, autoReloadKey]);

    useEffect(() => () => cleanupAll(), [cleanupAll]);

    const setContainerRef = useCallback((name) => (el) => {
        if (el) {
            containerRefs.current[name] = el;
            const waiter = nodeReadyResolversRef.current[name];
            if (waiter) { try { if(waiter.timer) clearTimeout(waiter.timer); waiter.resolve(el); } catch(e){} delete nodeReadyResolversRef.current[name]; }
        } else {
            delete containerRefs.current[name];
        }
    }, []);

    return (
        <>
            {manifests.map((manifest) => (
                <div key={manifest.name} ref={setContainerRef(manifest.name)} data-plugin-host={manifest.name} style={{ display: 'contents' }} />
            ))}
            {children}
        </>
    );
}