import React, { ReactNode, useEffect, useState, useMemo, useRef, useCallback, useLayoutEffect } from 'react';
import { ChevronLeft, ChevronRight, Search, RefreshCw, ArrowUp } from 'lucide-react';
import './UnifiedPageLayout.css';

// --- Types ---
interface TabItem {
    id: string;
    label: string;
    icon?: ReactNode;
}

interface PaginationConfig {
    currentPage: number;
    totalPages: number;
    onPageChange: (page: number) => void;
    t?: (key: string) => string;
}

interface SearchConfig {
    value: string;
    onChange: (value: string) => void;
    placeholder?: string;
}

interface RefreshConfig {
    onRefresh: () => void;
    loading?: boolean;
    title?: string;
}

interface UnifiedPageLayoutProps {
    activeTab: string;
    onTabChange: (id: string) => void;
    tabs: TabItem[];
    headerActions?: ReactNode;
    children: ReactNode;
    className?: string;
    useInnerContainer?: boolean;
    pagination?: PaginationConfig;
    contentRef?: React.RefObject<HTMLDivElement>;
    searchConfig?: SearchConfig;
    refreshConfig?: RefreshConfig;
    enableScrollTop?: boolean;
    hideScrollbar?: boolean;
}

// --- Sub-Component: Pagination ---
const Pagination = React.memo(({ config }: { config: PaginationConfig }) => {
    const { currentPage, totalPages, onPageChange, t } = config;
    const [jumpInput, setJumpInput] = useState<string>("");

    const paginationRange = useMemo(() => {
        const range = [];
        const delta = 1;
        for (let i = 1; i <= totalPages; i++) {
            if (i === 1 || i === totalPages || (i >= currentPage - delta && i <= currentPage + delta)) {
                range.push(i);
            } else if (i === currentPage - delta - 1 || i === currentPage + delta + 1) {
                range.push("...");
            }
        }
        return range.filter((val, index, arr) => val !== arr[index - 1]);
    }, [currentPage, totalPages]);

    if (totalPages <= 1) return null;

    const clampPage = (page: number) => Math.min(totalPages, Math.max(1, page));

    const jumpTo = () => {
        const parsed = Number(jumpInput);
        if (!Number.isFinite(parsed)) return;
        const target = clampPage(Math.trunc(parsed));
        onPageChange(target);
        setJumpInput("");
    };

    return (
        <div className="upl-footer">
            <button className="upl-page-btn" onClick={() => onPageChange(currentPage - 1)} disabled={currentPage === 1}>
                <ChevronLeft size={16} />
            </button>
            {paginationRange.map((page, i) => (
                <React.Fragment key={i}>
                    {page === "..." ? (
                        <span className="upl-dots">...</span>
                    ) : (
                        <button
                            className={`upl-page-btn ${currentPage === page ? 'active' : ''}`}
                            onClick={() => onPageChange(page as number)}
                        >
                            {page}
                        </button>
                    )}
                </React.Fragment>
            ))}
            <button className="upl-page-btn" onClick={() => onPageChange(currentPage + 1)} disabled={currentPage === totalPages}>
                <ChevronRight size={16} />
            </button>

            <div className="upl-page-jump" aria-label="page-jump">
                <span className="upl-page-jump-label">{t ? t('DownloadPage.pagination_goto') : 'Go to'}</span>
                <input
                    className="upl-page-jump-input"
                    type="number"
                    inputMode="numeric"
                    min={1}
                    max={totalPages}
                    value={jumpInput}
                    onChange={(e) => setJumpInput(e.target.value)}
                    onWheel={(e) => {
                        (e.currentTarget as HTMLInputElement).blur();
                    }}
                    onKeyDown={(e) => {
                        if (e.key === 'Enter') jumpTo();
                    }}
                    placeholder={`${currentPage}/${totalPages}`}
                />
                <button
                    className="upl-page-jump-go"
                    type="button"
                    onClick={jumpTo}
                    disabled={!jumpInput.trim()}
                >
                    {t ? t('DownloadPage.pagination_go') : 'Go'}
                </button>
            </div>
        </div>
    );
});

