import React, { useEffect, useState } from 'react';
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';
import ReactMarkdown from 'react-markdown';
import { motion, AnimatePresence } from 'framer-motion'; // 引入动画库
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

    // 动画变体配置
    const overlayVariants = {
        hidden: { opacity: 0 },
        visible: { opacity: 1 },
        exit: { opacity: 0, transition: { duration: 0.2 } }
    };

    const modalVariants = {
        hidden: { opacity: 0, scale: 0.9, y: 20 },
        visible: {
            opacity: 1,
            scale: 1,
            y: 0,
            transition: { type: "spring", damping: 25, stiffness: 300 }
        },
        exit: {
            opacity: 0,
            scale: 0.95,
            y: -10,
            transition: { duration: 0.2 }
        }
    };

    return (
        <AnimatePresence>
            {visible && (
                <motion.div
                    className="user-agreement-overlay"
                    variants={overlayVariants}
                    initial="hidden"
                    animate="visible"
                    exit="exit"
                >
                    <motion.div
                        className="user-agreement-modal glass-panel"
                        variants={modalVariants}
                    >
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
                            <motion.button
                                className="accept-button"
                                onClick={handleAccept}
                                whileHover={{ scale: 1.02, boxShadow: "0 4px 15px rgba(var(--theme-color-rgb), 0.3)" }}
                                whileTap={{ scale: 0.98 }}
                            >
                                {t('UserAgreement.accept_button')}
                            </motion.button>
                        </div>
                    </motion.div>
                </motion.div>
            )}
        </AnimatePresence>
    );
}

export default UserAgreement;