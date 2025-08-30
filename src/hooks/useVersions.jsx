import { useEffect, useState, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from 'react-i18next';
import releaseIcon from "../assets/img/minecraft/Release.png";
import previewIcon from "../assets/img/minecraft/Preview.png";
import educationPreviewIcon from "../assets/img/minecraft/EducationEditionPreview.png";
import educationIcon from "../assets/img/minecraft/EducationEdition.png";
import unknownIcon from "../assets/feather/box.svg";
import i18next from "i18next";

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
        localStorage.setItem("launchCounts", JSON.stringify(counts));
    };

    const getVersionType = (name) => {
        if (name.includes("EducationPreview")) return t('common.education_preview');
        if (name.includes("Education")) return t('common.education');
        if (name.includes("Preview")) return t('common.preview');
        if (name.includes("Beta")) return t('common.preview');
        return t('common.release');
    };

    const getVersionIcon = (name) => {
        if (name.includes("EducationPreview")) return educationPreviewIcon;
        if (name.includes("Education")) return educationIcon;
        if (name.includes("Beta")) return previewIcon;
        if (name) return releaseIcon;
        return unknownIcon;
    };

    const compareVersion = (a, b) => {
        const parseVersion = (v) => v.split(".").map(n => parseInt(n, 10) || 0);
        const pa = parseVersion(a), pb = parseVersion(b);
        for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
            const na = pa[i] || 0, nb = pb[i] || 0;
            if (na !== nb) return nb - na;
        }
        return 0;
    };

    // ✅ 用 useCallback 确保 reload 稳定
    const fetchVersions = useCallback(async () => {
        try {
            const resp = await invoke("get_version_list", { fileName: "" });
            if (resp && typeof resp === "object" && !Array.isArray(resp)) {
                const map = resp;
                const initCounts = loadLaunchCounts();
                Object.keys(initCounts).forEach(k => { if (!map[k]) delete initCounts[k]; });
                saveLaunchCounts(initCounts);
                setCounts(initCounts);

                let list = Object.entries(map).map(([folder, info]) => ({
                    folder,
                    name: info.name,
                    version: info.version,
                    type: getVersionType(info.name),
                    icon: getVersionIcon(info.name),
                }));

                list.sort((a, b) => {
                    const d = (initCounts[b.folder] || 0) - (initCounts[a.folder] || 0);
                    return d !== 0 ? d : compareVersion(a.version, b.version);
                });

                setVersions(list);
            }
        } catch (e) {
            console.error("获取版本列表失败:", e);
        }
    }, [t]);


    useEffect(() => {
        fetchVersions();
    }, [fetchVersions, i18next.language]);

    return { versions, counts, reload: fetchVersions };
};

export default useVersions;
