import {useEffect, useState, useCallback, useRef, useMemo} from "react";
import { useTranslation } from 'react-i18next';
import useVersions from "../../hooks/useVersions.jsx";
import "./VersionManager.css";
import unknownIcon from "../../assets/feather/box.svg";

function VersionManager() {
    const { t } = useTranslation();
    const { versions, counts, reload } = useVersions();
    const [filter, setFilter] = useState("");
    const [search, setSearch] = useState("");

    // 筛选逻辑
    const filteredVersions = useMemo(() => {
        return versions.filter(v => {
            const matchType = filter ? v.type?.toLowerCase() === filter.toLowerCase() : true;
            const matchSearch = search
                ? (v.name?.toLowerCase().includes(search.toLowerCase()) ||
                    v.version?.toLowerCase().includes(search.toLowerCase()) ||
                    v.folder?.toLowerCase().includes(search.toLowerCase())) // 支持 folder 搜索
                : true;
            return matchType && matchSearch;
        });
    }, [filter, search, versions]);

    // 提取唯一的类型列表
    const uniqueTypes = useMemo(() => {
        const types = Array.from(new Set(versions.map(v => v.type)));
        return types.filter(Boolean);
    }, [versions]);

    return (
        <div className="vlist-wrapper">
            <div className="vtoolbar">
                <input
                    type="text"
                    placeholder={t('common.search_placeholder')}
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    className="vsearch"
                />
                <select
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                    className="vfilter-dropdown"
                >
                    <option value="">{t('common.all_versions')}</option>
                    {uniqueTypes.map(type => (
                        <option key={type} value={type}>{type}</option>
                    ))}
                </select>
                <button onClick={reload} className="vrefresh-btn">
                    {t('common.refresh')}
                </button>
            </div>

            {/* 独立的可滚动区域 */}
            <div className="vlist-container">
                {filteredVersions.map(({ folder, name, version, type, icon }) => {
                    const count = counts[folder] || 0;
                    return (
                        <div key={folder} className="vcard">
                            <img
                                src={icon}
                                alt={name}
                                className="vimg"
                                onError={(e) => e.target.src = unknownIcon}
                            />
                            <div className="vdetails">
                                <div className="vname">{folder}</div>
                                <div className="vmeta">
                                    <span className="vbadge">{type}</span> {version} · {t('common.launch_count')}: {count}
                                </div>
                            </div>
                        </div>
                    );
                })}
                {filteredVersions.length === 0 && (
                    <div className="vempty">{t('common.no_result')}</div>
                )}
            </div>
        </div>
    );
}

export default VersionManager;
