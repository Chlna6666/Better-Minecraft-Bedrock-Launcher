import React, { useCallback, useEffect, useRef, useState, useMemo } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import * as mm from 'music-metadata-browser';
import { Buffer } from 'buffer';
import {
    Play, Pause, SkipForward, SkipBack,
    Shuffle, ListMusic, Volume2, VolumeX, Disc,
    Music2
} from "lucide-react";
import "./MusicPlayer.css";

if (typeof window !== 'undefined') {
    window.Buffer = window.Buffer || Buffer;
}

const MUSIC_DIRECTORY = "BMCBL/music/";
const LOCAL_STORAGE_MODE_KEY = "musicPlayerMode";
const LOCAL_STORAGE_VOL_KEY = "musicPlayerVolume";

const formatTime = (seconds: number) => {
    if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
    const m = Math.floor(seconds / 60);
    const s = Math.floor(seconds % 60);
    return `${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
};

// 滚动文字组件
const ScrollingText = ({ text, className }: { text: string; className?: string }) => (
    <div className={`mp-scroll-container ${className || ''}`}>
        <div className="mp-scroll-inner">
            <span>{text}</span>
            <span aria-hidden="true">{text}</span>
        </div>
    </div>
);

interface TrackMetadata {
    title: string;
    artist: string;
    coverUrl: string | null;
}

function MusicPlayer() {
    const { t } = useTranslation();

    // 状态
    const [isExpanded, setIsExpanded] = useState(false);
    const [tracks, setTracks] = useState<string[]>([]);
    const [selectedTrack, setSelectedTrack] = useState<string | null>(null);
    const [isPlaying, setIsPlaying] = useState(false);
    const [metadata, setMetadata] = useState<TrackMetadata>({ title: "Not Playing", artist: "Select a song", coverUrl: null });

    // 进度与音量
    const [currentTime, setCurrentTime] = useState(0);
    const [duration, setDuration] = useState(0);
    const [volume, setVolume] = useState(0.5);
    const [isMuted, setIsMuted] = useState(false);
    const [showVolumePopup, setShowVolumePopup] = useState(false);
    const [mode, setMode] = useState<"repeat" | "shuffle">("repeat");

    // 拖拽状态：isDragging 为 true 时，timeupdate 不会更新 currentTime
    const [isDragging, setIsDragging] = useState(false);
    // 拖拽时的临时进度值 (0-100)
    const [dragProgress, setDragProgress] = useState(0);

    const audioRef = useRef<HTMLAudioElement>(null);
    const containerRef = useRef<HTMLDivElement>(null);

    // --- 初始化 ---
    useEffect(() => {
        const storedMode = localStorage.getItem(LOCAL_STORAGE_MODE_KEY);
        if (storedMode === "shuffle" || storedMode === "repeat") setMode(storedMode);
        const storedVol = localStorage.getItem(LOCAL_STORAGE_VOL_KEY);
        if (storedVol) setVolume(parseFloat(storedVol));

        const loadTracks = async () => {
            try {
                const files = await invoke("read_music_directory", { directory: MUSIC_DIRECTORY }) as string[];
                if (files && files.length > 0) {
                    setTracks(files);
                    setSelectedTrack(files[0]);
                }
            } catch (error) { console.error("Music Load Error:", error); }
        };
        loadTracks();

        const handleClickOutside = (event: MouseEvent) => {
            if (containerRef.current && !containerRef.current.contains(event.target as Node)) {
                setIsExpanded(false);
                setShowVolumePopup(false);
            }
        };
        document.addEventListener("mousedown", handleClickOutside);
        return () => document.removeEventListener("mousedown", handleClickOutside);
    }, []);

    // --- 元数据 ---
    useEffect(() => {
        if (!selectedTrack) return;
        const fetchMetadata = async () => {
            try {
                const src = convertFileSrc(selectedTrack);
                const fileName = selectedTrack.split(/[/\\]/).pop() || "Unknown";

                const response = await fetch(src);
                const blob = await response.blob();
                const tags = await mm.parseBlob(blob);

                let cover = null;
                if (tags.common.picture && tags.common.picture.length > 0) {
                    const pic = tags.common.picture[0];
                    const base64 = btoa(new Uint8Array(pic.data).reduce((data, byte) => data + String.fromCharCode(byte), ''));
                    cover = `data:${pic.format};base64,${base64}`;
                }
                setMetadata({
                    title: tags.common.title || fileName.replace(/\.[^/.]+$/, ""),
                    artist: tags.common.artist || "Unknown Artist",
                    coverUrl: cover
                });
            } catch (e) {
                const fileName = selectedTrack.split(/[/\\]/).pop() || "Unknown";
                setMetadata({ title: fileName.replace(/\.[^/.]+$/, ""), artist: "Unknown Artist", coverUrl: null });
            }
        };
        fetchMetadata();
    }, [selectedTrack]);

    // --- 切歌逻辑 ---
    useEffect(() => {
        const audio = audioRef.current;
        if (!audio || !selectedTrack) return;

        // 重置状态
        setCurrentTime(0);
        setDuration(0);
        setIsDragging(false);
        setDragProgress(0);

        const src = convertFileSrc(selectedTrack);
        if (decodeURIComponent(audio.src) === decodeURIComponent(src)) return;

        try {
            audio.src = src;
            audio.load(); // 重新加载

            // 应用音量
            audio.volume = isMuted ? 0 : volume;

            // 尝试自动播放
            const playPromise = audio.play();
            if (playPromise !== undefined) {
                playPromise
                    .then(() => setIsPlaying(true))
                    .catch(() => setIsPlaying(false));
            }
        } catch (e) {
            console.error("Audio Error:", e);
        }
    }, [selectedTrack]);

    // --- 音量监听 ---
    useEffect(() => {
        if (audioRef.current) audioRef.current.volume = isMuted ? 0 : volume;
        localStorage.setItem(LOCAL_STORAGE_VOL_KEY, volume.toString());
    }, [volume, isMuted]);

    useEffect(() => { localStorage.setItem(LOCAL_STORAGE_MODE_KEY, mode); }, [mode]);

    // --- [核心修复] 音频事件处理 ---
    // 直接定义函数传给 audio 标签的 props，比 addEventListener 更可靠
    const onTimeUpdate = () => {
        const audio = audioRef.current;
        if (!audio || isDragging) return; // 拖拽时不更新

        setCurrentTime(audio.currentTime);

        // 双保险：有些格式 duration 加载慢
        if (Number.isFinite(audio.duration) && audio.duration !== duration) {
            setDuration(audio.duration);
        }
    };

    const onLoadedMetadata = () => {
        const audio = audioRef.current;
        if (audio && Number.isFinite(audio.duration)) {
            setDuration(audio.duration);
        }
    };

    const onPlay = () => setIsPlaying(true);
    const onPause = () => setIsPlaying(false);
    const onEnded = () => playNext(); // 自动下一首

    // --- 控制逻辑 ---
    const togglePlay = useCallback((e?: React.MouseEvent) => {
        e?.stopPropagation();
        const audio = audioRef.current;
        if (audio) {
            audio.paused ? audio.play().catch(() => {}) : audio.pause();
        }
    }, []);

    const playNext = useCallback(() => {
        if (!tracks.length) return;
        let nextIdx = (tracks.indexOf(selectedTrack || "") + 1) % tracks.length;
        if (mode === "shuffle") nextIdx = Math.floor(Math.random() * tracks.length);
        setSelectedTrack(tracks[nextIdx]);
    }, [tracks, mode, selectedTrack]);

    const playPrev = useCallback(() => {
        if (!tracks.length) return;
        let prevIdx = (tracks.indexOf(selectedTrack || "") - 1 + tracks.length) % tracks.length;
        if (mode === "shuffle") prevIdx = Math.floor(Math.random() * tracks.length);
        setSelectedTrack(tracks[prevIdx]);
    }, [tracks, mode, selectedTrack]);

    // --- [核心修复] 进度条拖拽 ---

    // 1. 开始拖拽 / 拖拽中
    const handleSeekChange = (e: React.ChangeEvent<HTMLInputElement>) => {
        setIsDragging(true);
        const newVal = Number(e.target.value);
        setDragProgress(newVal);
        // 实时更新时间显示 (视觉欺骗)
        if (duration > 0) setCurrentTime((newVal / 100) * duration);
    };

    // 2. 拖拽结束 (松手)
    const handleSeekCommit = () => {
        const audio = audioRef.current;
        if (audio && Number.isFinite(duration) && duration > 0) {
            const newTime = (dragProgress / 100) * duration;
            audio.currentTime = newTime;
        }
        // 稍微延迟一点重置状态，防止 timeupdate 闪回
        setTimeout(() => setIsDragging(false), 100);
    };

    const handleVolumeWheel = useCallback((e: React.WheelEvent) => {
        e.stopPropagation();
        const delta = e.deltaY > 0 ? -0.05 : 0.05;
        setVolume(v => Math.min(1, Math.max(0, v + delta)));
        setIsMuted(false);
    }, [volume]);

    // 计算进度条当前值：拖拽时用 dragProgress，播放时计算百分比
    const currentPercent = useMemo(() => {
        if (isDragging) return dragProgress;
        if (!duration || duration <= 0) return 0;
        return (currentTime / duration) * 100;
    }, [isDragging, dragProgress, currentTime, duration]);

    if (tracks.length === 0) return null;

    return (
        <div className="mp-navbar-wrapper" ref={containerRef}>
            {/* [修复] 直接绑定事件到标签，更稳定 */}
            <audio
                ref={audioRef}
                key={selectedTrack || "init"}
                onTimeUpdate={onTimeUpdate}
                onLoadedMetadata={onLoadedMetadata}
                onDurationChange={onLoadedMetadata} // 额外监听
                onPlay={onPlay}
                onPause={onPause}
                onEnded={onEnded}
            />

            {/* 1. 胶囊 */}
            <div
                className={`mp-mini-capsule ${isPlaying ? 'playing' : ''} ${isExpanded ? 'expanded' : ''}`}
                onClick={() => setIsExpanded(!isExpanded)}
            >
                <div className={`mp-mini-disc ${isPlaying ? 'rotating' : ''}`}>
                    {metadata.coverUrl ?
                        <img src={metadata.coverUrl} className="mp-mini-cover" alt="art" /> :
                        <Disc size={16} />
                    }
                </div>
                <div className="mp-mini-text-area">
                    <ScrollingText text={metadata.title} />
                </div>
                <button className="mp-mini-btn" onClick={togglePlay}>
                    {isPlaying ? <Pause size={10} fill="currentColor" /> : <Play size={10} fill="currentColor" className="play-fix" />}
                </button>
            </div>

            {/* 2. 下拉卡片 */}
            <div className={`mp-dropdown-card glass ${isExpanded ? 'visible' : ''}`}>
                <div className="ios-top-section">
                    <div className="ios-cover-shadow">
                        {metadata.coverUrl ? (
                            <img src={metadata.coverUrl} alt="Cover" className="ios-cover-img" />
                        ) : (
                            <div className="ios-cover-placeholder"><Disc size={32} opacity={0.5} /></div>
                        )}
                    </div>
                    <div className="ios-info-col">
                        <div className="ios-title-row">
                            <ScrollingText text={metadata.title} className="ios-title-scroll" />
                        </div>
                        <span className="ios-artist">{metadata.artist}</span>
                    </div>
                    <div className="ios-source-icon">
                        <div className="ios-icon-circle"><Music2 size={10} /></div>
                    </div>
                </div>

                <div className="ios-progress-section">
                    <div className="ios-slider-container">
                        {/* [修复] 必须同时监听 MouseUp 和 TouchEnd */}
                        <input
                            type="range"
                            className="ios-slider"
                            min={0} max={100} step={0.1}
                            value={currentPercent}
                            onChange={handleSeekChange}
                            onMouseUp={handleSeekCommit}
                            onTouchEnd={handleSeekCommit}
                            style={{ backgroundSize: `${currentPercent}% 100%` }}
                        />
                    </div>
                    <div className="ios-time-labels">
                        <span>{formatTime(currentTime)}</span>
                        <span>{formatTime(duration)}</span>
                    </div>
                </div>

                <div className="ios-controls-row">
                    <button
                        className={`ios-aux-btn ${mode === 'shuffle' ? 'active' : ''}`}
                        onClick={() => setMode(m => m === 'repeat' ? 'shuffle' : 'repeat')}
                    >
                        {mode === 'shuffle' ? <Shuffle size={18} /> : <ListMusic size={18} />}
                    </button>

                    <div className="ios-main-group">
                        <button onClick={playPrev} className="ios-skip-btn"><SkipBack size={24} fill="currentColor" /></button>
                        <button onClick={togglePlay} className="ios-play-btn">
                            {isPlaying ?
                                <Pause size={24} fill="currentColor" /> :
                                <Play size={24} fill="currentColor" className="play-offset" />
                            }
                        </button>
                        <button onClick={playNext} className="ios-skip-btn"><SkipForward size={24} fill="currentColor" /></button>
                    </div>

                    <div
                        className="ios-volume-container"
                        onWheel={handleVolumeWheel}
                        onMouseEnter={() => setShowVolumePopup(true)}
                        onMouseLeave={() => setShowVolumePopup(false)}
                    >
                        <button className="ios-aux-btn" onClick={() => setIsMuted(!isMuted)}>
                            {isMuted || volume === 0 ? <VolumeX size={18} /> : <Volume2 size={18} />}
                        </button>

                        <div className={`ios-volume-popup-wrapper ${showVolumePopup ? 'visible' : ''}`}>
                            <div className="ios-volume-popup">
                                <input
                                    type="range"
                                    className="ios-vol-slider"
                                    min={0} max={1} step={0.01}
                                    value={isMuted ? 0 : volume}
                                    onChange={(e) => {
                                        setVolume(Number(e.target.value));
                                        setIsMuted(false);
                                    }}
                                    style={{ backgroundSize: `100% ${volume * 100}%` }}
                                />
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}

export default React.memo(MusicPlayer);