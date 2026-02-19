import React, { useState, useEffect } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { X, Save, Loader2, AlertTriangle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useToast } from '../../../components/Toast';
import { Input, Select } from '../../../components';
import './VersionSettingsModal.css';

interface VersionSettingsModalProps {
    isOpen: boolean;
    onClose: () => void;
    version: any; // 版本对象
    onSaved?: () => void; // [新增] 保存成功后的回调
}

// 辅助函数：版本比较 (v1 >= v2 返回 true)
const isVersionAtLeast = (v1: string, v2: string) => {
    if (!v1) return false;
    const p1 = v1.split('.').map(Number);
    const p2 = v2.split('.').map(Number);
    const len = Math.max(p1.length, p2.length);
    for (let i = 0; i < len; i++) {
        const n1 = p1[i] || 0;
        const n2 = p2[i] || 0;
        if (n1 > n2) return true;
        if (n1 < n2) return false;
    }
    return true;
};

export const VersionSettingsModal: React.FC<VersionSettingsModalProps> = ({ isOpen, onClose, version, onSaved }) => {
    const [config, setConfig] = useState({
        enable_debug_console: false,
        enable_redirection: false,
        editor_mode: false,
        disable_mod_loading: false,
        lock_mouse_on_launch: false,
        unlock_mouse_hotkey: 'ALT',
        reduce_pixels: 20,
    });
    const [loading, setLoading] = useState(true);
    const [saving, setSaving] = useState(false);
    const toast = useToast();
    const { t } = useTranslation();

    // 1.19.80.20 是编辑器模式的最低版本要求
    const canUseEditor = version?.version && isVersionAtLeast(version.version, "1.19.80.20");
    const buildType = String(version?.kind || 'uwp').toLowerCase();
    const isGdk = buildType === 'gdk';

    const HOTKEY_OPTIONS: Array<{ value: string; label: string }> = [
        { value: 'ALT', label: 'ALT' },
        { value: 'CTRL', label: 'CTRL' },
        { value: 'SHIFT', label: 'SHIFT' },
        { value: 'LWIN', label: 'LWIN' },
        { value: 'RWIN', label: 'RWIN' },
    ];

    useEffect(() => {
        if (isOpen && version) {
            setLoading(true);
            invoke('get_version_config', { folderName: version.folder })
                .then((res: any) => {
                    // 确保拿到的是对象
                    const next = res || {
                        enable_debug_console: false,
                        enable_redirection: false,
                        editor_mode: false,
                        disable_mod_loading: false,
                        lock_mouse_on_launch: false,
                        unlock_mouse_hotkey: 'ALT',
                        reduce_pixels: 20,
                    };

                    // GDK: mouse lock is not needed (official fix), force disabled.
                    if (isGdk) {
                        next.lock_mouse_on_launch = false;
                    }
                    setConfig(next);
                })
                .catch((err) => {
                    toast.error(t("VersionSettingsModal.load_failed", { message: String(err) }));
                })
                .finally(() => setLoading(false));
        }
    }, [isOpen, version, t, isGdk]);

    const handleSave = async () => {
        setSaving(true);
        try {
            const payload = {
                ...config,
                enable_redirection: config.enable_redirection,
                editor_mode: canUseEditor ? config.editor_mode : false,
                lock_mouse_on_launch: isGdk ? false : config.lock_mouse_on_launch,
                reduce_pixels: Number.isFinite(Number(config.reduce_pixels)) ? Number(config.reduce_pixels) : 0,
                unlock_mouse_hotkey: String(config.unlock_mouse_hotkey || 'ALT'),
            };
            await invoke('save_version_config', { folderName: version.folder, config: payload });
            toast.success(t("VersionSettingsModal.save_success"));

            // [新增] 触发刷新回调
            if (onSaved) {
                onSaved();
            }
            onClose();
        } catch (err: any) {
            toast.error(t("VersionSettingsModal.save_failed", { message: String(err) }));
        } finally {
            setSaving(false);
        }
    };

    const toggle = (key: keyof typeof config) => {
        setConfig(prev => ({ ...prev, [key]: !prev[key] }));
    };

    const setField = (key: keyof typeof config, value: any) => {
        setConfig(prev => ({ ...prev, [key]: value }));
    };

    if (!isOpen) return null;

    return createPortal(
        <div className="vs-modal-overlay" onClick={onClose}>
            <div className="vs-modal" onClick={e => e.stopPropagation()}>
                <div className="vs-header">
                    <h3 className="vs-title">{t("VersionSettingsModal.title")}</h3>
                    <button className="icon-btn-ghost" onClick={onClose}><X size={20} /></button>
                </div>

                <div className="vs-body">
                    {loading ? (
                        <div style={{ display: 'flex', justifyContent: 'center', padding: 20 }}>
                            <Loader2 className="loader-spin" size={24} />
                        </div>
                    ) : (
                        <>
                            {/* 1. 调试控制台 */}
                            <div className="vs-option-item">
                                <div className="vs-option-info">
                                    <span className="vs-option-label">{t("VersionSettingsModal.debug_console_label")}</span>
                                    <span className="vs-option-desc">{t("VersionSettingsModal.debug_console_desc")}</span>
                                </div>
                                <div className={`vs-switch ${config.enable_debug_console ? 'checked' : ''}`} onClick={() => toggle('enable_debug_console')}>
                                    <div className="vs-switch-thumb" />
                                </div>
                            </div>

                            {/* 2. 目录重定向 */}
                            <div className="vs-option-item">
                                <div className="vs-option-info">
                                    <span className="vs-option-label">{t("VersionSettingsModal.redirection_label")}</span>
                                    <span className="vs-option-desc">
                                        {t("VersionSettingsModal.redirection_desc")}
                                    </span>
                                </div>
                                <div className={`vs-switch ${config.enable_redirection ? 'checked' : ''}`} onClick={() => toggle('enable_redirection')}>
                                    <div className="vs-switch-thumb" />
                                </div>
                            </div>

                            {/* 3. 鼠标锁定 (GDK 隐藏) */}
                            {!isGdk && (
                                <div className={`vs-option-item vs-mouse-lock-card ${config.lock_mouse_on_launch ? 'expanded' : ''}`}>
                                    <div className="vs-mouse-lock-header">
                                        <div className="vs-option-info">
                                            <span className="vs-option-label">{t("VersionSettingsModal.mouse_lock_label")}</span>
                                            <span className="vs-option-desc">{t("VersionSettingsModal.mouse_lock_desc")}</span>
                                        </div>
                                        <div className={`vs-switch ${config.lock_mouse_on_launch ? 'checked' : ''}`} onClick={() => toggle('lock_mouse_on_launch')}>
                                            <div className="vs-switch-thumb" />
                                        </div>
                                    </div>

                                    {config.lock_mouse_on_launch && (
                                        <div className="vs-mouse-lock-body">
                                            <div className="vs-subrow">
                                                <div className="vs-option-info">
                                                    <span className="vs-option-label">{t("VersionSettingsModal.mouse_lock_reduce_label")}</span>
                                                    <span className="vs-option-desc">{t("VersionSettingsModal.mouse_lock_reduce_desc")}</span>
                                                </div>
                                                <div className="vs-control vs-control-narrow">
                                                    <Input
                                                        type="number"
                                                        value={config.reduce_pixels as any}
                                                        min={0}
                                                        onChange={(e: any) => setField('reduce_pixels', parseInt(e.target.value, 10) || 0)}
                                                        fullWidth
                                                        inputStyle={{ textAlign: 'right' }}
                                                    />
                                                </div>
                                            </div>

                                            <div className="vs-subrow">
                                                <div className="vs-option-info">
                                                    <span className="vs-option-label">{t("VersionSettingsModal.mouse_lock_hotkey_label")}</span>
                                                    <span className="vs-option-desc">{t("VersionSettingsModal.mouse_lock_hotkey_desc")}</span>
                                                </div>
                                                <div className="vs-control">
                                                    <Select
                                                        value={config.unlock_mouse_hotkey as any}
                                                        onChange={(val: any) => setField('unlock_mouse_hotkey', val)}
                                                        options={HOTKEY_OPTIONS}
                                                        size={13}
                                                        dropdownMatchButton={false}
                                                        maxHeight={180}
                                                    />
                                                </div>
                                            </div>

                                            <div className="vs-subhint">
                                                {t("VersionSettingsModal.mouse_lock_hotkey_tip")}
                                            </div>
                                        </div>
                                    )}
                                </div>
                            )}

                            {/* 4. 编辑器模式 (低版本隐藏) */}
                            {canUseEditor ? (
                                <div className="vs-option-item">
                                    <div className="vs-option-info">
                                        <span className="vs-option-label">{t("VersionSettingsModal.editor_label")}</span>
                                        <span className="vs-option-desc">{t("VersionSettingsModal.editor_desc")}</span>
                                    </div>
                                    <div className={`vs-switch ${config.editor_mode ? 'checked' : ''}`} onClick={() => toggle('editor_mode')}>
                                        <div className="vs-switch-thumb" />
                                    </div>
                                </div>
                            ) : null}

                            {/* 5. Mod 加载开关 (BLoader.dll 管理) */}
                            <div className="vs-option-item">
                                <div className="vs-option-info">
                                    <span className="vs-option-label">{t("VersionSettingsModal.disable_mod_loading_label")}</span>
                                    <span className="vs-option-desc">{t("VersionSettingsModal.disable_mod_loading_desc")}</span>
                                </div>
                                <div className={`vs-switch ${config.disable_mod_loading ? 'checked' : ''}`} onClick={() => toggle('disable_mod_loading')}>
                                    <div className="vs-switch-thumb" />
                                </div>
                            </div>
                        </>
                    )}
                </div>

                <div className="vs-footer">
                    <button className="btn-modal-cancel" onClick={onClose}>{t("common.cancel")}</button>
                    <button className="btn-modal-primary" onClick={handleSave} disabled={loading || saving}>
                        {saving ? t("common.saving") : t("VersionSettingsModal.save_changes")}
                    </button>
                </div>
            </div>
        </div>,
        document.body
    );
};
