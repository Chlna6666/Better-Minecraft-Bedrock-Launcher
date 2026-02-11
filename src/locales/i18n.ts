import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import enUS from './en-US.json';
import zhCN from './zh-CN.json';
import zhTW from './zh-TW.json';
import jaJP from './ja-JP.json';
import koKR from './ko-KR.json';

export const SUPPORTED_LANGUAGES = {
    'en-US': {
        translation: enUS,
        label: 'English'
    },
    'zh-CN': {
        translation: zhCN,
        label: '简体中文'
    },
    'zh-TW': {
        translation: zhTW,
        label: '繁體中文'
    },
    'ja-JP': {
        translation: jaJP,
        label: '日本語'
    },
    'ko-KR': {
        translation: koKR,
        label: '한국어'
    },
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
