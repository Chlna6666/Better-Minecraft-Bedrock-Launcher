import React, { useState } from 'react';
import { AnimatePresence } from 'framer-motion';
import { useTranslation } from 'react-i18next';
import { Gamepad2, Rocket, Palette, Info } from 'lucide-react';
import './Settings.css';

import UnifiedPageLayout from '../../components/UnifiedPageLayout/UnifiedPageLayout';
import Game from './Game';
import Launcher from './Launcher';
import Customization from './Customization';
import About from './About';

export default function SettingsPage() {
    const { t } = useTranslation();
    const [activeTab, setActiveTab] = useState('game');

    const tabs = [
        { id: 'game', label: t("Settings.tabs.game"), icon: <Gamepad2 size={18} /> },
        { id: 'launcher', label: t("Settings.tabs.launcher"), icon: <Rocket size={18} /> },
        { id: 'customization', label: t("Settings.tabs.customization"), icon: <Palette size={18} /> },
        { id: 'about', label: t("Settings.tabs.about"), icon: <Info size={18} /> },
    ];

    return (
        <UnifiedPageLayout
            activeTab={activeTab}
            onTabChange={setActiveTab}
            tabs={tabs}
            useInnerContainer={false}
            hideScrollbar={true} // [新增] 在这里添加属性来隐藏 UnifiedPageLayout 的滚动条
        >
            <div className="settings-content-scroll">
                {/* mode="wait" 是消除抖动的关键：等待上一个退出后再进入下一个 */}
                <AnimatePresence mode="wait">
                    {activeTab === 'game' && <Game key="game" />}
                    {activeTab === 'launcher' && <Launcher key="launcher" />}
                    {activeTab === 'customization' && <Customization key="customization" />}
                    {activeTab === 'about' && <About key="about" />}
                </AnimatePresence>
            </div>
        </UnifiedPageLayout>
    );
}