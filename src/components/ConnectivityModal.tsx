import React, { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { X, Check, AlertCircle, Loader2, Globe, RotateCw } from 'lucide-react'; // [新增] 引入 RotateCw
import { useTranslation } from 'react-i18next';
import './ConnectivityModal.css';

// ... (ServiceItem, ServiceGroup 接口保持不变) ...
interface ServiceItem {
    name: string;
    url: string;
    desc?: string;
}

interface ServiceGroup {
    title: string;
    items: ServiceItem[];
}

// ... (SERVICE_GROUPS 数据保持不变) ...
const SERVICE_GROUPS: ServiceGroup[] = [
    {
        title: "Launcher Services",
        items: [
            { name: 'MCAPPX API', url: 'https://api.chlna6666.com/' },
            { name: 'Update Proxy', url: 'https://dl-proxy.bmcbl.com/' },
            { name: 'Update Check', url: 'https://updater.bmcbl.com/' },
        ]
    },
    {
        title: "Microsoft / Xbox Services",
        items: [
            { name: 'Xbox Live Auth', url: 'https://user.auth.xboxlive.com' },
            { name: 'Xbox XSTS Auth', url: 'https://xsts.auth.xboxlive.com' },
            { name: 'GDK Download', url: 'http://assets1.xboxlive.cn/' },
            { name: 'UWP Download', url: 'http://tlu.dl.delivery.mp.microsoft.com/' },
            { name: 'UWP URL Parse', url: 'https://fe3.delivery.mp.microsoft.com/' },
        ]
    },
    {
        title: "Community & Resources",
        items: [
            { name: 'CurseForge', url: 'https://www.curseforge.com/minecraft-bedrock/' },
            { name: 'GitHub', url: 'https://github.com' },
        ]
    }
];

// ... (Props 和 Result 接口保持不变) ...
interface ConnectivityModalProps {
    isOpen: boolean;
    onClose: () => void;
}

interface ServiceResult {
    status: 'pending' | 'loading' | 'success' | 'error';
    latency: number;
    error?: string;
}

export default function ConnectivityModal({ isOpen, onClose }: ConnectivityModalProps) {
    const { t } = useTranslation();
    const [results, setResults] = useState<Record<string, ServiceResult>>({});
    const [isRunning, setIsRunning] = useState(false);

    useEffect(() => {
        if (isOpen) {
            runTests();
        } else {
            setResults({});
            setIsRunning(false);
        }
    }, [isOpen]);

    const runTests = async () => {
        if (isRunning) return;
        setIsRunning(true);

        const initialResults: Record<string, ServiceResult> = {};
        SERVICE_GROUPS.forEach(group => {
            group.items.forEach(item => {
                initialResults[item.name] = { status: 'pending', latency: 0 };
            });
        });
        setResults(initialResults);

        for (const group of SERVICE_GROUPS) {
            for (const service of group.items) {
                setResults(prev => ({
                    ...prev,
                    [service.name]: { ...prev[service.name], status: 'loading' }
                }));

                try {
                    const latency = await invoke<number>('test_network_connectivity', { url: service.url });
                    setResults(prev => ({
                        ...prev,
                        [service.name]: { status: 'success', latency }
                    }));
                } catch (err: any) {
                    setResults(prev => ({
                        ...prev,
                        [service.name]: { status: 'error', latency: 0, error: String(err) }
                    }));
                }
            }
        }
        setIsRunning(false);
    };

    const getLatencyClass = (latency: number, status: string) => {
        if (status === 'error') return 'error';
        if (latency < 200) return 'fast';
        if (latency < 600) return 'medium';
        return 'slow';
    };

    if (!isOpen) return null;

    return createPortal(
        <div className="connectivity-overlay cm-anim-backdrop" onClick={onClose}>
            <div
                className="connectivity-card cm-anim-modal"
                onClick={(e) => e.stopPropagation()}
            >
                        <div className="connectivity-header">
                            <h3>
                                {isRunning ? <Loader2 className="animate-spin" size={18} /> : <Globe size={18} />}
                                {t("Connectivity.title") || "Network Services Status"}
                            </h3>

                            {/* [修改] 头部操作区：包含刷新和关闭按钮 */}
                            <div className="header-actions">
                                <button
                                    className="header-btn refresh-btn"
                                    onClick={runTests}
                                    disabled={isRunning}
                                    title={t("common.refresh") || "Refresh"}
                                >
                                    {/* 如果正在运行，给刷新图标也加上旋转动画 */}
                                    <RotateCw size={18} className={isRunning ? 'animate-spin' : ''} />
                                </button>
                                <button className="header-btn close-btn" onClick={onClose}>
                                    <X size={20} />
                                </button>
                            </div>
                        </div>

                        <div className="connectivity-list cm-anim-content">
                            {SERVICE_GROUPS.map((group, groupIndex) => (
                                <div
                                    key={group.title}
                                    className="group-block"
                                >
                                    <div className="group-title">
                                        {t(`Connectivity.groups.${groupIndex}`) || group.title}
                                    </div>

                                    {group.items.map((service) => {
                                        const res = results[service.name] || { status: 'pending', latency: 0 };
                                        const badgeClass = getLatencyClass(res.latency, res.status);

                                        return (
                                            <div key={service.name} className="service-item">
                                                <div className="service-info">
                                                    <span className="service-name">{service.name}</span>
                                                    <span className="service-url">{service.url}</span>
                                                </div>

                                                <div className="service-status">
                                                    {res.status === 'pending' && (
                                                        <span className="status-badge pending">•••</span>
                                                    )}
                                                    {res.status === 'loading' && (
                                                        <span className="status-badge loading">
                                                            <Loader2 className="animate-spin" size={14} />
                                                        </span>
                                                    )}
                                                    {res.status === 'success' && (
                                                        <div className={`status-badge ${badgeClass}`}>
                                                            <Check size={14} strokeWidth={3} />
                                                            <span>{res.latency} ms</span>
                                                        </div>
                                                    )}
                                                    {res.status === 'error' && (
                                                        <div className="status-badge error" title={res.error}>
                                                            <AlertCircle size={14} />
                                                            <span>Error</span>
                                                        </div>
                                                    )}
                                                </div>
                                            </div>
                                        );
                                    })}
                                </div>
                            ))}
                        </div>
            </div>
        </div>,
        document.body
    );
}
