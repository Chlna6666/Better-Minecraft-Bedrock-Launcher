import React, { useEffect, useMemo, useRef, useState, useCallback } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { AnimatePresence, motion } from "framer-motion";
import { AlertCircle, CheckCircle, Download, Loader2, Package, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useToast } from "../../../components/Toast";
import useVersions from "../../../hooks/useVersions";
import Select from "../../../components/Select";
import { importAssets, checkImportConflict } from "../../Manage/api/assetApi";
import "./CurseForgeInstallModal.css";

type Mod = {
  id: number;
  name: string;
  logo?: { url?: string; thumbnailUrl?: string };
};

type ModFile = {
  id: number;
  displayName: string;
  fileName: string;
  fileLength: number;
  downloadUrl?: string;
  fileDate?: string;
};

type TaskSnapshot = {
  id: string;
  stage: string;
  total: number | null;
  done: number;
  speedBytesPerSec: number;
  eta: string;
  percent: number | null;
  status: string;
  message: string | null;
};

type PreviewInfo = {
  name: string;
  description: string;
  icon: string | null;
  kind: string;
  version: string | null;
  size: number;
  valid?: boolean;
  invalid_reason?: string | null;
  sub_packs?: PreviewInfo[];
};

type ConflictInfo = {
  has_conflict: boolean;
  conflict_type: string | null;
  target_name: string;
  message: string;
  existing_pack_info?: PreviewInfo;
};

