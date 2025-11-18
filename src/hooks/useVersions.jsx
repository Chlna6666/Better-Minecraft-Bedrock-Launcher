import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import releaseIcon from "../assets/img/minecraft/Release.png";
import previewIcon from "../assets/img/minecraft/Preview.png";
import educationPreviewIcon from "../assets/img/minecraft/EducationEditionPreview.png";
import educationIcon from "../assets/img/minecraft/EducationEdition.png";
import unknownIcon from "../assets/feather/box.svg";

const useVersions = () => {
    const { t } = useTranslation();
    const [versions, setVersions] = useState([]);
    const [counts, setCounts] = useState({});

    const loadLaunchCounts = () => {
        try {
            const raw = localStorage.getItem("launchCounts");
            return raw ? JSON.parse(raw) : {};
        } catch {
            return {};
        }
    };

    const saveLaunchCounts = (counts) => {
        try {
            localStorage.setItem("launchCounts", JSON.stringify(counts));
        } catch {
            // ignore storage errors
        }
    };

    // 返回规范化的版本类型 code（便于判断）
    const detectVersionType = (name) => {
        if (!name) return "release";
        if (name.includes("EducationPreview")) return "education_preview";
        if (name.includes("Education")) return "education";
        if (name.includes("Preview")) return "preview";
        if (name.includes("Beta")) return "preview";
        return "release";
    };

    // 本地化版本类型显示文本
    const versionTypeLabel = (type) => {
        switch (type) {
            case "education_preview": return t('common.education_preview');
            case "education": return t('common.education');
            case "preview": return t('common.preview');
            case "release": default: return t('common.release');
        }
    };

    // 版本图标仍然基于 name（如果需要也可以改为基于 kind）
    const getVersionIcon = (name) => {
        if (!name) return unknownIcon;
        if (name.includes("EducationPreview")) return educationPreviewIcon;
        if (name.includes("Education")) return educationIcon;
        if (name.includes("Beta")) return previewIcon;
        return releaseIcon;
    };

    // 应用类型 kind 本地化
    const getKindLabel = (kind) => {
        if (!kind) return null;
        const k = String(kind).toUpperCase();
        if (k === "GDK") return t('common.gdk') || "GDK";
        if (k === "UWP") return t('common.uwp') || "UWP";
        return k;
    };

    const compareVersion = (a = "", b = "") => {
        // 返回正值表示 b 更新 -> 用于将新版本排在前面（降序）
        const parseVersion = (v) => v.split(".").map(n => parseInt(n, 10) || 0);
        const pa = parseVersion(a);
        const pb = parseVersion(b);
        for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
            const na = pa[i] || 0, nb = pb[i] || 0;
            if (na !== nb) return nb - na; // nb>na => positive -> b 在前
        }
        return 0;
    };

    const fetchVersions = useCallback(async () => {
        try {
            const resp = await invoke("get_version_list", { fileName: "" });
            if (resp && typeof resp === "object" && !Array.isArray(resp)) {
                const map = resp;
                const initCounts = loadLaunchCounts();

                // 清理不在返回列表中的 launchCounts 键
                Object.keys(initCounts).forEach(k => { if (!map[k]) delete initCounts[k]; });
                saveLaunchCounts(initCounts);
                setCounts(initCounts);

                let list = Object.entries(map).map(([folder, info]) => {
                    const kind = info.kind || null; // 应用类型：GDK / UWP
                    const kindLbl = getKindLabel(kind); // 本地化显示（如果有）
                    const name = info.name || "";
                    const version = info.version || "";
                    const vType = detectVersionType(name); // 版本类型 code
                    const vTypeLbl = versionTypeLabel(vType); // 本地化显示

                    return {
                        folder,
                        name,
                        version,
                        path: info.path || "",
                        kind,           // 原始 kind 字段（方便逻辑判断）
                        kindLabel: kindLbl, // 本地化显示（可能为 null）
                        versionType: vType, // "release" | "preview" | "education" | "education_preview"
                        versionTypeLabel: vTypeLbl, // 用于 UI 显示
                        icon: getVersionIcon(name),
                    };
                });

                list.sort((a, b) => {
                    const d = (initCounts[b.folder] || 0) - (initCounts[a.folder] || 0); // launch count 降序
                    return d !== 0 ? d : compareVersion(a.version, b.version); // 否则按版本降序
                });

                setVersions(list);
            }
        } catch (e) {
            console.error("获取版本列表失败:", e);
        }
    }, [t]);

    useEffect(() => {
        fetchVersions();
    }, [fetchVersions]);

    return { versions, counts, reload: fetchVersions };
};

export default useVersions;
