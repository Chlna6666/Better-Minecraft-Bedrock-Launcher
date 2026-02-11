import { createRoot } from 'react-dom/client';
import ImportWindow from './pages/ImportWindow'; // 你之前的 ImportWindow 组件
import './index.css';
import './App.css';
import i18n, { SUPPORTED_LANGUAGES } from "./locales/i18n.ts";
import { I18nextProvider } from 'react-i18next';
import { invoke } from "@tauri-apps/api/core";

// 这里的渲染逻辑非常纯净，只加载导入功能
(async () => {
    try {
        const systemLocale = await invoke('get_locale').catch(() => 'en-US');
        const localeStr = String(systemLocale);
        const normalizedLocale = SUPPORTED_LANGUAGES[localeStr]
            ? localeStr
            : localeStr.replace('_', '-');
        const validLocale = SUPPORTED_LANGUAGES[normalizedLocale] ? normalizedLocale : 'en-US';
        await i18n.changeLanguage(validLocale);
    } catch (e) {
        console.warn("Locale setup failed, fallback to default", e);
    }

    createRoot(document.getElementById('root')!).render(
        <I18nextProvider i18n={i18n}>
            <ImportWindow />
        </I18nextProvider>
    );
})();
