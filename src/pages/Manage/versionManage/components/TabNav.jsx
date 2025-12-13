// File: /src/pages/Manage/versionManage/components/TabNav.jsx
import React from 'react';
export default function TabNav({ tabs = [], activeTab, onChange }) {
    return (
        <div className="vmp-tabnav" role="tablist">
            {tabs.map(tab => (
                <button
                    key={tab.key}
                    className={`vmp-tab ${tab.key === activeTab ? 'active' : ''}`}
                    onClick={() => onChange(tab.key)}
                    role="tab"
                    aria-selected={tab.key === activeTab}
                    title={tab.label}
                >
                    {tab.label}
                </button>
            ))}
        </div>
    );
}
