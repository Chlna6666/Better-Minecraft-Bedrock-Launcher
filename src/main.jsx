import React from "react";
import App from "./App";
import { createRoot } from 'react-dom/client';
import i18n, { SUPPORTED_LANGUAGES } from "./i18n/i18n.js";
import { I18nextProvider } from 'react-i18next';
import PluginHost from "./PluginHost.jsx";
import { invoke } from "@tauri-apps/api/core";
import {Toast} from "./components/Toast.jsx";

(async () => {
    const systemLocale = await invoke('get_locale');
    const validLocale = SUPPORTED_LANGUAGES[systemLocale] ? systemLocale : 'en-US';
    await i18n.changeLanguage(validLocale);

    createRoot(document.getElementById('root')).render(
        <I18nextProvider i18n={i18n}>
            <PluginHost>
            <Toast>
                <App />
            </Toast>
            </PluginHost>
        </I18nextProvider>
    );
})();
