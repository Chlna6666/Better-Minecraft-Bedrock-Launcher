import React, { useEffect, useState } from 'react';
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';
import ReactMarkdown from 'react-markdown';
import { ShieldCheck } from 'lucide-react'; // 引入图标
import './UserAgreement.css';

// @ts-ignore
import zhContent from "../../locales/agreement/zh.md?raw";
// @ts-ignore
import enContent from "../../locales/agreement/en.md?raw";

function UserAgreement({ onAccept }: { onAccept?: () => void }) {
    const { t, i18n } = useTranslation();
    const [visible, setVisible] = useState(true);
    const [mdContent, setMdContent] = useState('');

    useEffect(() => {
        if (i18n.language && i18n.language.includes('zh')) {
            setMdContent(zhContent);
        } else {
            setMdContent(enContent);
        }
    }, [i18n.language]);

    useEffect(() => {
        const checkAgreement = async () => {
            try {
                const config: any = await invoke('get_config');
                if (config?.agreement_accepted === true) {
                    setVisible(false);
                }
            } catch (error) {
                console.error('Failed to get agreement status:', error);
            }
        };
        checkAgreement();
    }, []);

    const handleAccept = async () => {
        try {
            await invoke('set_config', {
                key: 'agreement_accepted',
                value: true
            });
            setVisible(false);
            if (onAccept) onAccept();
        } catch (error) {
            console.error('Failed to save agreement status:', error);
        }
    };

    return (
        visible ? (
            <div className="user-agreement-overlay ua-anim-backdrop">
                <div className="user-agreement-modal glass-panel ua-anim-modal">
                        {/* 顶部标题区 */}
                        <div className="modal-header">
                            <div className="icon-wrapper">
                                <ShieldCheck size={24} className="header-icon" />
                            </div>
                            <h2 className="modal-title">{t('UserAgreement.title')}</h2>
                        </div>

                        {/* 内容滚动区 */}
                        <div className="user-agreement-scrollable custom-scrollbar">
                            <div className="markdown-body">
                                <ReactMarkdown>{mdContent}</ReactMarkdown>
                            </div>
                        </div>

                        {/* 底部按钮区 (带渐变遮罩) */}
                        <div className="modal-footer">
                            <button
                                className="accept-button"
                                onClick={handleAccept}
                            >
                                {t('UserAgreement.accept_button')}
                            </button>
                        </div>
                </div>
            </div>
        ) : null
    );
}

export default UserAgreement;
