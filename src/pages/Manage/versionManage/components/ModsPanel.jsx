// File: /src/pages/Manage/versionManage/components/ModsPanel.jsx
import React, { useMemo } from 'react';
import Input from '../../../../components/Input.jsx';

export default function ModsPanel({ mods = [], searchTerms = {}, setSearchTerms, toggleModEnabled, setModDelay }) {
    const q = (searchTerms.mods || '').toLowerCase().trim();
    const filtered = useMemo(() => {
        if (!q) return mods;
        return mods.filter(m => (m.name || '').toLowerCase().includes(q) || String(m.id).includes(q));
    }, [mods, searchTerms.mods]);

    return (
        <div className="vmp-panel vmp-panel-mods">
            <div className="vmp-panel-ops">
                <Input className="vmp-search-wrapper" placeholder="搜索 Mods..." value={searchTerms.mods} onChange={(e) => setSearchTerms(s => ({ ...s, mods: e.target.value }))} />
                <div className="vmp-ops-right">
                    <button className="btn">导入 Mod(s)</button>
                    <button className="btn">打开 Mod 文件夹</button>
                </div>
            </div>

            <div className="vmp-list">
                {filtered.map(m => (
                    <div key={m.id} className="vmp-list-item">
                        <div className="vmp-list-left">
                            <div className="vmp-mod-name">{m.name}</div>
                            <div className="vmp-mod-id">ID: {m.id}</div>
                        </div>
                        <div className="vmp-list-right">
                            <label className="vmp-switch">
                                <input type="checkbox" checked={!!m.enabled} onChange={() => toggleModEnabled(m.id)} />
                                <span className="vmp-switch-label">启用</span>
                            </label>
                            <input type="number" min={0} className="vmp-input-number" value={m.delay ?? 0} onChange={(e) => setModDelay(m.id, e.target.value)} />
                        </div>
                    </div>
                ))}
                {filtered.length === 0 && <div className="vmp-empty">没有检测到 Mods</div>}
            </div>
        </div>
    );
}