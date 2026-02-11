import React, { useEffect, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { User, ChevronDown, Check, Users } from 'lucide-react'; // 引入 Users 图标
import { motion, AnimatePresence } from 'framer-motion';
import { useTranslation } from 'react-i18next';
import './GDKUserSelect.css';

interface GDKUser {
    user_id?: string | number;
    userId?: string | number;
    user_folder?: string;
    xuid?: string;
    displayName?: string;
    name?: string;
}

interface Props {
    onChange: (userId: string) => void;
    currentUserId: string | null;
    edition: string;
}

export const GDKUserSelect: React.FC<Props> = ({ onChange, currentUserId, edition }) => {
    const [users, setUsers] = useState<GDKUser[]>([]);
    const [isOpen, setIsOpen] = useState(false);
    const containerRef = useRef<HTMLDivElement>(null);
    const { t } = useTranslation();

    const getUserInfo = (u: GDKUser, index: number) => {
        const rawId = u.user_folder || u.xuid || u.user_id || u.userId;
        let realId = String(rawId || '');
        let displayName = u.displayName || u.name || (u.user_folder ? String(u.user_folder) : null) || t("GDKUserSelect.user_n", { index: index + 1 });

        // [新增] 针对 Shared 用户的优化显示
        if (realId.toLowerCase() === 'shared') {
            displayName = t("GDKUserSelect.shared_label");
        }

        return { realId, displayName };
    };

    useEffect(() => {
        let isMounted = true;
        invoke<GDKUser[]>('get_gdk_users', { edition: edition || 'release' })
            .then(list => {
                if (!isMounted) return;
                const validList = Array.isArray(list) ? list : [];
                setUsers(validList);

                if (validList.length > 0) {
                    const { realId: firstId } = getUserInfo(validList[0], 0);
                    // 检查当前选中是否有效，无效则选中第一个
                    const isValidUser = currentUserId && validList.some((u, idx) =>
                        getUserInfo(u, idx).realId === currentUserId
                    );
                    if (!currentUserId || !isValidUser) {
                        if (firstId) onChange(firstId);
                    }
                } else {
                    if (currentUserId) onChange('');
                }
            })
            .catch(err => {
                console.error("Failed to load GDK users:", err);
                if (isMounted) setUsers([]);
            });

        return () => { isMounted = false; };
    }, [edition, t]);

    useEffect(() => {
        const handleClickOutside = (event: MouseEvent) => {
            if (containerRef.current && !containerRef.current.contains(event.target as Node)) {
                setIsOpen(false);
            }
        };
        document.addEventListener('mousedown', handleClickOutside);
        return () => document.removeEventListener('mousedown', handleClickOutside);
    }, []);

    if (users.length === 0) return null;

    const currentUser = users.find((u, idx) => {
        const { realId } = getUserInfo(u, idx);
        return realId === currentUserId;
    });

    const { displayName: currentLabel, realId: currentIdVal } = currentUser
        ? getUserInfo(currentUser, 0)
        : { displayName: t("GDKUserSelect.select_user"), realId: "" };

    // 判断是否是 Shared 用户，显示不同图标
    const isShared = currentIdVal.toLowerCase() === 'shared';

    return (
        <div className="gdk-user-select-container" ref={containerRef}>
            <div
                className={`gdk-select-trigger ${isOpen ? 'is-open' : ''}`}
                onClick={() => setIsOpen(!isOpen)}
                title={t("GDKUserSelect.switch_user")}
            >
                <div className="gdk-current-value">
                    {/* 根据是否 Shared 显示不同图标 */}
                    {isShared ? <Users size={14} className="gdk-user-icon" /> : <User size={14} className="gdk-user-icon" />}
                    <span>{currentLabel}</span>
                </div>
                <ChevronDown size={14} className="gdk-chevron" />
            </div>

            <AnimatePresence>
                {isOpen && (
                    <motion.div
                        className="gdk-dropdown-menu"
                        initial={{ opacity: 0, y: -5, scale: 0.95 }}
                        animate={{ opacity: 1, y: 0, scale: 1 }}
                        exit={{ opacity: 0, y: -5, scale: 0.95 }}
                        transition={{ duration: 0.15, ease: "easeOut" }}
                    >
                        {users.map((u, index) => {
                            const { realId, displayName } = getUserInfo(u, index);
                            const isSelected = realId === currentUserId;
                            const isItemShared = realId.toLowerCase() === 'shared';

                            return (
                                <div
                                    key={`${realId}-${index}`}
                                    className={`gdk-option ${isSelected ? 'selected' : ''}`}
                                    onClick={() => {
                                        onChange(realId);
                                        setIsOpen(false);
                                    }}
                                >
                                    <div className="gdk-option-info">
                                        <div style={{display:'flex', alignItems:'center', gap: 6}}>
                                            {isItemShared && <Users size={12} style={{opacity:0.5}} />}
                                            <span className="gdk-option-name">{displayName}</span>
                                        </div>
                                        <span className="gdk-option-id">{realId}</span>
                                    </div>
                                    {isSelected && <Check size={14} className="gdk-check-icon" />}
                                </div>
                            );
                        })}
                    </motion.div>
                )}
            </AnimatePresence>
        </div>
    );
};
