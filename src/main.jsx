import React from "react";
import App from "./App";
import { createRoot } from 'react-dom/client';
import i18n, { SUPPORTED_LANGUAGES } from "./locales/i18n.ts";
import { I18nextProvider } from 'react-i18next';
import PluginHost from "./PluginHost.jsx";
import { invoke } from "@tauri-apps/api/core";
import { Toast } from "./components/Toast.tsx";
import { BrowserRouter } from "react-router-dom";

(async () => {
    try {
        const systemLocale = await invoke('get_locale').catch(() => 'en-US');
        const validLocale = SUPPORTED_LANGUAGES[systemLocale] ? systemLocale : 'en-US';
        await i18n.changeLanguage(validLocale);
    } catch (e) {
        console.warn("Locale setup failed, fallback to default", e);
    }

    createRoot(document.getElementById('root')).render(
        <I18nextProvider i18n={i18n}>
            <PluginHost>
                <BrowserRouter>
                    <Toast>
                        <App />
                    </Toast>
                </BrowserRouter>
            </PluginHost>
        </I18nextProvider>
    );
})();