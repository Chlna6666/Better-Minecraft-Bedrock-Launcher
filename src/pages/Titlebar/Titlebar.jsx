import React, { useEffect, useState, useCallback, useMemo } from "react";
import minimize from "../../assets/feather/minus.svg";
import close from "../../assets/feather/x.svg";
import "./Titlebar.css";
import MusicPlayer from "./MusicPlayer.jsx";
import { invoke } from "@tauri-apps/api/core";
import logo from "../../assets/logo.png";
import {getCurrentWindow} from "@tauri-apps/api/window";

function Titlebar() {
    const [appVersion, setAppVersion] = useState("");
    const appWindow = getCurrentWindow();

    useEffect(() => {
        invoke("get_app_version")
            .then((v) => {
                setAppVersion(v);
                console.log("App version:", v);
            })
            .catch((err) => {
                console.error("获取应用版本失败：", err);
            });
    }, []);

    // 最小化窗口
    const handleMinimize = useCallback(() => appWindow.minimize(), []);
    // 关闭窗口
    const handleClose = useCallback(() => appWindow.close(), []);

    return (
        <div data-tauri-drag-region=""  className="titlebar">
            <div data-tauri-drag-region=""  className="titlebar-left">
                <img data-tauri-drag-region=""  src={logo} alt="App Logo" className="titlebar-logo" draggable="false" />
                <div data-tauri-drag-region=""  className="titlebar-title">
                    Better Minecraft: Bedrock Launcher V{appVersion}
                </div>
            </div>
            <div className="titlebar-buttons">
                <div className="titlebar-button" id="titlebar-minimize" onClick={handleMinimize}>
                    <img src={minimize} alt="Minimize"/>
                </div>
                <div className="titlebar-button" id="titlebar-close" onClick={handleClose}>
                    <img src={close} alt="Close"/>
                </div>
            </div>
            <MusicPlayer />
        </div>
    );
}

export default React.memo(Titlebar);
