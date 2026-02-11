// File: src/pages/Manage/components/ConfirmModal.tsx
import React from 'react';
import { useTranslation } from 'react-i18next';
import { createPortal } from 'react-dom';
import './ConfirmModal.css';

interface ConfirmModalProps {
    isOpen: boolean;
    title: string;
    content: React.ReactNode;
    onConfirm: () => void;
    onCancel: () => void;
    isLoading?: boolean;
    confirmText?: string;
    cancelText?: string;
    isDanger?: boolean;
}

export const ConfirmModal: React.FC<ConfirmModalProps> = ({
                                                              isOpen,
                                                              title,
                                                              content,
                                                              onConfirm,
                                                              onCancel,
                                                              isLoading = false,
                                                              confirmText,
                                                              cancelText,
                                                              isDanger = false
                                                          }) => {
    if (!isOpen) return null;
    const { t } = useTranslation();
    const finalConfirm = confirmText || t("common.confirm");
    const finalCancel = cancelText || t("common.cancel");

    // 使用 createPortal 将弹窗渲染到 document.body
    return createPortal(
        <div className="confirm-modal-overlay" onClick={!isLoading ? onCancel : undefined}>
            <div className="confirm-modal" onClick={(e) => e.stopPropagation()}>
                <div className="confirm-modal-header">
                    <h3 className="confirm-modal-title">{title}</h3>
                </div>

                <div className="confirm-modal-body">
                    {content}
                </div>

                <div className="confirm-modal-footer">
                    <button
                        className="btn-modal-cancel"
                        onClick={onCancel}
                        disabled={isLoading}
                    >
                        {finalCancel}
                    </button>
                    <button
                        className={isDanger ? "btn-modal-danger" : "btn-modal-primary"}
                        onClick={onConfirm}
                        disabled={isLoading}
                    >
                        {isLoading ? (
                            <span className="modal-loading-text">{t("common.processing")}</span>
                        ) : finalConfirm}
                    </button>
                </div>
            </div>
        </div>,
        document.body
    );
};
