import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import enUS from './locales/en-US.json';
import zhCN from './locales/zh-CN.json';

// 统一合法语言列表
export const SUPPORTED_LANGUAGES = {
    'en-US': { translation: enUS, label: 'English' },
    'zh-CN': { translation: zhCN, label: '简体中文' }
};

i18n.use(initReactI18next).init({
    resources: Object.fromEntries(
        Object.entries(SUPPORTED_LANGUAGES).map(([key, value]) => [key, { translation: value.translation }])
    ),
    lng: 'en-US', // 先默认
    fallbackLng: 'en-US',
    interpolation: { escapeValue: false }
});

export default i18n;
