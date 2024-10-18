import React, { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./MusicPlayer.css";

import music from "../../assets/feather/music.svg";
import close from "../../assets/feather/x.svg";
import play from "../../assets/feather/play.svg";
import pause from "../../assets/feather/pause.svg";
import next from "../../assets/feather/skip-forward.svg";
import previous from "../../assets/feather/skip-back.svg";
import shuffle from "../../assets/feather/shuffle.svg";
import repeat from "../../assets/feather/repeat.svg";

function MusicPlayer() {
    const [isExpanded, setIsExpanded] = useState(false);
    const [selectedTrack, setSelectedTrack] = useState(null);
    const [isPlaying, setIsPlaying] = useState(false);
    const [tracks, setTracks] = useState([]);
    const [currentTime, setCurrentTime] = useState(0);
    const [duration, setDuration] = useState(0);
    const [isShuffle, setIsShuffle] = useState(false);
    const audioRef = useRef(null);

    const musicDirectory = "BMCBL/music/";

    useEffect(() => {
        const loadTracks = async () => {
            try {
                const files = await invoke('read_music_directory', { directory: musicDirectory });
                if (files && files.length > 0) {
                    setTracks(files);
                    setSelectedTrack(files[0]);
                } else {
                    setTracks([]);
                }
            } catch (error) {
                console.error("Error reading music directory:", error);
            }
        };
        loadTracks();
    }, []);

    useEffect(() => {
        const loadTrackContent = async () => {
            if (selectedTrack && audioRef.current) {
                try {
                    const fileContent = await invoke('read_file_content', { filePath: selectedTrack });
                    const base64String = `data:audio/mp4;base64,${fileContent}`;
                    audioRef.current.src = base64String;
                    audioRef.current.play();
                    setIsPlaying(true);
                } catch (error) {
                    console.error("Error loading track content:", error);
                }
            }
        };
        loadTrackContent();
    }, [selectedTrack]);

    useEffect(() => {
        if (audioRef.current) {
            const handleTimeUpdate = () => setCurrentTime(audioRef.current.currentTime);
            const handleLoadedMetadata = () => setDuration(audioRef.current.duration);

            audioRef.current.addEventListener("timeupdate", handleTimeUpdate);
            audioRef.current.addEventListener("loadedmetadata", handleLoadedMetadata);

            return () => {
                if (audioRef.current) {
                    audioRef.current.removeEventListener("timeupdate", handleTimeUpdate);
                    audioRef.current.removeEventListener("loadedmetadata", handleLoadedMetadata);
                }
            };
        }
    }, []);

    useEffect(() => {
        if (isExpanded && selectedTrack) {
            const textElement = document.querySelector('.music-info-scroll span');
            const containerElement = document.querySelector('.music-info-scroll');
            if (textElement && containerElement) {
                if (textElement.scrollWidth > containerElement.clientWidth) {
                    containerElement.classList.add('scroll');
                } else {
                    containerElement.classList.remove('scroll');
                }
            }
        }
    }, [isExpanded, selectedTrack]);

    const toggleExpand = () => {
        setIsExpanded(!isExpanded);
    };

    const handlePlayPause = () => {
        if (isPlaying) {
            audioRef.current.pause();
        } else {
            audioRef.current.play();
        }
        setIsPlaying(!isPlaying);
    };

    const getRandomTrack = () => {
        const randomIndex = Math.floor(Math.random() * tracks.length);
        return tracks[randomIndex];
    };

    const handleNextTrack = () => {
        if (isShuffle) {
            setSelectedTrack(getRandomTrack());
        } else {
            const currentIndex = tracks.indexOf(selectedTrack);
            const nextIndex = (currentIndex + 1) % tracks.length;
            setSelectedTrack(tracks[nextIndex]);
        }
    };

    const handlePreviousTrack = () => {
        const currentIndex = tracks.indexOf(selectedTrack);
        const previousIndex = (currentIndex - 1 + tracks.length) % tracks.length;
        setSelectedTrack(tracks[previousIndex]);
    };

    const handleProgressChange = (event) => {
        const newTime = (event.target.value / 100) * duration;
        audioRef.current.currentTime = newTime;
        setCurrentTime(newTime);
    };

    const handleTrackEnd = () => {
        handleNextTrack();
    };

    const getFileName = (path) => {
        const fileName = path.split('/').pop();
        return fileName.split('.').slice(0, -1).join('.');
    };

    return (
        <div>
            {tracks.length > 0 && (
                <div className={`music-player-container ${isExpanded ? "expanded" : ""}`}>
                    {isExpanded && (
                        <>
                            <button className="close-button" onClick={toggleExpand}>
                                <img src={close} alt="close" />
                            </button>
                            <div className="music-player">
                                <div className="music-info-container">
                                    <div className="music-info-label">正在播放:</div>
                                    <div className="music-info-scroll">
                                        <span>{getFileName(selectedTrack)}</span>
                                    </div>
                                </div>
                                <input
                                    type="range"
                                    className="progress-bar"
                                    value={(currentTime / duration) * 100 || 0}
                                    onInput={handleProgressChange}
                                />
                                <div className="music-controls">
                                    <button className="control-button" onClick={handlePreviousTrack}>
                                        <img src={previous} alt="previous" />
                                    </button>
                                    <button className="control-button" onClick={handlePlayPause}>
                                        <img src={isPlaying ? pause : play} alt="play/pause" />
                                    </button>
                                    <button className="control-button" onClick={handleNextTrack}>
                                        <img src={next} alt="next" />
                                    </button>
                                    <button className="control-button" onClick={() => setIsShuffle(!isShuffle)}>
                                        <img src={isShuffle ? shuffle : repeat} alt="shuffle/repeat" />
                                    </button>
                                </div>
                            </div>
                        </>
                    )}
                    {!isExpanded && (
                        <button className="music-player-container" onClick={toggleExpand}>
                            <img src={music} alt="music" />
                        </button>
                    )}
                </div>
            )}
            <audio ref={audioRef} onEnded={handleTrackEnd} />
        </div>
    );
}

export default MusicPlayer;
