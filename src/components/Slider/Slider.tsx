import React, { useRef, useMemo } from 'react';
import './Slider.css';

interface SliderProps {
    min?: number;
    max?: number;
    step?: number;
    value: number;
    onChange: (val: number) => void;
    className?: string;
    style?: React.CSSProperties;
    disabled?: boolean;
}

export default function Slider({
                                   min = 0,
                                   max = 100,
                                   step = 1,
                                   value,
                                   onChange,
                                   className = '',
                                   style = {},
                                   disabled = false
                               }: SliderProps) {
    // 计算进度百分比
    const percentage = useMemo(() => {
        const p = ((value - min) / (max - min)) * 100;
        return Math.min(100, Math.max(0, p));
    }, [value, min, max]);

    const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
        onChange(Number(e.target.value));
    };

    return (
        <div
            className={`ui-slider-container ${className} ${disabled ? 'disabled' : ''}`}
            style={{ ...style, '--slider-percent': `${percentage}%` } as React.CSSProperties}
        >
            <input
                type="range"
                className="ui-slider-input"
                min={min}
                max={max}
                step={step}
                value={value}
                onChange={handleChange}
                disabled={disabled}
            />
            {/* 轨道背景 */}
            <div className="ui-slider-track">
                {/* 进度填充 */}
                <div className="ui-slider-fill" />
            </div>
        </div>
    );
}