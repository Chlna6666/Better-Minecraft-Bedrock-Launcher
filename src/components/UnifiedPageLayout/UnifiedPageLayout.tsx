import React, { ReactNode, useEffect, useState, useMemo, useRef, useCallback, useLayoutEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
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
    t: (key: string) => string;
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

// --- Internal Pagination Component ---
const Pagination = React.memo(({ config }: { config: PaginationConfig }) => {
    const { currentPage, totalPages, onPageChange, t } = config;
    const [jumpInput, setJumpInput] = useState("");

    useEffect(() => { setJumpInput(""); }, [currentPage]);

    const handleJump = (e: React.KeyboardEvent) => {
        if (e.key === 'Enter') {
            const page = parseInt(jumpInput, 10);
            if (!isNaN(page) && page >= 1 && page <= totalPages) {
                onPageChange(page);
            }
        }
    };

    const paginationRange = useMemo(() => {
        const range = [];
        const delta = 1;
        const rangeLeft = currentPage - delta;
        const rangeRight = currentPage + delta;
        for (let i = 1; i <= totalPages; i++) {
            if (i === 1 || i === totalPages || (i >= rangeLeft && i <= rangeRight)) {
                range.push(i);
            } else if (i === rangeLeft - 1 || i === rangeRight + 1) {
                range.push("...");
            }
        }
        return range.filter((val, index, arr) => val !== arr[index - 1]);
    }, [currentPage, totalPages]);

    if (totalPages <= 1) return null;

    return (
        <div className="upl-footer">
            <button className="upl-page-btn" onClick={() => onPageChange(currentPage - 1)} disabled={currentPage === 1} title={t('DownloadPage.pagination_prev') || "Prev"}>
                <ChevronLeft size={18} />
            </button>
            {paginationRange.map((page, index) => {
                if (page === "...") return <span key={`dots-${index}`} style={{ padding: '0 8px', opacity: 0.5 }}>...</span>;
                return (
                    <button key={`pg-btn-${page}`} className={`upl-page-btn ${currentPage === page ? 'active' : ''}`} onClick={() => onPageChange(page as number)}>
                        {page}
                    </button>
                );
            })}
            <button className="upl-page-btn" onClick={() => onPageChange(currentPage + 1)} disabled={currentPage === totalPages} title={t('DownloadPage.pagination_next') || "Next"}>
                <ChevronRight size={18} />
            </button>
            <div className="upl-pagination-jumper">
                {t('DownloadPage.pagination_goto') || "Go to"}
                <input type="number" className="upl-page-input" value={jumpInput} onChange={(e) => setJumpInput(e.target.value)} onKeyDown={handleJump} placeholder={String(currentPage)} min={1} max={totalPages} />
            </div>
        </div>
    );
});

// --- Main Layout Component ---
export default function UnifiedPageLayout({
                                              activeTab,
                                              onTabChange,
                                              tabs,
                                              headerActions,
                                              children,
                                              className = "",
                                              useInnerContainer = true,
                                              pagination,
                                              contentRef,
                                              searchConfig,
                                              refreshConfig,
                                              enableScrollTop = false,
                                              hideScrollbar = false
                                          }: UnifiedPageLayoutProps) {

    const internalRef = useRef<HTMLDivElement>(null);
    const finalRef = contentRef || internalRef;

    const [showScrollTop, setShowScrollTop] = useState(false);

    // [修复逻辑] 切换 Tab 时重置滚动条
    useLayoutEffect(() => {
        const el = finalRef.current;
        if (el) {
            // 只有当实际上有滚动或者不在顶部时才强制重置，减少不必要的绘制
            if (el.scrollTop !== 0) {
                el.scrollTo({
                    top: 0,
                    left: 0,
                    behavior: 'instant' as ScrollBehavior // 强制瞬间归位
                });
            }
        }
    }, [activeTab, finalRef]);

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
            <div className="upl-bg-shape upl-shape-1" />
            <div className="upl-bg-shape upl-shape-2" />

            <div className="upl-glass-panel">
                {/* 1. Header */}
                <div className="upl-header">
                    <div className="upl-tab-switcher">
                        {tabs.map((tab) => (
                            <button key={tab.id} className={`upl-tab-btn ${activeTab === tab.id ? 'active' : ''}`} onClick={() => onTabChange(tab.id)}>
                                {tab.icon && <span style={{ marginRight: 6, display: 'flex' }}>{tab.icon}</span>}
                                {tab.label}
                            </button>
                        ))}
                    </div>

                    <div className="upl-header-actions">
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
                        {headerActions}
                        {refreshConfig && (
                            <button
                                className="upl-action-icon-btn"
                                onClick={refreshConfig.onRefresh}
                                title={refreshConfig.title || "Refresh"}
                            >
                                <RefreshCw size={18} className={refreshConfig.loading ? "upl-spin" : ""} />
                            </button>
                        )}
                    </div>
                </div>

                {/* 2. Content */}
                <div
                    className={`upl-content ${hideScrollbar ? 'upl-no-scrollbar' : ''}`}
                    ref={finalRef}
                >
                    <AnimatePresence mode="wait">
                        {useInnerContainer ? (
                            <motion.div
                                className="upl-inner-container"
                                // [微调] 减少 y 轴位移距离，使过渡更自然，减少视觉跳动感
                                initial={{ opacity: 0, y: 3 }}
                                animate={{ opacity: 1, y: 0 }}
                                exit={{ opacity: 0, y: -3 }}
                                transition={{ duration: 0.15, ease: "easeOut" }}
                                key={activeTab}
                            >
                                {children}
                            </motion.div>
                        ) : (
                            <motion.div
                                key={activeTab}
                                initial={{ opacity: 0, y: 3 }}
                                animate={{ opacity: 1, y: 0 }}
                                exit={{ opacity: 0, y: -3 }}
                                transition={{ duration: 0.15, ease: "easeOut" }}
                                style={{ height: '100%' }}
                            >
                                {children}
                            </motion.div>
                        )}
                    </AnimatePresence>

                    <AnimatePresence>
                        {enableScrollTop && showScrollTop && (
                            <motion.button
                                initial={{ opacity: 0, scale: 0.5, y: 20 }}
                                animate={{ opacity: 1, scale: 1, y: 0 }}
                                exit={{ opacity: 0, scale: 0.5, y: 20 }}
                                className="upl-scroll-top-btn"
                                onClick={scrollToTop}
                            >
                                <ArrowUp size={22} />
                            </motion.button>
                        )}
                    </AnimatePresence>
                </div>

                {/* 3. Footer */}
                {pagination && <Pagination config={pagination} />}
            </div>
        </div>
    );
}