function formatSize(bytes: number) {
  if (!bytes) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`;
}

export function CurseForgeInstallModal({
  open,
  mod,
  file,
  onClose,
}: {
  open: boolean;
  mod: Mod | null;
  file: ModFile | null;
  onClose: () => void;
}) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const { versions } = useVersions();

  const [selectedFolder, setSelectedFolder] = useState<string>("");
  const [targetFolder, setTargetFolder] = useState<string>("");
  const [stage, setStage] = useState<"idle" | "downloading" | "inspecting" | "installing" | "conflict" | "success" | "error">("idle");
  const [error, setError] = useState<string | null>(null);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [snapshot, setSnapshot] = useState<TaskSnapshot | null>(null);
  const [downloadedPath, setDownloadedPath] = useState<string | null>(null);
  const [preview, setPreview] = useState<PreviewInfo | null>(null);
  const [conflict, setConflict] = useState<ConflictInfo | null>(null);
  const [showConflictDialog, setShowConflictDialog] = useState(false);

  const listeningRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (!open) return;
    if (versions.length > 0 && (!selectedFolder || !versions.find((v: any) => v.folder === selectedFolder))) {
      setSelectedFolder(versions[0].folder);
    }
  }, [open, versions, selectedFolder]);

  const versionOptions = useMemo(() => {
    return (versions || []).map((v: any) => ({
      label: `${v.folder} (${v.version}) ${v.versionTypeLabel || ""} - ${v.kindLabel || v.kind || "UWP"}`,
      value: v.folder,
    }));
  }, [versions]);

  const resetState = useCallback(() => {
    setStage("idle");
    setError(null);
    setTaskId(null);
    setSnapshot(null);
    setDownloadedPath(null);
    setPreview(null);
    setConflict(null);
    setShowConflictDialog(false);
    setTargetFolder("");
  }, []);

  useEffect(() => {
    if (!open) {
      resetState();
    }
  }, [open, resetState]);

  const stopListening = useCallback(() => {
    if (listeningRef.current) {
      try {
        listeningRef.current();
      } catch {}
      listeningRef.current = null;
    }
  }, []);

  const closeAndCleanup = useCallback(() => {
    stopListening();
    onClose();
  }, [onClose, stopListening]);

  const startDownload = useCallback(async () => {
    if (!file?.downloadUrl) {
      setError(t("CurseForgeInstall.no_download_url"));
      setStage("error");
      return;
    }
    if (!selectedFolder) {
      setError(t("CurseForgeInstall.no_target_version"));
      setStage("error");
      return;
    }

    const chosenFolder = selectedFolder;
    setTargetFolder(chosenFolder);

    stopListening();
    setError(null);
    setSnapshot(null);
    setDownloadedPath(null);
    setPreview(null);
    setConflict(null);
    setShowConflictDialog(false);

    setStage("downloading");
    try {
      const suggestedName = file.fileName || `curseforge_${file.id}.zip`;
      const id = await invoke<string>("download_resource_to_cache", {
        url: file.downloadUrl,
        fileName: suggestedName,
        md5: null,
      });
      setTaskId(id);

      const eventName = `task-update::${id}`;
      const unlisten = await (await import("@tauri-apps/api/event")).listen<TaskSnapshot>(eventName, (event) => {
        setSnapshot(event.payload);
        if (event.payload.status === "completed") {
          const p = event.payload.message || "";
          setDownloadedPath(p);
        }
        if (event.payload.status === "error") {
          setError(event.payload.message || "Task failed");
          setStage("error");
        }
        if (event.payload.status === "cancelled") {
          setStage("idle");
        }
      });
      listeningRef.current = unlisten;
    } catch (e: any) {
      setError(e?.message ? String(e.message) : String(e));
      setStage("error");
    }
  }, [file, selectedFolder, stopListening, t]);

  useEffect(() => {
    if (stage !== "downloading") return;
    if (!downloadedPath) return;
    const inspectAndInstall = async () => {
      setStage("inspecting");
      try {
        const lang = (i18n.language || "en-US").replace("_", "-");
        const info = await invoke<PreviewInfo>("inspect_import_file", { filePath: downloadedPath, lang });
        setPreview(info);
        if (info?.valid === false) {
          setError(info.invalid_reason || t("CurseForgeInstall.invalid_package"));
          setStage("error");
          return;
        }
      } catch (e: any) {
        setError(e?.message ? String(e.message) : String(e));
        setStage("error");
        return;
      }

      const folder = targetFolder || selectedFolder;
      const targetVersion: any = (versions || []).find((v: any) => v.folder === folder);
      if (!targetVersion) {
        setError(t("CurseForgeInstall.no_target_version"));
        setStage("error");
        return;
      }

      setStage("installing");
      setError(null);
      try {
        const enableIsolation = await ensureIsolationFlag(targetVersion.folder);

        const conflictInfo: ConflictInfo = await checkImportConflict({
          kind: targetVersion.kind || "uwp",
          folder: targetVersion.folder,
          filePath: downloadedPath,
          enableIsolation,
          edition: targetVersion.versionType,
          allowSharedFallback: false,
        });

        if (conflictInfo.has_conflict) {
          setConflict(conflictInfo);
          setShowConflictDialog(true);
          setStage("conflict");
          return;
        }

        await importAssets({
          kind: targetVersion.kind || "uwp",
          folder: targetVersion.folder,
          filePaths: [downloadedPath],
          enableIsolation,
          edition: targetVersion.versionType,
          overwrite: false,
          allowSharedFallback: false,
        });

        setStage("success");
        toast.success(t("CurseForgeInstall.install_success"));
        setTimeout(() => closeAndCleanup(), 800);
      } catch (e: any) {
        setError(e?.message ? String(e.message) : String(e));
        setStage("error");
      }
    };
    inspectAndInstall();
  }, [closeAndCleanup, downloadedPath, i18n.language, selectedFolder, stage, t, targetFolder, toast, versions]);

  const cancelDownload = useCallback(async () => {
    if (!taskId) return closeAndCleanup();
    try {
      await invoke("cancel_task", { taskId });
    } catch {}
    closeAndCleanup();
  }, [closeAndCleanup, taskId]);

  const ensureIsolationFlag = async (folder: string) => {
    try {
      const cfg: any = await invoke("get_version_config", { folderName: folder });
      return !!cfg?.enable_redirection;
    } catch {
      return false;
    }
  };

  const executeImportWithOverwrite = useCallback(async (overwrite: boolean) => {
    const folder = targetFolder || selectedFolder;
    if (!downloadedPath || !folder) return;
    const targetVersion: any = (versions || []).find((v: any) => v.folder === folder);
    if (!targetVersion) return;

    setStage("installing");
    setError(null);
    try {
      const enableIsolation = await ensureIsolationFlag(targetVersion.folder);
      await importAssets({
        kind: targetVersion.kind || "uwp",
        folder: targetVersion.folder,
        filePaths: [downloadedPath],
        enableIsolation,
        edition: targetVersion.versionType,
        overwrite,
        allowSharedFallback: false,
      });
      setStage("success");
      toast.success(t("CurseForgeInstall.install_success"));
      setTimeout(() => closeAndCleanup(), 800);
    } catch (e: any) {
      setError(e?.message ? String(e.message) : String(e));
      setStage("error");
    } finally {
      setShowConflictDialog(false);
    }
  }, [closeAndCleanup, downloadedPath, selectedFolder, t, targetFolder, toast, versions]);

  const percent = snapshot?.percent != null ? Math.max(0, Math.min(100, snapshot.percent)) : null;
  const speed = snapshot?.speedBytesPerSec ? `${(snapshot.speedBytesPerSec / 1024 / 1024).toFixed(2)} MB/s` : null;

  if (!open) return null;

  const logo = mod?.logo?.thumbnailUrl || mod?.logo?.url;

  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div className="cf-install-backdrop" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
          <motion.div
            className="cf-install-modal"
            initial={{ y: 30, opacity: 0 }}
            animate={{ y: 0, opacity: 1 }}
            exit={{ y: 20, opacity: 0 }}
            transition={{ type: "spring", stiffness: 260, damping: 24 }}
          >
            <div className="cf-install-head">
              <div className="cf-install-title">
                {logo ? <img src={logo} alt="" referrerPolicy="no-referrer" /> : <div className="cf-install-logo-fallback"><Package size={18} /></div>}
                <div className="cf-install-title-text">
                  <div className="cf-install-kicker">{t("CurseForgeInstall.title")}</div>
                  <div className="cf-install-name">{mod?.name || t("common.unknown")}</div>
                  <div className="cf-install-sub">{file?.displayName || file?.fileName || ""}</div>
                </div>
              </div>
              <button className="cf-close-btn" onClick={closeAndCleanup}><X size={16} /></button>
            </div>

            <div className="cf-install-body custom-scrollbar">
              <div className="cf-install-section">
                <div className="cf-install-row">
                  <div className="cf-install-label">{t("CurseForgeInstall.target_version")}</div>
                  <div className="cf-install-control">
                    <Select
                      value={selectedFolder}
                      onChange={setSelectedFolder}
                      options={versionOptions}
                      size="md"
                      className="cf-install-select"
                      placeholder={t("CurseForgeInstall.select_version")}
                      disabled={stage !== "idle" && stage !== "error" && stage !== "success"}
                    />
                  </div>
                </div>
              </div>

              <div className="cf-install-section">
                {stage === "idle" && (
                  <div className="cf-install-state">
                    <button className="cf-install-primary" onClick={startDownload}>
                      <Download size={16} /> {t("CurseForgeInstall.download_and_install")}
                    </button>
                    <div className="cf-install-hint">
                      {t("CurseForgeInstall.download_to_cache")} ({formatSize(file?.fileLength || 0)})
                    </div>
                  </div>
                )}

                {(stage === "downloading" || stage === "inspecting") && (
                  <div className="cf-install-state">
                    <div className="cf-install-progress-title">
                      <Loader2 size={16} className="spin" />
                      {stage === "downloading" ? t("CurseForgeInstall.downloading") : t("CurseForgeInstall.inspecting")}
                    </div>
                    <div className="cf-install-progress-bar">
                      <div className="cf-install-progress-fill" style={{ width: `${percent ?? 8}%` }} />
                    </div>
                    <div className="cf-install-progress-meta">
                      <span className="tabular-nums">{percent != null ? `${percent.toFixed(1)}%` : "--"}</span>
                      <span className="tabular-nums">{speed || "--"}</span>
                      <span className="tabular-nums">{snapshot?.eta || "--"}</span>
                    </div>
                    <button className="cf-install-secondary" onClick={cancelDownload}>{t("common.cancel")}</button>
                  </div>
                )}

                {(stage === "installing" || stage === "conflict") && (
                  <div className="cf-install-state">
                    <div className="cf-install-progress-title">
                      <Loader2 size={16} className="spin" />
                      {t("CurseForgeInstall.installing")}
                    </div>
                    {preview && (
                      <div className="cf-install-mini-preview">
                        <div className="cf-install-mini-icon">
                          {preview.icon ? <img src={preview.icon} alt="" /> : <Package size={16} />}
                        </div>
                        <div className="cf-install-mini-text">
                          <div className="cf-install-mini-name">{preview.name || file?.fileName}</div>
                          <div className="cf-install-mini-sub">{preview.kind || ""}</div>
                        </div>
                      </div>
                    )}
                  </div>
                )}

                {stage === "success" && (
                  <div className="cf-install-state success">
                    <CheckCircle size={18} />
                    {t("CurseForgeInstall.install_success")}
                  </div>
                )}

                {stage === "error" && (
                  <div className="cf-install-state error">
                    <AlertCircle size={18} />
                    <div className="cf-install-error-text">{error || t("common.unknown")}</div>
                    <div className="cf-install-error-actions">
                      <button className="cf-install-secondary" onClick={closeAndCleanup}>{t("common.close")}</button>
                      <button className="cf-install-primary" onClick={startDownload}>{t("InstallProgressBar.retry")}</button>
                    </div>
                  </div>
                )}
              </div>
            </div>

            {showConflictDialog && conflict && (
              <div className="cf-install-conflict">
                <div className="cf-install-conflict-title">{t("CurseForgeInstall.conflict_title")}</div>
                <div className="cf-install-conflict-text">{conflict.message}</div>
                <div className="cf-install-conflict-actions">
                  <button className="cf-install-secondary" onClick={() => setShowConflictDialog(false)}>{t("common.cancel")}</button>
                  <button className="cf-install-primary" onClick={() => executeImportWithOverwrite(true)}>{t("CurseForgeInstall.overwrite")}</button>
                </div>
              </div>
            )}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body
  );
}
