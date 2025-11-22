import React, { useEffect, useRef, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';

// 简单事件总线（保持不变）
class EventBus {
    constructor() { this.handlers = {}; }
    on(evt, fn) { (this.handlers[evt] ||= []).push(fn); }
    off(evt, fn) { if (this.handlers[evt]) this.handlers[evt].splice(this.handlers[evt].indexOf(fn) >>> 0, 1); }
    emit(evt, data) { (this.handlers[evt] || []).forEach(fn => fn(data)); }
}
const pluginBus = new EventBus();

export default function PluginHost({ children, autoReloadKey, concurrency = 4 }) {
    const createdUrlsRef = useRef(new Set());
    const [manifests, setManifests] = useState([]);

    const containerRefs = useRef({});                // name -> DOM node
    const cleanupRef = useRef({});                   // name -> cleanup fn
    const moduleCacheRef = useRef(new Map());        // cacheKey -> module
    const loadingTasksRef = useRef(new Map());       // name -> { cancelled, promise }
    const pluginAPIRef = useRef({});                 // name -> module
    const nodeReadyResolversRef = useRef({});        // name -> { promise, resolve, reject, timer }

    const cleanupAll = useCallback(() => {
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
        const token = loadingTasksRef.current.get(name);
        if (token?.cancelled) return;

        const node = await waitForNode(name);
        if (!node) {
            loadingTasksRef.current.delete(name);
            return;
        }

        // 同步检查取消（避免在等待 node 期间被取消）
        if (loadingTasksRef.current.get(name)?.cancelled) {
            loadingTasksRef.current.delete(name);
            return;
        }

        try {
            node.replaceChildren();

            // 让浏览器先绘制（把插件的同步 DOM 操作推到下一个帧）
            await new Promise(res => {
                if (typeof requestAnimationFrame === 'function') requestAnimationFrame(res);
                else setTimeout(res, 0);
            });

            const context = {
                invoke,
                log: async (level, ...args) => {
                    const message = `[${manifest.name}] ` + args.map(a => String(a)).join(' ');
                    console[level]?.(message);
                    try { await invoke('log', { level, message }); } catch {}
                },
                on: (evt, handler) => pluginBus.on(`${name}:${evt}`, handler),
                off: (evt, handler) => pluginBus.off(`${name}:${evt}`, handler),
                emit: (evt, data) => pluginBus.emit(`${name}:${evt}`, data),
            };

            const pluginFunc = module?.default;
            if (!pluginFunc || typeof pluginFunc !== 'function') {
                return Promise.reject(new Error('插件未导出默认函数'));
            }

            const maybeCleanup = pluginFunc(node, context);
            const cleanup = maybeCleanup instanceof Promise ? await maybeCleanup : maybeCleanup;
            cleanupRef.current[name] = typeof cleanup === 'function' ? cleanup : () => {};
            pluginAPIRef.current[name] = module;
            // keep console message for debugging
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

        // 分组并发：先处理高优先级（dependency/core），再处理 others
        const high = manifestsList.filter(m => (m.type === 'dependency' || m.type === 'core'))
            .filter(m => !loadingTasksRef.current.has(m.name));
        const others = manifestsList.filter(m => !(m.type === 'dependency' || m.type === 'core'))
            .filter(m => !loadingTasksRef.current.has(m.name));

        const mapToTasks = (list) => {
            // 按 name 查找对应 tasks 索引，返回实际任务函数
            return list.map(m => {
                const idx = manifestsList.findIndex(x => x.name === m.name);
                // if moved to fast path, idx may be -1 or task may be unavailable; fallback to find by name in tasks list
                // simpler: find first task whose closure references this name:
                const found = tasks.find(t => {
                    // trick: cannot introspect; instead rely on order: tasks was created in manifestsList order excluding fast path.
                    return true; // we will just run workerPool on tasks array filtered below
                });
                return null;
            }).filter(Boolean);
        };

        // Simpler and efficient approach: run workerPool on tasks in two passes:
        // first pass: spawn workerPool with concurrency = max(concurrency, 2) but tasks filtered to high priority
        const highTasks = [];
        const otherTasks = [];
        // Split tasks based on manifestsList order mapping
        for (const taskWrapper of tasks) {
            // each taskWrapper closes over a manifest; extract name by temporarily running a proxy? that's messy.
            // Instead, rebuild tasks by iterating manifestsList directly (clean)
        }

        // Rebuild tasks cleanly by iterating manifestsList and skipping those already scheduled
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
