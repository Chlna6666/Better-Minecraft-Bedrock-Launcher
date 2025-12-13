import React, { useEffect, useRef, useState, useCallback } from 'react';
import {convertFileSrc, invoke} from '@tauri-apps/api/core';
import htm from 'htm';
import {createRoot} from "react-dom/client";

const html = htm.bind(React.createElement);

// 简单事件总线（保持不变）
class EventBus {
    constructor() { this.handlers = {}; }
    on(evt, fn) { (this.handlers[evt] ||= []).push(fn); }
    off(evt, fn) { if (this.handlers[evt]) this.handlers[evt].splice(this.handlers[evt].indexOf(fn) >>> 0, 1); }
    emit(evt, data) { (this.handlers[evt] || []).forEach(fn => fn(data)); }
}
const pluginBus = new EventBus();

// Windows/Unix 路径分隔符
const joinPath = (root, relative) => {
    if (!root) return '';
    if (!relative) return root;

    // 1. 统一分隔符为 / (JS中处理路径通常转为/比较方便，convertFileSrc能识别)
    // 注意：Windows 绝对路径可能是 C:\xxx，保留盘符后的冒号
    let cleanRoot = root.replace(/\\/g, '/').replace(/\/$/, '');
    let cleanRelative = relative.replace(/\\/g, '/').replace(/^\.\//, '').replace(/^\//, '');

    return `${cleanRoot}/${cleanRelative}`;
};

export default function PluginHost({ children, autoReloadKey, concurrency = 4 }) {
    const createdUrlsRef = useRef(new Set());
    const [manifests, setManifests] = useState([]);

    const containerRefs = useRef({});                // name -> DOM node
    const cleanupRef = useRef({});                   // name -> cleanup fn
    const moduleCacheRef = useRef(new Map());        // cacheKey -> module
    const loadingTasksRef = useRef(new Map());       // name -> { cancelled, promise }
    const pluginAPIRef = useRef({});                 // name -> module
    const nodeReadyResolversRef = useRef({});        // name -> { promise, resolve, reject, timer }

    const pluginRootsRef = useRef({});

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

        // revoke any leftover object URLs (防御性回收)
        try {
            createdUrlsRef.current.forEach(u => {
                try { URL.revokeObjectURL(u); } catch (e) {}
            });
        } finally {
            createdUrlsRef.current.clear();
        }

        moduleCacheRef.current.clear();
    }, []);


    // waitForNode: 用 callback-ref + promise resolver (无轮询)
    const waitForNode = useCallback((name, timeout = 3000) => {
        const existing = containerRefs.current[name];
        if (existing) return Promise.resolve(existing);

        const existingResolver = nodeReadyResolversRef.current[name];
        if (existingResolver) return existingResolver.promise;

        let resolveFn, rejectFn;
        const p = new Promise((resolve, reject) => { resolveFn = resolve; rejectFn = reject; });

        const timer = setTimeout(() => {
            const r = nodeReadyResolversRef.current[name];
            if (r) {
                r.resolve(null);
                delete nodeReadyResolversRef.current[name];
            }
        }, timeout);

        nodeReadyResolversRef.current[name] = { promise: p, resolve: resolveFn, reject: rejectFn, timer };
        // 清理计时器在 promise 结束时
        p.finally(() => { const r = nodeReadyResolversRef.current[name]; if (r && r.timer) { clearTimeout(r.timer); } }).catch(()=>{});
        return p;
    }, []);

    // 更轻量 worker pool：直接消费 tasks 队列
    const workerPool = useCallback(async (tasks, workerCount) => {
        if (!tasks || tasks.length === 0) return;
        let i = 0;
        const results = new Array(tasks.length);
        const run = async () => {
            while (true) {
                const idx = i++;
                if (idx >= tasks.length) break;
                try {
                    results[idx] = await tasks[idx]();
                } catch (e) {
                    results[idx] = { error: e };
                }
            }
        };
        const workers = Array.from({ length: Math.max(1, Math.min(workerCount, tasks.length)) }, () => run());
        await Promise.all(workers);
        return results;
    }, []);

    // 挂载单个插件（不变逻辑，但更小心处理取消）
    const mountPlugin = useCallback(async (manifest, module) => {
        const name = manifest.name;
        const loadedStyleElements = [];
        const activeObservers = [];
        const modifiedElementsMap = new Map();

        // 🆕 新增：用于防抖和批量处理的 RAF 句柄
        let rafId = null;
        // 🆕 新增：待处理任务队列 (使用 Set 防止重复添加同一个元素)
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
            node.replaceChildren();
            await new Promise(res => setTimeout(res, 0));

            let reactRoot = null;

            // --- 🔧 核心工具函数 (性能优化版) ---
            const domUtils = {
                // 通用观察者 (增加了防抖警告，并未强制 RAF，但也限制了 filter)
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

                    // 默认配置，避免监听 subtree 的所有属性变化 (性能杀手)
                    const obsOptions = {
                        childList: true,
                        subtree: true,
                        attributes: !!options.attributes,
                        // 如果监听属性，强烈建议提供 filter，否则默认为空(不监听)以保护性能
                        attributeFilter: options.attributeFilter || (options.attributes ? [] : undefined)
                    };

                    const observer = new MutationObserver(handleMutations);
                    observer.observe(document.body, obsOptions);
                    activeObservers.push(observer);

                    document.querySelectorAll(selector).forEach(callback);
                },

                // 🚀 高性能图片替换 (RAF + Batching)
                replaceImage: (selector, newSrc) => {
                    // 1. 实际执行 DOM 修改的函数 (在下一帧执行)
                    const flushTasks = () => {
                        rafId = null;
                        if (pendingImageTasks.size === 0) return;

                        // 遍历待处理的图片集合
                        for (const img of pendingImageTasks) {
                            // 防御性检查：元素可能在等待期间被移除了
                            if (!document.body.contains(img)) continue;

                            // 再次检查是否需要替换 (防止 React 已经改回去了，或者其他插件改了)
                            if (img.src === newSrc) continue;

                            try {
                                // 备份逻辑
                                if (!modifiedElementsMap.has(img)) {
                                    modifiedElementsMap.set(img, {
                                        src: img.src,
                                        srcset: img.getAttribute('srcset')
                                    });
                                }

                                // 修改 DOM
                                img.src = newSrc;
                                img.removeAttribute('srcset');
                            } catch (e) {
                                console.warn(`[Plugin ${name}] Image replace failed:`, e);
                            }
                        }
                        pendingImageTasks.clear();
                    };

                    // 2. 将任务添加到队列
                    const scheduleTask = (img) => {
                        if (img.src === newSrc) return;

                        // 避免重复添加
                        pendingImageTasks.add(img);

                        // 如果还没有安排 RAF，就安排一个
                        if (!rafId) {
                            rafId = requestAnimationFrame(flushTasks);
                        }
                    };

                    // 3. 观察者回调 (只负责发现，不负责修改)
                    const handleMutations = (mutations) => {
                        for (const m of mutations) {
                            // 这是一个微小的优化：先判断 type 再循环，减少判断次数
                            if (m.type === 'childList') {
                                // 使用传统的 for 循环比 forEach 稍微快一点点 (在大量节点时)
                                for (let i = 0; i < m.addedNodes.length; i++) {
                                    const n = m.addedNodes[i];
                                    if (n.nodeType !== 1) continue; // 跳过非元素节点 (如文本)

                                    if (n.matches(selector)) scheduleTask(n);
                                    // 只有当该节点包含我们要找的元素时才查询 (性能优化)
                                    // 简单的启发式检查：如果它是容器，可能包含 img
                                    if (n.tagName === 'DIV' || n.tagName === 'HEADER' || n.tagName === 'NAV' || n.tagName === 'MAIN') {
                                        const found = n.querySelectorAll(selector);
                                        for (let j = 0; j < found.length; j++) scheduleTask(found[j]);
                                    }
                                }
                            } else if (m.type === 'attributes') {
                                // 属性变化 (React 重置了 src)
                                if (m.target.matches(selector) && m.target.src !== newSrc) {
                                    scheduleTask(m.target);
                                }
                            }
                        }
                    };

                    const observer = new MutationObserver(handleMutations);
                    // 仅监听 src 和 srcset，绝对不要监听 style 或 class
                    observer.observe(document.body, {
                        childList: true,
                        subtree: true,
                        attributes: true,
                        attributeFilter: ['src', 'srcset']
                    });
                    activeObservers.push(observer);

                    // 立即启动第一次检查
                    document.querySelectorAll(selector).forEach(scheduleTask);
                }
            };

            const context = {
                html,
                React,
                // 暴露 DOM 工具
                utils: domUtils,

                render: (component) => {
                    if (!reactRoot) {
                        reactRoot = createRoot(node);
                        pluginRootsRef.current[name] = reactRoot;
                    }
                    reactRoot.render(component);
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
                        const fullPath = joinPath(root, localPath);
                        return convertFileSrc(fullPath);
                    } catch (e) { return ''; }
                },
                loadStyle: (localPath) => {
                    try {
                        const root = manifest.root_path;
                        if (!root) return;
                        const fullPath = joinPath(root, localPath);
                        const assetUrl = convertFileSrc(fullPath);
                        const link = document.createElement('link');
                        link.rel = 'stylesheet';
                        link.href = assetUrl;
                        link.dataset.plugin = name;
                        document.head.appendChild(link);
                        loadedStyleElements.push(link);
                    } catch (e) {}
                },
            };

            const pluginFunc = module?.default;
            if (!pluginFunc || typeof pluginFunc !== 'function') throw new Error('插件未导出默认函数');

            const maybeCleanup = pluginFunc(node, context);
            const pluginCleanup = maybeCleanup instanceof Promise ? await maybeCleanup : maybeCleanup;

            // ✅ 修正了 Cleanup 逻辑：避免覆盖，统一管理
            cleanupRef.current[name] = () => {
                // 1. 插件自定义清理
                if (typeof pluginCleanup === 'function') { try { pluginCleanup(); } catch(e) {} }

                // 1. 取消任何挂起的 RAF 任务
                if (rafId) {
                    cancelAnimationFrame(rafId);
                    rafId = null;
                }
                pendingImageTasks.clear();

                // 2. 停止观察者
                activeObservers.forEach(obs => obs.disconnect());
                activeObservers.length = 0;

                // 3. 还原 DOM
                for (const [el, original] of modifiedElementsMap) {
                    if (document.contains(el)) {
                        if (original.src !== undefined) el.src = original.src;
                        if (original.srcset !== undefined && original.srcset !== null) {
                            el.setAttribute('srcset', original.srcset);
                        } else {
                            el.removeAttribute('srcset');
                        }
                    }
                }
                modifiedElementsMap.clear();

                // 4. 移除 CSS
                if (loadedStyleElements.length > 0) {
                    for (const el of loadedStyleElements) try { el.remove(); } catch (e) {}
                    loadedStyleElements.length = 0;
                }

                // 5. 卸载 React
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

    const loadModuleForManifest = useCallback(async (manifest, { useCache = true, cancelToken } = {}) => {
        const name = manifest.name;
        const cacheKey = `${manifest.name}::${manifest.entry || ''}`;

        if (useCache && moduleCacheRef.current.has(cacheKey)) {
            return moduleCacheRef.current.get(cacheKey);
        }

        let code;
        try {
            code = await invoke('load_plugin_script', { pluginName: manifest.name, entryPath: manifest.entry });
        } catch (e) {
            throw new Error(`加载插件脚本失败: ${e?.message ?? e}`);
        }

        if (cancelToken?.cancelled) throw new Error('cancelled');

        if (typeof code !== 'string') {
            throw new Error('插件脚本不是字符串');
        }

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

    // 主流程：加载并优先挂载 type 重要的插件
    const loadPlugins = useCallback(async (opts = { clearCache: false }) => {
        let manifestsList;
        try {
            manifestsList = await invoke('get_plugins_list');
            if (!Array.isArray(manifestsList)) {
                return Promise.reject(new Error('插件清单非数组'));
            }
        } catch (e) {
            console.error('获取插件清单失败：', e);
            return;
        }

        // cancel & cleanup existing
        cleanupAll();

        if (opts.clearCache) moduleCacheRef.current.clear();

        // 优先级排序（你可按需扩展）
        const priorityOrder = { dependency: 0, core: 1, ui: 2 };
        manifestsList.sort((a, b) => {
            const pa = priorityOrder[a.type] ?? 99;
            const pb = priorityOrder[b.type] ?? 99;
            if (pa !== pb) return pa - pb;
            return String(a.name).localeCompare(String(b.name));
        });

        // 设置容器占位（一次 setState）
        setManifests(manifestsList);

        // FAST PATH: 对已经缓存且 DOM 已就绪的模块，尽可能马上挂载
        for (const manifest of manifestsList) {
            const cacheKey = `${manifest.name}::${manifest.entry || ''}`;
            const mod = moduleCacheRef.current.get(cacheKey);
            const node = containerRefs.current[manifest.name];
            if (mod && node) {
                // create cancel token and immediately mount (microtask)
                const token = { cancelled: false };
                loadingTasksRef.current.set(manifest.name, token);
                // 使用 microtask 挂载，避免同步阻塞当前 loop
                Promise.resolve().then(() => {
                    // double-check cancellation
                    if (loadingTasksRef.current.get(manifest.name)?.cancelled) {
                        loadingTasksRef.current.delete(manifest.name);
                        return;
                    }
                    mountPlugin(manifest, mod);
                });
            }
        }

        // Build tasks for remaining plugins (skip those which already started above)
        const tasks = [];
        for (const manifest of manifestsList) {
            if (loadingTasksRef.current.has(manifest.name)) continue; // 已经在挂载或已安排
            tasks.push(async () => {
                const name = manifest.name;
                const token = { cancelled: false };
                loadingTasksRef.current.set(name, token);
                try {
                    const mod = await loadModuleForManifest(manifest, { useCache: true, cancelToken: token });
                    if (token.cancelled) {
                        return Promise.reject(new Error('cancelled'));
                    }
                    await mountPlugin(manifest, mod);
                } catch (e) {
                    if (String(e) !== 'Error: cancelled') {
                        console.error(`插件 ${name} 加载/挂载失败：`, e);
                    }
                    loadingTasksRef.current.delete(name);
                }
            });
        }


        const rebuiltHighTasks = [];
        const rebuiltOtherTasks = [];
        for (const manifest of manifestsList) {
            if (loadingTasksRef.current.has(manifest.name)) continue;
            const fn = async () => {
                const name = manifest.name;
                const token = { cancelled: false };
                loadingTasksRef.current.set(name, token);
                try {
                    const mod = await loadModuleForManifest(manifest, { useCache: true, cancelToken: token });
                    if (token.cancelled) {
                        return Promise.reject(new Error('cancelled'));
                    }
                    await mountPlugin(manifest, mod);
                } catch (e) {
                    if (String(e) !== 'Error: cancelled') {
                        console.error(`插件 ${name} 加载/挂载失败：`, e);
                    }
                    loadingTasksRef.current.delete(name);
                }
            };
            if (manifest.type === 'dependency' || manifest.type === 'core') rebuiltHighTasks.push(fn);
            else rebuiltOtherTasks.push(fn);
        }

        // run high priority with larger concurrency
        if (rebuiltHighTasks.length > 0) {
            await workerPool(rebuiltHighTasks, Math.max(2, concurrency));
        }
        if (rebuiltOtherTasks.length > 0) {
            await workerPool(rebuiltOtherTasks, Math.max(1, Math.floor(concurrency / 2)));
        }
    }, [cleanupAll, loadModuleForManifest, mountPlugin, workerPool, concurrency]);

    // 初始加载 & autoReloadKey 变化触发
    useEffect(() => {
        let mounted = true;
        (async () => {
            if (!mounted) return;
            await loadPlugins({ clearCache: true });
        })();
        return () => { mounted = false; };
    }, [loadPlugins, autoReloadKey]);

    // 组件卸载
    useEffect(() => {
        return () => {
            cleanupAll();
            setManifests([]);
            moduleCacheRef.current.clear();
        };
    }, [cleanupAll]);

    // 回调 ref：节点一到就 resolve 等待
    const setContainerRef = useCallback((name) => (el) => {
        if (el) {
            containerRefs.current[name] = el;
            const waiter = nodeReadyResolversRef.current[name];
            if (waiter) {
                try { if (waiter.timer) clearTimeout(waiter.timer); waiter.resolve(el); } catch (e) {}
                delete nodeReadyResolversRef.current[name];
            }
        } else {
            delete containerRefs.current[name];
            const waiter = nodeReadyResolversRef.current[name];
            if (waiter) {
                try { if (waiter.timer) clearTimeout(waiter.timer); waiter.resolve(null); } catch (e) {}
                delete nodeReadyResolversRef.current[name];
            }
        }
    }, []);

    return (
        <>
            {manifests.map((manifest) => (
                <div
                    key={manifest.name}
                    ref={setContainerRef(manifest.name)}
                />
            ))}
            {children}
        </>
    );
}
