import React from "react";

type Props = {
    title: React.ReactNode;
    desc?: React.ReactNode;
    className?: string;
    style?: React.CSSProperties;
};

export default function SettingText({ title, desc, className, style }: Props) {
    return (
        <div className={["setting-text", className].filter(Boolean).join(" ")} style={style}>
            <div className="setting-title">{title}</div>
            {desc ? <div className="setting-desc">{desc}</div> : null}
        </div>
    );
}

