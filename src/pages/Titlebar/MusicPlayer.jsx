import React, { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import "./MusicPlayer.css";

import music from "../../assets/feather/music.svg";
import close from "../../assets/feather/x.svg";
import play from "../../assets/feather/play.svg";
import pause from "../../assets/feather/pause.svg";
import next from "../../assets/feather/skip-forward.svg";
import previous from "../../assets/feather/skip-back.svg";
import shuffleIcon from "../../assets/feather/shuffle.svg";
import repeatIcon from "../../assets/feather/repeat.svg";

function MusicPlayer() {
    const { t } = useTranslation();
    const [isExpanded, setIsExpanded] = useState(false);
    const [selectedTrack, setSelectedTrack] = useState(null);
    const [isPlaying, setIsPlaying] = useState(false);
    const [tracks, setTracks] = useState([]);
    const [currentTime, setCurrentTime] = useState(0);
    const [duration, setDuration] = useState(0);

    const [mode, setMode] = useState("repeat");
    const audioRef = useRef(null);
    const scrollContainerRef = useRef(null);

    const musicDirectory = "BMCBL/music/";
    const LOCALSTORAGE_KEY = "musicPlayerMode";

    useEffect(() => {
        try {
            const stored = localStorage.getItem(LOCALSTORAGE_KEY);
            if (stored === "shuffle" || stored === "repeat") {
                setMode(stored);
            }
        } catch (e) {
            // ignore
        }
    }, []);

    useEffect(() => {
        try {
            localStorage.setItem(LOCALSTORAGE_KEY, mode);
        } catch (e) {
            // ignore
        }
    }, [mode]);

    useEffect(() => {
        const loadTracks = async () => {
            try {
                const files = await invoke("read_music_directory", { directory: musicDirectory });
                console.log("读取到的音乐文件列表:", files);
                if (files && files.length > 0) {
                    setTracks(files);
                    setSelectedTrack((prev) => prev ?? files[0]);
                } else {
                    setTracks([]);
                    setSelectedTrack(null);
                }
            } catch (error) {
                console.error("读取音乐目录时出错:", error);
                setTracks([]);
                setSelectedTrack(null);
            }
        };
        loadTracks();
    }, []);

    // 当 selectedTrack 改变时设置音源并尝试播放（并处理 promise）
    useEffect(() => {
        const audio = audioRef.current;
        if (!audio) return;
        if (!selectedTrack) {
            audio.pause();
            audio.src = "";
            setIsPlaying(false);
            setCurrentTime(0);
            setDuration(0);
            return;
        }

        try {
            audio.src = convertFileSrc(selectedTrack);
            audio.load();
            // 尝试播放并根据结果设置状态
            audio.play().then(() => {
                setIsPlaying(true);
            }).catch((err) => {
                console.warn("播放被阻止或失败:", err);
                // 确保状态为 false（播放被阻止时 play() reject）
                setIsPlaying(false);
            });
        } catch (e) {
            console.error("设置音频源失败:", e);
            setIsPlaying(false);
        }
    }, [selectedTrack]);

    // 监听 audio 事件（timeupdate / metadata / play / pause）
    useEffect(() => {
        const audio = audioRef.current;
        if (!audio) return;

        const handleTimeUpdate = () => {
            setCurrentTime(audio.currentTime || 0);
        };
        const handleLoadedMetadata = () => {
            setDuration(audio.duration || 0);
        };
        const handlePlay = () => setIsPlaying(true);
        const handlePause = () => setIsPlaying(false);

        audio.addEventListener("timeupdate", handleTimeUpdate);
        audio.addEventListener("loadedmetadata", handleLoadedMetadata);
        audio.addEventListener("play", handlePlay);
        audio.addEventListener("pause", handlePause);

        return () => {
            audio.removeEventListener("timeupdate", handleTimeUpdate);
            audio.removeEventListener("loadedmetadata", handleLoadedMetadata);
            audio.removeEventListener("play", handlePlay);
            audio.removeEventListener("pause", handlePause);
        };
    }, []);

    useEffect(() => {
        if (isExpanded && selectedTrack && scrollContainerRef.current) {
            const container = scrollContainerRef.current;
            const textElement = container.querySelector("span");
            if (textElement) {
                container.classList.toggle("scroll", textElement.scrollWidth > container.clientWidth);
            }
        }
    }, [isExpanded, selectedTrack]);

    const toggleExpand = useCallback(() => {
        setIsExpanded((prev) => !prev);
    }, []);

    // 关键：使用 audio.paused 判断真实状态，且在 play/pause 调用后立刻设置状态（并处理 promise）
    const handlePlayPause = useCallback(() => {
        const audio = audioRef.current;
        if (!audio) return;

        // 使用 audio.paused 来判断真实播放状态
        if (audio.paused) {
            // 尝试播放
            audio.play().then(() => {
                setIsPlaying(true);
            }).catch((err) => {
                console.warn("播放失败:", err);
                setIsPlaying(false);
            });
        } else {
            // 立即更新 UI 状态以保证图标变化（事件监听会进一步同步）
            try {
                audio.pause();
                setIsPlaying(false);
            } catch (err) {
                console.warn("暂停失败:", err);
                // 不要把 state 留在播放状态
                setIsPlaying(false);
            }
        }
    }, []);

    const getRandomTrack = useCallback(() => {
        if (!tracks || tracks.length === 0) return null;
        if (tracks.length === 1) return tracks[0];
        const currentIndex = tracks.indexOf(selectedTrack);
        let randomIndex = Math.floor(Math.random() * tracks.length);
        let tries = 0;
        while (randomIndex === currentIndex && tries < 5) {
            randomIndex = Math.floor(Math.random() * tracks.length);
            tries++;
        }
        return tracks[randomIndex];
    }, [tracks, selectedTrack]);

    const handleNextTrack = useCallback(() => {
        if (!tracks || tracks.length === 0) return;
        if (mode === "shuffle") {
            const random = getRandomTrack();
            if (random) setSelectedTrack(random);
            return;
        }
        let currentIndex = tracks.indexOf(selectedTrack);
        if (currentIndex === -1) currentIndex = 0;
        const nextIndex = (currentIndex + 1) % tracks.length;
        setSelectedTrack(tracks[nextIndex]);
    }, [mode, tracks, selectedTrack, getRandomTrack]);

    const handlePreviousTrack = useCallback(() => {
        if (!tracks || tracks.length === 0) return;
        let currentIndex = tracks.indexOf(selectedTrack);
        if (currentIndex === -1) currentIndex = 0;
        const previousIndex = (currentIndex - 1 + tracks.length) % tracks.length;
        setSelectedTrack(tracks[previousIndex]);
    }, [tracks, selectedTrack]);

    const handleProgressChange = useCallback((event) => {
        const audio = audioRef.current;
        if (!audio) return;
        const value = Number(event.target.value);
        const dur = Number(duration) || 0;
        if (isNaN(value) || dur <= 0) return;
        const newTime = (value / 100) * dur;
        audio.currentTime = newTime;
        setCurrentTime(newTime);
    }, [duration]);

    const handleTrackEnd = useCallback(() => {
        if (!tracks || tracks.length === 0) return;

        if (mode === "shuffle") {
            const random = getRandomTrack();
            if (random) {
                setSelectedTrack(random);
                return;
            }
        }

        if (tracks.length === 1) {
            const audio = audioRef.current;
            if (audio) {
                audio.currentTime = 0;
                audio.play().catch((err) => {
                    console.warn("单曲循环重放失败:", err);
                });
            }
            return;
        }

        handleNextTrack();
    }, [mode, tracks, getRandomTrack, handleNextTrack]);

    const getFileName = useCallback((path) => {
        if (!path) return "";
        const fileNameWithExt = path.split(/[/\\]/).pop();
        if (!fileNameWithExt) return "";
        const lastDotIndex = fileNameWithExt.lastIndexOf(".");
        return lastDotIndex !== -1 ? fileNameWithExt.substring(0, lastDotIndex) : fileNameWithExt;
    }, []);

    const toggleMode = useCallback(() => {
        setMode((prev) => (prev === "shuffle" ? "repeat" : "shuffle"));
    }, []);

    const progressPercent = (() => {
        const d = Number(duration) || 0;
        if (d <= 0) return 0;
        const p = (Number(currentTime) || 0) / d * 100;
        if (isNaN(p) || !isFinite(p)) return 0;
        return Math.min(100, Math.max(0, p));
    })();

    if (tracks.length === 0) {
        return null; // 没有音乐文件时直接隐藏组件
    }

    return (
        <div>
            <div className={`music-player-container ${isExpanded ? "expanded" : ""}`}>
                {isExpanded ? (
                    <>
                        <button className="close-button" onClick={toggleExpand} aria-label="close">
                            <img src={close} alt="close" />
                        </button>
                        <div className="music-player">
                            <div className="music-info-container">
                                <div className="music-info-label">{t('MusicPlayer.now_playing')}</div>
                                <div ref={scrollContainerRef} className="music-info-scroll">
                                    <span>{getFileName(selectedTrack)}</span>
                                </div>
                            </div>

                            <input
                                type="range"
                                className="progress-bar"
                                min={0}
                                max={100}
                                step={0.1}
                                value={progressPercent}
                                onChange={handleProgressChange} // 改为 onChange
                            />

                            <div className="music-controls">
                                <button className="control-button" onClick={handlePreviousTrack} aria-label="previous">
                                    <img src={previous} alt="previous" />
                                </button>
                                <button className="control-button" onClick={handlePlayPause} aria-label="play-pause">
                                    <img src={isPlaying ? pause : play} alt="play/pause" />
                                </button>
                                <button className="control-button" onClick={handleNextTrack} aria-label="next">
                                    <img src={next} alt="next" />
                                </button>
                                <button className="control-button" onClick={toggleMode} aria-label="mode">
                                    <img src={mode === "shuffle" ? shuffleIcon : repeatIcon} alt={mode} />
                                </button>
                            </div>
                        </div>
                    </>
                ) : (
                    <button className="music-player-container" onClick={toggleExpand} aria-label="open-player">
                        <img src={music} alt="music" />
                    </button>
                )}
            </div>

            <audio
                ref={audioRef}
                onEnded={handleTrackEnd}
            />
        </div>
    );
}

export default React.memo(MusicPlayer);
