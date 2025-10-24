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

    const getVersionType = (name) => {
        if (!name) return t('common.release');
        if (name.includes("EducationPreview")) return t('common.education_preview');
        if (name.includes("Education")) return t('common.education');
        if (name.includes("Preview")) return t('common.preview');
        if (name.includes("Beta")) return t('common.preview');
        return t('common.release');
    };

    const getVersionIcon = (name) => {
        if (!name) return unknownIcon;
        if (name.includes("EducationPreview")) return educationPreviewIcon;
        if (name.includes("Education")) return educationIcon;
        if (name.includes("Beta")) return previewIcon;
        return releaseIcon;
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

                let list = Object.entries(map).map(([folder, info]) => ({
                    folder,
                    name: info.name || "",
                    version: info.version || "",
                    path: info.path || "",
                    type: getVersionType(info.name || ""),
                    icon: getVersionIcon(info.name || ""),
                }));

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
