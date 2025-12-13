// File: /src/pages/Manage/versionManage/components/UserSwitcher.jsx
import React from 'react';
import { Select } from '../../../../components/index.js';

export default function UserSwitcher(props) {
    const { normalizedKind, users, activeUserId, setActiveUserId, setConfirmDeleteOpen, setConfirmDeleteUserId, t } = props;
    if (normalizedKind === 'uwp') return null;
    const options = users.map(u => ({ value: u.id, label: u.name }));
    return (
        <div className="vmp-user-switcher">
            <Select
                value={activeUserId}
                onChange={(val) => setActiveUserId(val)}
                options={options}
                placeholder={t('common.select_user') || '选择用户'}
                size="md"
                className="vmp-user-select"
                dropdownMatchButton={true}
            />
            <div className="vmp-user-actions">
                {activeUserId && users.find(u => u.id === activeUserId) && (
                    <button className="btn btn-danger" onClick={() => { setConfirmDeleteUserId(activeUserId); setConfirmDeleteOpen(true); }}>删除用户</button>
                )}
            </div>
        </div>
    );
}
