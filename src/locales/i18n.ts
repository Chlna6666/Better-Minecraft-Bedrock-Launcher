import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import enUS from './en-US.json';
import zhCN from './zh-CN.json';

export const SUPPORTED_LANGUAGES = {
    'en-US': {
        translation: enUS,
        label: 'English'
    },
    'zh-CN': {
        translation: zhCN,
        label: '简体中文'
    }
};

i18n
    .use(initReactI18next)
    .init({
        resources: SUPPORTED_LANGUAGES, // 结构已经匹配，直接赋值即可
        lng: 'zh-CN',
        fallbackLng: 'en-US',
        interpolation: {
            escapeValue: false
        },
        detection: {
            order: ['localStorage', 'navigator'],
            caches: ['localStorage']
        }
    });

export default i18n;