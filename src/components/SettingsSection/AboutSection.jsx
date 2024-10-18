import React, { useEffect, useState } from "react";
import './AboutSection.css';
import * as shell from "@tauri-apps/plugin-shell";

import {invoke} from "@tauri-apps/api/core";
import Chlan6666 from "../../assets/img/about/Chlna6666.jpg";
import github from "../../assets/img/about/github.png";
import Tauri from "../../assets/img/about/Tauri.png";
import Fufuha from "../../assets/img/about/Fufuha.jpg";
import afdian from "../../assets/img/about/afdian.png";
import logo from "../../assets/logo.png";


function AboutSection() {
    const [loaded, setLoaded] = useState(false);
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
        // Simulate content loading
        const timer = setTimeout(() => {
            setLoaded(true);  // Set to true when content is loaded
        }, 130);  // Simulating a delay of 0.5 seconds

        return () => clearTimeout(timer);  // Cleanup timeout on unmount
    }, []);

    const openInBrowser = (url) => {
        shell.open(url);
    };

    return (
        <div className={`about-section ${loaded ? "loaded-content" : ""}`}>
            <div className={`about-card ${loaded ? "loaded-content" : ""}`}>
                <img src={Chlan6666} alt="icon" />
                <div className="about-card-content">
                    <h4>Chlna6666</h4>
                    <p>？？？</p>
                </div>
                <div className="about-card-buttons">
                    <button  onClick={() => openInBrowser('https://afdian.com/a/Chlna6666')}>赞助</button>
                </div>
            </div>

            <div className={`about-card ${loaded ? "loaded-content" : ""}`}>
                <img src={logo} alt="icon" style={{width: "50px", height: "50px", marginRight: "15px", borderRadius: "0%"}}/>
                <div className="about-card-content">
                    <h4>Better Minecraft:Bedrock Launcher</h4>
                    <p>当前版本：{appVersion}</p>
                </div>
                <div className="about-card-buttons">
                    <button onClick={() => openInBrowser('https://bmcbl.com')}>官网</button>
                </div>
            </div>

            <div className={`special-thanks ${loaded ? "loaded-content" : ""}`}>
                <h3>鸣谢</h3>
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={Tauri} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>Tauri</h4>
                        <p>提供了开发框架</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://v2.tauri.app/')}>官网</button>
                    </div>
                </div>
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src="https://avatars.githubusercontent.com/u/5191659?v=4" alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>MCMrARM</h4>
                        <p>提供 mc-w10-versiondb 版本库。</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://github.com/MCMrARM/mc-w10-versiondb')}>查看
                        </button>
                    </div>
                </div>
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={Fufuha} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>Fufuha</h4>
                        <p>提供背景图。</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://space.bilibili.com/1798893653/')}>查看
                        </button>
                    </div>
                </div>
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={github} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>提交推送等方式参加本项目的所有贡献者</h4>
                        <p>感谢开源社区支持</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button
                            onClick={() => openInBrowser('https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher/graphs/contributors')}>查看
                        </button>
                    </div>
                </div>
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={afdian} alt="icon"/>
                    <div className="special-thanks-card-content">
                        <h4>赞助者</h4>
                        <p>感谢各位对BMCBL的支持。</p>
                    </div>
                    <div className="special-thanks-card-buttons">
                        <button onClick={() => openInBrowser('https://bmcbl.com/#sponsors')}>查看列表</button>
                    </div>
                </div>
                <div className={`special-thanks-card ${loaded ? "loaded-content" : ""}`}>
                    <img src={logo} alt="icon" style={{width: "50px", height: "50px", marginRight: "15px", borderRadius: "0%"}}/>
                    <div className="special-thanks-card-content">
                        <h4>BMCBL用户</h4>
                        <p>对本项目的支持</p>
                    </div>
                </div>
            </div>
            <div className={`legal-notices ${loaded ? "loaded-content" : ""}`}>
                <h3>法律声明</h3>
                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4><a
                            onClick={() => openInBrowser('https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher')}>版权</a>
                        </h4>
                        <p>版权所有 © Chlna6666</p>
                    </div>
                </div>
                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4><a onClick={() => openInBrowser('https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher')}>用户协议</a></h4>
                        <p>点击按键查看</p>
                    </div>
                </div>
                <div className={`legal-notices-card ${loaded ? "loaded-content" : ""}`}>
                    <div className="legal-notices-card-content">
                        <h4><a onClick={() => openInBrowser('https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher')}>开源协议</a></h4>
                        <p>GPL-3.0 license</p>
                    </div>
                </div>
            </div>
        </div>
    );
}

export default AboutSection;
