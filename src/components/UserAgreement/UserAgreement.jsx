import React, { useEffect, useState } from 'react';
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';
import './UserAgreement.css';

function UserAgreement({ onAccept }) {
    const { t, i18n } = useTranslation();
    const [visible, setVisible] = useState(true);

    useEffect(() => {
        const checkAgreement = async () => {
            try {
                const config = await invoke('get_config');
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

    if (!visible) return null;

    const sections = [
        'privacy',
        'license',
        'minecraft',
        'disclaimer',
        'updates'
    ];

    return (
        <div className="user-agreement-overlay">
            <div className="user-agreement-modal glass">
                <h2>{t('UserAgreement.title')}</h2>
                <div className="user-agreement-scrollable">
                    <div className="user-agreement-content">
                        <p>{t('UserAgreement.introduction')}</p>
                        <ol>
                            {sections.map(key => {
                                const title = t(`UserAgreement.sections.${key}.title`);
                                let items = t(`UserAgreement.sections.${key}.content`, { returnObjects: true });
                                if (!Array.isArray(items)) {
                                    items = [items];
                                }
                                return (
                                    <li key={key}>
                                        <strong>{title}</strong>
                                        <ul>
                                            {items.map((item, idx) => (
                                                <li key={idx}>{item}</li>
                                            ))}
                                        </ul>
                                    </li>
                                );
                            })}
                        </ol>
                        <p>{t('UserAgreement.contact')}</p>
                        <p style={{ marginTop: '1rem', fontWeight: 'bold' }}>{t('UserAgreement.thanks')}</p>
                    </div>
                </div>
                <button className="user-agreement-button" onClick={handleAccept}>
                    {t('UserAgreement.accept_button')}
                </button>
            </div>
        </div>
    );
}

export default UserAgreement;
