import React, { useCallback, useEffect, useRef, useState, useMemo } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import * as mm from 'music-metadata-browser';
import { Buffer } from 'buffer';
import {
    Play, Pause, SkipForward, SkipBack,
    Shuffle, Repeat, Volume2, VolumeX, Disc,
    Music
} from "lucide-react";
import "./MusicPlayer.css";

// Polyfill for buffer in browser environment if needed
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
    return `${m}:${s.toString().padStart(2, '0')}`;
};

// 滚动文字组件：优化判断逻辑
const ScrollingText = ({ text, className, style }: { text: string; className?: string, style?: React.CSSProperties }) => {
    const getLength = (str: string) => {
        let len = 0;
        for (let i = 0; i < str.length; i++) {
            len += str.charCodeAt(i) > 127 ? 2 : 1;
        }
        return len;
    };

    const shouldScroll = getLength(text) > 20;

    if (!shouldScroll) {
        return (
            <div className={className} style={{ ...style, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                {text}
            </div>
        );
    }

    return (
        <div className={`mp-scroll-container ${className || ''}`} style={style}>
            <div className="mp-scroll-inner">
                <span>{text} &nbsp;&nbsp;&nbsp;&nbsp;</span>
                <span aria-hidden="true">{text} &nbsp;&nbsp;&nbsp;&nbsp;</span>
            </div>
        </div>
    );
};

interface TrackMetadata {
    title: string;
    artist: string;
    coverUrl: string | null;
}

function MusicPlayer() {
    const { t } = useTranslation();

    const [isExpanded, setIsExpanded] = useState(false);
    const [tracks, setTracks] = useState<string[]>([]);
    const [selectedTrack, setSelectedTrack] = useState<string | null>(null);
    const [isPlaying, setIsPlaying] = useState(false);
    const [metadata, setMetadata] = useState<TrackMetadata>({ title: "Not Playing", artist: "Select a song", coverUrl: null });

    const [currentTime, setCurrentTime] = useState(0);
    const [duration, setDuration] = useState(0);
    const [volume, setVolume] = useState(0.5);
    const [isMuted, setIsMuted] = useState(false);
    const [mode, setMode] = useState<"repeat" | "shuffle">("repeat");

    const [isDragging, setIsDragging] = useState(false);
    const [dragProgress, setDragProgress] = useState(0);

    const audioRef = useRef<HTMLAudioElement>(null);
    const containerRef = useRef<HTMLDivElement>(null);

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
            }
        };
        document.addEventListener("mousedown", handleClickOutside);
        return () => document.removeEventListener("mousedown", handleClickOutside);
    }, []);

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

    useEffect(() => {
        const audio = audioRef.current;
        if (!audio || !selectedTrack) return;

        setCurrentTime(0);
        setDuration(0);
        setIsDragging(false);
        setDragProgress(0);

        const src = convertFileSrc(selectedTrack);
        if (decodeURIComponent(audio.src) === decodeURIComponent(src)) return;

        audio.src = src;
        audio.load();
        audio.volume = isMuted ? 0 : volume;

        const playPromise = audio.play();
        if (playPromise !== undefined) {
            playPromise.then(() => setIsPlaying(true)).catch(() => setIsPlaying(false));
        }
    }, [selectedTrack]);

    useEffect(() => {
        if (audioRef.current) audioRef.current.volume = isMuted ? 0 : volume;
        localStorage.setItem(LOCAL_STORAGE_VOL_KEY, volume.toString());
    }, [volume, isMuted]);

    useEffect(() => { localStorage.setItem(LOCAL_STORAGE_MODE_KEY, mode); }, [mode]);

    const onTimeUpdate = () => {
        const audio = audioRef.current;
        if (!audio || isDragging) return;
        setCurrentTime(audio.currentTime);
        if (Number.isFinite(audio.duration) && audio.duration !== duration) {
            setDuration(audio.duration);
        }
    };

    const onLoadedMetadata = () => {
        const audio = audioRef.current;
        if (audio && Number.isFinite(audio.duration)) setDuration(audio.duration);
    };

    const togglePlay = useCallback((e?: React.MouseEvent) => {
        e?.stopPropagation();
        const audio = audioRef.current;
        if (audio) audio.paused ? audio.play().catch(()=>{}) : audio.pause();
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

    const handleSeekChange = (e: React.ChangeEvent<HTMLInputElement>) => {
        setIsDragging(true);
        const newVal = Number(e.target.value);
        setDragProgress(newVal);
        if (duration > 0) setCurrentTime((newVal / 100) * duration);
    };

    const handleSeekCommit = () => {
        const audio = audioRef.current;
        if (audio && Number.isFinite(duration) && duration > 0) {
            audio.currentTime = (dragProgress / 100) * duration;
        }
        setTimeout(() => setIsDragging(false), 100);
    };

    const currentPercent = useMemo(() => {
        if (isDragging) return dragProgress;
        if (!duration || duration <= 0) return 0;
        return (currentTime / duration) * 100;
    }, [isDragging, dragProgress, currentTime, duration]);

    if (tracks.length === 0) return null;

    return (
        <div className="mp-navbar-wrapper" ref={containerRef}>
            <audio
                ref={audioRef}
                onTimeUpdate={onTimeUpdate}
                onLoadedMetadata={onLoadedMetadata}
                onDurationChange={onLoadedMetadata}
                onPlay={() => setIsPlaying(true)}
                onPause={() => setIsPlaying(false)}
                onEnded={playNext}
            />

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
                    {isPlaying ? <Pause size={10} fill="currentColor" /> : <Play size={10} fill="currentColor" style={{ marginLeft: 1 }} />}
                </button>
            </div>

            <div className={`mp-dropdown-card ${isExpanded ? 'visible' : ''}`}>
                <div className="card-cover-section">
                    {metadata.coverUrl ? (
                        <img src={metadata.coverUrl} alt="Cover" className="card-cover-img" />
                    ) : (
                        <div style={{width:'100%', height:'100%', display:'flex', alignItems:'center', justifyContent:'center', color:'rgba(255,255,255,0.5)'}}>
                            <Music size={48} />
                        </div>
                    )}
                </div>

                <div className="card-info-section">
                    <div className="card-header-row">
                        <div style={{display:'flex', flexDirection:'column'}}>
                            <span className="card-context-text">Daily Mix</span>
                            <span className="card-track-count">{metadata.artist}</span>
                        </div>
                    </div>

                    <div className="card-title-row" style={{ overflow: 'hidden', width: '100%' }}>
                        <ScrollingText text={metadata.title} className="card-title-text" />
                    </div>

                    <div className="card-progress-row">
                        <div className="progress-track">
                            <div
                                className="progress-fill"
                                style={{ width: `${currentPercent}%` }}
                            >
                                <div className="progress-handle"></div>
                            </div>
                            <input
                                type="range"
                                className="progress-input"
                                min={0} max={100} step={0.1}
                                value={currentPercent}
                                onChange={handleSeekChange}
                                onMouseUp={handleSeekCommit}
                                onTouchEnd={handleSeekCommit}
                            />
                        </div>
                        <div className="time-labels">
                            <span>{formatTime(currentTime)}</span>
                            <span>{formatTime(duration)}</span>
                        </div>
                    </div>

                    <div className="card-controls-row">
                        <button
                            className={`ctrl-btn ${mode === 'shuffle' ? 'active' : ''}`}
                            onClick={() => setMode(m => m === 'repeat' ? 'shuffle' : 'repeat')}
                            data-bm-title={mode === 'shuffle' ? "Shuffle On" : "Repeat All"}
                        >
                            {mode === 'shuffle' ? <Shuffle size={16} /> : <Repeat size={16} />}
                        </button>

                        <button onClick={playPrev} className="ctrl-btn">
                            <SkipBack size={20} fill="currentColor" />
                        </button>

                        <button onClick={togglePlay} className="play-btn-large">
                            {isPlaying ?
                                <Pause size={20} fill="currentColor" /> :
                                <Play size={20} fill="currentColor" style={{marginLeft:2}} />
                            }
                        </button>

                        <button onClick={playNext} className="ctrl-btn">
                            <SkipForward size={20} fill="currentColor" />
                        </button>

                        <div className="volume-wrapper">
                            <button className="ctrl-btn" onClick={() => setIsMuted(!isMuted)}>
                                {isMuted || volume === 0 ? <VolumeX size={18} /> : <Volume2 size={18} />}
                            </button>

                            <div className="vol-popup">
                                <input
                                    type="range"
                                    className="vol-range-v"
                                    min={0} max={1} step={0.01}
                                    value={isMuted ? 0 : volume}
                                    onChange={(e) => {
                                        setVolume(Number(e.target.value));
                                        setIsMuted(false);
                                    }}
                                    // [关键] 使用 CSS 变量自适应颜色
                                    style={{
                                        background: `linear-gradient(to right, var(--vol-fill) ${isMuted ? 0 : volume * 100}%, var(--vol-bg) ${isMuted ? 0 : volume * 100}%)`
                                    }}
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
