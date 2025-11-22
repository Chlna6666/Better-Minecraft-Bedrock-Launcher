import React, { useEffect, useState, useCallback } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import "./McPackManager.css";
import { MinecraftFormattedText } from "../../utils/MinecraftFormattedText.jsx";
import Select from "../../components/Select.jsx";
import {Input} from "../../components/index.js";

function McPackManager() {
    const { t, i18n } = useTranslation();
    const [packs, setPacks] = useState([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState(null);
    const [query, setQuery] = useState("");
    const [expanded, setExpanded] = useState(null);
    const [packType, setPackType] = useState("resource"); // Default to resource packs

    // normalize i18n.language to backend-friendly format (e.g. zh-CN -> zh_CN)
    const langParam = (i18n && i18n.language) ? i18n.language.replace(
        "-", "_") : "en_US";

    const fetchPacks = useCallback(async () => {
        setLoading(true);
        setError(null);
        try {
            const cmd = packType === "resource" ? "get_all_resource_packs" : "get_all_behavior_packs";
            // pass language param to backend
            const res = await invoke(cmd, { lang: langParam });
            setPacks(Array.isArray(res) ? res : []);
        } catch (e) {
            console.error(`${packType} packs fetch failed`, e);
            setError(String(e));
            setPacks([]);
        } finally {
            setLoading(false);
        }
    }, [packType, langParam]);

    // refetch when packType or language changes
    useEffect(() => {
        fetchPacks();
    }, [fetchPacks]);

    const filtered = packs.filter((p) => {
        const name =
            p.manifest_parsed?.header?.name ||
            p.manifest?.header?.name ||
            p.folder_name ||
            "";
        const desc =
            p.manifest_parsed?.header?.description ||
            p.manifest?.header?.description ||
            p.short_description ||
            "";
        const q = query.trim().toLowerCase();
        if (!q) return true;
        return (
            name.toString().toLowerCase().includes(q) ||
            desc.toString().toLowerCase().includes(q) ||
            p.folder_name.toLowerCase().includes(q) ||
            p.folder_path.toLowerCase().includes(q)
        );
    });

    return (
        <div className="mc-pack-section">
            <div className="vtoolbar">
                <Input
                    type="text"
                    placeholder={t('McPackManager.search')}
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    style={{ flex: 1 }}
                    inputStyle={{ height: '29px' }}
                />
                    <Select
                        value={packType}
                        onChange={(v) => setPackType(String(v))}
                        options={[
                            { value: "resource", label: t("McPackManager.resourcePacks") || "Resource Packs" },
                            { value: "behavior", label: t("McPackManager.behaviorPacks") || "Behavior Packs" },
                        ]}
                        size={13}
                    />

                <button onClick={fetchPacks} className="vrefresh-btn">
                    {t('common.refresh')}
                </button>
            </div>

            {error && <div className="mc-pack-error">Error: {error}</div>}
            {!error && loading && <div className="mc-pack-loading">Loading packs…</div>}
            {!loading && filtered.length === 0 && (
                <div className="mc-pack-empty">{t("McPackManager.empty")}</div>
            )}

            <div className="vlist-container">
                {filtered.map((p) => {
                    const displayName =
                        p.manifest_parsed?.header?.name ||
                        p.manifest?.header?.name ||
                        p.folder_name ||
                        "Unknown";
                    const shortDesc =
                        p.short_description ||
                        p.manifest_parsed?.header?.description ||
                        "";
                    const iconPath = p.icon_path ? convertFileSrc(p.icon_path) : null;
                    const isExpanded = expanded === p.folder_path;

                    return (
                        <div className="pack-card" key={p.folder_path}>
                            <div className="pack-row">
                                <div className="pack-left">
                                    {iconPath ? (
                                        <img src={iconPath} alt={`${displayName} icon`} className="pack-icon" />
                                    ) : (
                                        <div className="pack-icon placeholder" aria-hidden>
                                            <svg viewBox="0 0 24 24" width="36" height="36" focusable="false" aria-hidden>
                                                <path d="M3 3h18v18H3z" fill="none" stroke="currentColor" strokeWidth="1.2" />
                                                <path d="M7 14l3-4 2 3 3-5 2 3" fill="none" stroke="currentColor" strokeWidth="1.2" />
                                            </svg>
                                        </div>
                                    )}
                                </div>

                                <div className="pack-right">
                                    <div className="pack-info">
                                        <div className="pack-name">
                                            <MinecraftFormattedText text={String(displayName || "")} />
                                        </div>
                                        {shortDesc && (
                                            <div className="pack-desc">
                                                <MinecraftFormattedText text={String(shortDesc)} />
                                            </div>
                                        )}
                                    </div>
                                    <div className="pack-actions">
                                        <button
                                            className="btn"
                                            onClick={() => setExpanded(isExpanded ? null : p.folder_path)}
                                        >
                                            {isExpanded
                                                ? t("McPackManager.collapse") || "Collapse"
                                                : t("McPackManager.details") || "Details"}
                                        </button>
                                    </div>
                                </div>
                            </div>

                            {isExpanded && (
                                <div className="pack-expanded">
                                    <div className="manifest-section">
                                        <div className="manifest-title">{t("McPackManager.manifest") || "manifest.json"}</div>
                                        <pre className="manifest-json">{JSON.stringify(p.manifest, null, 2)}</pre>
                                    </div>
                                </div>
                            )}
                        </div>
                    );
                })}
            </div>
        </div>
    );
}

export default McPackManager;