// --- Sub-Component: Header (Optimized) ---
const LayoutHeader = React.memo(({
                                     tabs, activeTab, onTabChange, searchConfig, refreshConfig, headerActions
                                 }: Pick<UnifiedPageLayoutProps, 'tabs' | 'activeTab' | 'onTabChange' | 'searchConfig' | 'refreshConfig' | 'headerActions'>) => {

    // [优化] 使用 Ref 和 Style 实现高性能滑块，避免 Framer Motion 的 layoutId 计算
    const [pillStyle, setPillStyle] = useState<{ left: number; width: number; opacity: number }>({ left: 0, width: 0, opacity: 0 });
    const tabsRef = useRef<{ [key: string]: HTMLButtonElement | null }>({});

    useLayoutEffect(() => {
        const currentTabEl = tabsRef.current[activeTab];
        if (currentTabEl) {
            setPillStyle({
                left: currentTabEl.offsetLeft,
                width: currentTabEl.offsetWidth,
                opacity: 1
            });
        }
    }, [activeTab, tabs]); // tabs 依赖确保初始化时能正确获取

    return (
        <div className="upl-header">
            {/* 左侧：Tab 切换 */}
            <div className="upl-header-left">
                {/* 必须添加 position: relative 以便滑块定位。虽然 CSS 中可能有，但内联样式更保险 */}
                <div className="upl-tab-switcher" style={{ position: 'relative' }}>

                    {/* [核心修改] 独立的滑块层，兄弟节点，不再嵌套在 Button 内部 */}
                    <div
                        className="upl-active-pill"
                        style={{
                            position: 'absolute',
                            left: pillStyle.left,
                            width: pillStyle.width,
                            opacity: pillStyle.opacity,
                            height: 'calc(100% - 6px)', // 减去 padding (3px top + 3px bottom)
                            top: '3px',
                            transition: 'all 0.2s cubic-bezier(0.4, 0, 0.2, 1)', // 顺滑的 CSS 过渡
                            pointerEvents: 'none', // 防止遮挡点击
                            inset: 'auto' // 覆盖 CSS 中的 inset: 0
                        }}
                    />

                    {tabs.map((tab) => {
                        const isActive = activeTab === tab.id;
                        return (
                            <button
                                key={tab.id}
                                ref={(el) => (tabsRef.current[tab.id] = el)}
                                className={`upl-tab-btn ${isActive ? 'active' : ''}`}
                                onClick={() => onTabChange(tab.id)}
                            >
                                {/* 移除了内部的 motion.div */}
                                {tab.icon && <span className="upl-tab-icon">{tab.icon}</span>}
                                <span className="upl-tab-text">{tab.label}</span>
                            </button>
                        );
                    })}
                </div>
            </div>

            {/* 中间：搜索框 (Flex 1) */}
            <div className="upl-header-middle">
                {searchConfig && (
                    <div className="upl-search-wrapper">
                        <Search className="upl-search-icon" />
                        <input
                            type="text"
                            className="upl-search-input"
                            placeholder={searchConfig.placeholder || "Search..."}
                            value={searchConfig.value}
                            onChange={(e) => searchConfig.onChange(e.target.value)}
                        />
                    </div>
                )}
            </div>

            {/* 右侧：自定义操作 + 刷新 */}
            <div className="upl-header-right">
                {headerActions}

                {refreshConfig && (
                    <button className="upl-action-icon-btn" onClick={refreshConfig.onRefresh} data-bm-title={refreshConfig.title || "Refresh"}>
                        <RefreshCw size={18} className={refreshConfig.loading ? "upl-spin" : ""} />
                    </button>
                )}
            </div>
        </div>
    );
});

// --- Main Component ---
export default function UnifiedPageLayout({
                                              activeTab, onTabChange, tabs, headerActions, children,
                                              className = "", useInnerContainer = true, pagination,
                                              contentRef, searchConfig, refreshConfig,
                                              enableScrollTop = false, hideScrollbar = false
                                          }: UnifiedPageLayoutProps) {

    const internalRef = useRef<HTMLDivElement>(null);
    const finalRef = contentRef || internalRef;
    const [showScrollTop, setShowScrollTop] = useState(false);

    useEffect(() => {
        if (!enableScrollTop) return;
        const el = finalRef.current;
        if (!el) return;

        let ticking = false;
        const handleScroll = () => {
            if (!ticking) {
                requestAnimationFrame(() => {
                    if (el) setShowScrollTop(el.scrollTop > 300);
                    ticking = false;
                });
                ticking = true;
            }
        };
        el.addEventListener('scroll', handleScroll, { passive: true });
        return () => el.removeEventListener('scroll', handleScroll);
    }, [enableScrollTop, finalRef]);

    const scrollToTop = useCallback(() => {
        finalRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
    }, [finalRef]);

    return (
        <div className={`upl-container ${className}`}>
            <div className="upl-glass-panel">
                <LayoutHeader
                    tabs={tabs}
                    activeTab={activeTab}
                    onTabChange={onTabChange}
                    searchConfig={searchConfig}
                    headerActions={headerActions}
                    refreshConfig={refreshConfig}
                />

                <div className={`upl-content ${hideScrollbar ? 'upl-no-scrollbar' : ''}`} ref={finalRef}>
                    <div className={`upl-content-wrapper ${useInnerContainer ? 'upl-inner-container' : ''}`}>
                        {children}
                    </div>

                    {enableScrollTop && (
                        <button
                            className={`upl-scroll-top-btn ${showScrollTop ? 'is-visible' : ''}`}
                            onClick={scrollToTop}
                            aria-hidden={!showScrollTop}
                            tabIndex={showScrollTop ? 0 : -1}
                        >
                            <ArrowUp size={20} />
                        </button>
                    )}
                </div>

                {pagination && <Pagination config={pagination} />}
            </div>
        </div>
    );
}
