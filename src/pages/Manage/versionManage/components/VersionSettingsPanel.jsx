// File: /src/pages/Manage/versionManage/components/VersionSettingsPanel.jsx
import React from 'react';
import Input from '../../../../components/Input.jsx';
import { Switch } from '../../../../components/index.js';

export default function VersionSettingsPanel({ versionSettings = {}, setVersionSettings = () => {}, createDesktopShortcut = () => {}, onClose = () => {}, normalizedKind }) {
    const isGDK = normalizedKind === 'gdk';
    return (
        <div className="vmp-panel vmp-panel-settings">
            <div className="vmp-form-row">
                <label>实例名称</label>
                <Input value={versionSettings.instanceName} onChange={(e) => setVersionSettings(s => ({ ...s, instanceName: e.target.value }))} inputClassName="vmp-input" fullWidth placeholder="实例名称" />
            </div>

            <div className="vmp-form-row">
                <label>图标路径</label>
                <div className="vmp-input-with-btn">
                    <Input value={versionSettings.iconPath} onChange={(e) => setVersionSettings(s => ({ ...s, iconPath: e.target.value }))} inputClassName="vmp-input" fullWidth placeholder="图标路径" />
                    <button className="btn">选择图标</button>
                </div>
            </div>

            {isGDK && (
                <>
                    <div className="vmp-form-row">
                        <label>桌面快捷方式</label>
                        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                            <button className="btn" onClick={createDesktopShortcut}>创建桌面快捷方式</button>
                            {versionSettings.desktopShortcutCreated && <span style={{ color: 'var(--accent-2)', fontWeight: 600 }}>已创建</span>}
                        </div>
                    </div>

                    <div className="vmp-form-row vmp-checkbox-row">
                        <label style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                            <span>版本隔离</span>
                            <Switch id="versionIsolation" checked={versionSettings.versionIsolation} onChange={(e) => setVersionSettings(s => ({ ...s, versionIsolation: !!e.target.checked }))} />
                        </label>
                    </div>

                    <div className="vmp-form-row vmp-checkbox-row">
                        <label style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                            <span>启用控制台</span>
                            <Switch id="enableConsole" checked={versionSettings.enableConsole} onChange={(e) => setVersionSettings(s => ({ ...s, enableConsole: !!e.target.checked }))} />
                        </label>
                    </div>
                </>
            )}

            <div className="vmp-form-row vmp-checkbox-row">
                <label style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                    <span>启用编辑模式</span>
                    <Switch id="enableEditMode" checked={versionSettings.enableEditMode} onChange={(e) => setVersionSettings(s => ({ ...s, enableEditMode: !!e.target.checked }))} />
                </label>
            </div>

            <div className="vmp-form-actions">
                <button className="btn btn-ghost" onClick={() => onClose && onClose()}>关闭</button>
            </div>
        </div>
    );
}