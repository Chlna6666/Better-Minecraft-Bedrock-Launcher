import React, { useEffect, useState } from "react";
import minimize from "../../assets/feather/minus.svg";
import close from "../../assets/feather/x.svg";
import "./Titlebar.css";
import MusicPlayer from "./MusicPlayer.jsx";
import { invoke } from "@tauri-apps/api/core";
import logo from "../../assets/logo.png";

function Titlebar({ title, onMinimize, onClose }) {
    const [appVersion, setAppVersion] = useState("");

    useEffect(() => {
        // 调用 Rust 命令获取版本号
        invoke('get_app_version')
            .then(version => {
                setAppVersion(version);
                console.log("App version:", version);
            })
            .catch(error => {
                console.error("Failed to get app version:", error);
            });
    }, []);

    return (
        <div className="titlebar">
            <div data-tauri-drag-region="" className="titlebar-left">
                <img src={logo} alt="App Logo" className="titlebar-logo" draggable="false" />
                <div className="titlebar-title">
                    Better Minecraft: Bedrock Launcher V{appVersion}
                </div>
            </div>
            <div className="titlebar-buttons">
                <div className="titlebar-button" id="titlebar-minimize" onClick={onMinimize}>
                    <img src={minimize} alt="Minimize"/>
                </div>
                <div className="titlebar-button" id="titlebar-close" onClick={onClose}>
                    <img src={close} alt="Close"/>
                </div>
            </div>
            <MusicPlayer /> {/* 独立于标题栏 */}
        </div>
    );
}

export default Titlebar;
