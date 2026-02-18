// src/components/LaunchStatusModal.tsx
import React, { useEffect, useRef } from "react";
import { Loader2, AlertCircle, RotateCcw, X, Copy, Search } from "lucide-react";
import { useTranslation } from "react-i18next";
import './LaunchStatusModal.css';

// ... 保持 LaunchError, Props 定义不变 ...
import { LaunchError } from "../hooks/useLauncher"; // 确保路径正确

interface LaunchStatusModalProps {
    isOpen: boolean;
    logs: string[];
    error: LaunchError | null;
    onClose: () => void;
    onRetry?: () => void;
}

// [新增] 简单的日志格式化组件
const FormattedLogLine: React.FC<{ line: string }> = ({ line }) => {
    // 匹配时间戳 [12:00:00]
    const timeMatch = line.match(/^(\[\d{2}:\d{2}:\d{2}\])(.*)/);

    if (timeMatch) {
        return (
            <span className="log-line">
                <span style={{ color: '#569cd6', opacity: 0.8 }}>{timeMatch[1]}</span>
                {formatContent(timeMatch[2])}
            </span>
        );
    }
    return <span className="log-line">{formatContent(line)}</span>;
};

// 简单的关键词高亮
const formatContent = (text: string) => {
    // 这里可以添加更多关键词逻辑
    if (text.toLowerCase().includes("error") || text.toLowerCase().includes("失败")) {
        return <span style={{ color: '#f44747' }}>{text}</span>;
    }
    if (text.toLowerCase().includes("success") || text.toLowerCase().includes("成功")) {
        return <span style={{ color: '#4ec9b0' }}>{text}</span>;
    }
    if (text.includes("minecraft://")) {
        return <span style={{ color: '#ce9178' }}>{text}</span>; // 类似字符串的颜色
    }
    return <span>{text}</span>;
}

export const LaunchStatusModal: React.FC<LaunchStatusModalProps> = ({
                                                                        isOpen, logs, error, onClose, onRetry
                                                                    }) => {
    const logsEndRef = useRef<HTMLDivElement>(null);
    const { t } = useTranslation();

    useEffect(() => {
        if (isOpen && logsEndRef.current) {
            logsEndRef.current.scrollIntoView({ behavior: "smooth" });
        }
    }, [logs, isOpen]);

    const copyErrorToClipboard = () => {
        if (error) navigator.clipboard.writeText(JSON.stringify(error, null, 2));
    };

    const searchErrorOnline = () => {
        if (error) {
            const query = `Minecraft Bedrock Error ${error.code}`;
            window.open(`https://www.bing.com/search?q=${encodeURIComponent(query)}`, '_blank');
        }
    };

    if (!isOpen) return null;

    return (
        <div className="launch-overlay-fixed" style={{ zIndex: 99999 }}>
            <div className="overlay-backdrop lsm-anim-backdrop" />
            <div className={`launch-modal-fixed glass lsm-anim-modal ${error ? 'error-mode' : ''}`}>
                        {error && (
                            <button className="close-icon-btn" onClick={onClose} data-bm-title={t("LaunchStatusModal.close")}>
                                <X size={18} />
                            </button>
                        )}

                        <div className="modal-header">
                            <div className={`status-icon-ring ${error ? 'error' : 'loading'}`}>
                                {error ? <AlertCircle size={28} /> : <Loader2 size={28} className="spin" />}
                            </div>
                            <div className="header-text">
                                <h3>{error ? t("LaunchStatusModal.launch_failed") : t("LaunchStatusModal.launching")}</h3>
                                <span className="sub-status">
                                    {error ? t("LaunchStatusModal.error_code", { code: error.code }) : t("LaunchStatusModal.loading_env")}
                                </span>
                            </div>
                        </div>

                        <div className="modal-body">
                            {error ? (
                                <div className="error-detail-box">
                                    <p>{error.message}</p>
                                </div>
                            ) : (
                                <div className="log-output-box custom-scrollbar">
                                    {logs.map((log, i) => (
                                        // 使用新的格式化组件
                                        <FormattedLogLine key={i} line={log} />
                                    ))}
                                    <div ref={logsEndRef} style={{ float: "left", clear: "both" }} />
                                </div>
                            )}
                        </div>

                        <div className="modal-footer">
                            {error ? (
                                <>
                                    <div className="footer-tools">
                                        <button onClick={copyErrorToClipboard} className="tool-link">
                                            <Copy size={14} /> {t("LaunchStatusModal.copy")}
                                        </button>
                                        <button onClick={searchErrorOnline} className="tool-link">
                                            <Search size={14} /> {t("LaunchStatusModal.help")}
                                        </button>
                                    </div>
                                    <button onClick={onRetry} className="action-btn primary">
                                        <RotateCcw size={16} /> {t("LaunchStatusModal.retry")}
                                    </button>
                                </>
                            ) : (
                                <button onClick={onClose} className="action-btn ghost">
                                    {t("LaunchStatusModal.hide")}
                                </button>
                            )}
                        </div>
            </div>
        </div>
    );
};
