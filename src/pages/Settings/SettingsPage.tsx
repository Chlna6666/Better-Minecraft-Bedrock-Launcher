/* src/pages/Settings/SettingsPage.tsx */
import React, { useState, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Gamepad2, Rocket, Palette, Info } from 'lucide-react';
import './Settings.css';

import UnifiedPageLayout from '../../components/UnifiedPageLayout/UnifiedPageLayout';
import BackgroundLayer from '../../components/BackgroundLayer';
import { useAppConfig } from '../../hooks/useAppConfig';

import Game from './Game';
import Launcher from './Launcher';
import Customization from './Customization';
import About from './About';

// 简单的辅助组件：如果 not active，就隐藏，而不是销毁
const TabPanel = ({ active, children }: { active: boolean; children: React.ReactNode }) => {
    // 性能优化：如果不活动且从未渲染过，可以考虑不渲染 children（可选），这里保持 display:none 以保留状态
    return (
        <div
            style={{
                display: active ? 'block' : 'none',
                width: '100%',
            }}
        >
            {children}
        </div>
    );
};

export default function SettingsPage() {
    const { t } = useTranslation();
    const [activeTab, setActiveTab] = useState('game');
    const { backgroundImageUrl } = useAppConfig();

    // [优化] 使用 useMemo 缓存 tabs，防止每次 render 都生成新数组导致 Layout 重新测量
    const tabs = useMemo(() => [
        { id: 'game', label: t("Settings.tabs.game"), icon: <Gamepad2 size={18} /> },
        { id: 'launcher', label: t("Settings.tabs.launcher"), icon: <Rocket size={18} /> },
        { id: 'customization', label: t("Settings.tabs.customization"), icon: <Palette size={18} /> },
        { id: 'about', label: t("Settings.tabs.about"), icon: <Info size={18} /> },
    ], [t]);

    return (
        <>
            <BackgroundLayer url={backgroundImageUrl} />

            <UnifiedPageLayout
                activeTab={activeTab}
                onTabChange={setActiveTab}
                tabs={tabs}
                useInnerContainer={false}
                hideScrollbar={true}
            >
                <div className="settings-content-scroll">
                    <TabPanel active={activeTab === 'game'}>
                        <Game />
                    </TabPanel>

                    <TabPanel active={activeTab === 'launcher'}>
                        <Launcher />
                    </TabPanel>

                    <TabPanel active={activeTab === 'customization'}>
                        <Customization />
                    </TabPanel>

                    <TabPanel active={activeTab === 'about'}>
                        <About />
                    </TabPanel>
                </div>
            </UnifiedPageLayout>
        </>
    );
}