// File: /src/pages/Manage/versionManage/components/ConfirmDeleteModal.jsx
import React from 'react';

export default function ConfirmDeleteModal({ onCancel = () => {}, onDelete = () => {} }) {
    return (
        <div className="vmp-modal-overlay">
            <div className="vmp-modal" onClick={(e) => e.stopPropagation()}>
                <div className="vmp-modal-title">确认删除用户</div>
                <div className="vmp-modal-body">确定要从列表中删除该用户（仅前端移除）？该操作不可撤销。</div>
                <div className="vmp-modal-actions">
                    <button className="btn btn-ghost" onClick={onCancel}>取消</button>
                    <button className="btn btn-danger" onClick={onDelete}>删除</button>
                </div>
            </div>
        </div>
    );
}