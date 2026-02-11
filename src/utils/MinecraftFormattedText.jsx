import React, { useMemo } from "react";
import glyph_E0 from "../assets/img/minecraft/glyph_E0.png";
import glyph_E1 from "../assets/img/minecraft/glyph_E1.png";

/* 省略前文注释（与原始文件相同） */

const COLOR_MAP = {
    "0": "#000000",
    "1": "#0000AA",
    "2": "#00AA00",
    "3": "#00AAAA",
    "4": "#AA0000",
    "5": "#AA00AA",
    "6": "#FFAA00",
    "7": "#AAAAAA",
    "8": "#555555",
    "9": "#5555FF",
    a: "#55FF55",
    b: "#55FFFF",
    c: "#FF5555",
    d: "#FF55FF",
    e: "#FFFF55",
    f: "#FFFFFF",

    g: "#DDD605",
    h: "#E3D4D1",
    i: "#CECACA",
    j: "#443A3B",
    m: "#971607",
    n: "#B4684D",
    p: "#DEB12D",
    q: "#47A036",
    s: "#2CBAA8",
    t: "#21497B",
    u: "#9A5CC6",
    v: "#EB7114",
};

const STYLE_MAP = {
    l: { fontWeight: "700" },
    o: { fontStyle: "italic" },
    n: { textDecoration: "underline" },
    m: { textDecoration: "line-through" },
};

function mergeStyles(base, add) {
    const res = { ...base };
    if (base.textDecoration && add.textDecoration) {
        const parts = new Set(
            base.textDecoration.split(" ").concat(add.textDecoration.split(" "))
        );
        res.textDecoration = Array.from(parts).join(" ");
    } else if (add.textDecoration) {
        res.textDecoration = add.textDecoration;
    }
    for (const k of Object.keys(add)) {
        if (k === "textDecoration") continue;
        res[k] = add[k];
    }
    return res;
}

function lcg(seed) {
    let state = seed >>> 0;
    return function next() {
        state = (1664525 * state + 1013904223) >>> 0;
        return state;
    };
}

function isGlyphCode(codePoint) {
    return (codePoint >= 0xE000 && codePoint <= 0xE0FF) || (codePoint >= 0xE100 && codePoint <= 0xE1FF);
}

/* 缓存 glyph 样式，避免每次渲染都产生新对象 */
const glyphStyleCache = new Map();
function getCachedGlyphStyle(codePoint, glyphPx) {
    const key = `${codePoint}@${glyphPx}`;
    const v = glyphStyleCache.get(key);
    if (v) return v;
    // 生成按 glyphPx 缩放的样式
    const SPRITE_SIZE = 32;   // 每格原始像素
    const SPRITE_FULL = 512;  // sprite sheet 原始尺寸
    const index = codePoint & 0xff;
    const row = Math.floor(index / 16);
    const col = index % 16;
    const high = codePoint & 0xff00;
    const file = (high === 0xe000) ? glyph_E0 : glyph_E1;

    const scale = glyphPx / SPRITE_SIZE; // 比例
    const backgroundSizeX = SPRITE_FULL * scale;
    const backgroundSizeY = SPRITE_FULL * scale;
    const posX = -col * SPRITE_SIZE * scale;
    const posY = -row * SPRITE_SIZE * scale;

    const style = {
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: `${glyphPx}px`,
        height: `${glyphPx}px`,
        backgroundImage: `url(${file})`,
        backgroundSize: `${backgroundSizeX}px ${backgroundSizeY}px`,
        backgroundPosition: `${posX}px ${posY}px`,
        verticalAlign: "middle",
    };
    glyphStyleCache.set(key, style);
    return style;
}

export function parseMinecraftFormatting(input, obfuscatedSeed = 0) {
    if (!input) return [];
    const segments = [];
    let i = 0;
    const n = input.length;
    let currentColor = null;
    let currentStyle = {};
    let obfuscated = false;
    let pendingText = "";
    const pushPending = () => {
        if (pendingText.length === 0) return;
        const classes = [];
        if (obfuscated) classes.push("mc-obfuscated");
        const style = { ...currentStyle };
        if (currentColor) style.color = currentColor;
        const segIndex = segments.length;
        let obfText = undefined;
        if (obfuscated) {
            const gen = lcg((obfuscatedSeed >>> 0) + segIndex + 1);
            const pool = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            const poolLen = pool.length;
            const arr = new Array(pendingText.length);
            for (let j = 0; j < pendingText.length; j++) {
                const ch = pendingText[j];
                const cp = ch.codePointAt(0);
                if (cp != null && isGlyphCode(cp)) {
                    arr[j] = ch;
                } else if (ch === " ") {
                    arr[j] = " ";
                } else {
                    const rnd = gen();
                    arr[j] = pool[(rnd & 0xffff) % poolLen];
                }
            }
            obfText = arr.join("");
        }
        segments.push({
            text: pendingText,
            obfText,
            style,
            classes,
        });
        pendingText = "";
    };

    while (i < n) {
        const ch = input[i];
        if (ch === "§" || ch === "\u00A7") {
            const code = i + 1 < n ? input[i + 1] : null;
            if (!code) {
                pendingText += ch;
                i++;
                continue;
            }
            pushPending();
            const lower = code.toLowerCase();
            if (Object.prototype.hasOwnProperty.call(COLOR_MAP, lower)) {
                currentColor = COLOR_MAP[lower];
                currentStyle = {};
            } else if (lower === "r") {
                currentColor = null;
                currentStyle = {};
                obfuscated = false;
            } else if (lower === "k") {
                obfuscated = true;
            } else if (lower === "l") {
                currentStyle = mergeStyles(currentStyle, STYLE_MAP["l"]);
            } else if (lower === "o") {
                currentStyle = mergeStyles(currentStyle, STYLE_MAP["o"]);
            } else if (lower === "n") {
                currentStyle = mergeStyles(currentStyle, STYLE_MAP["n"]);
            } else if (lower === "m") {
                currentStyle = mergeStyles(currentStyle, STYLE_MAP["m"]);
            }
            i += 2;
        } else {
            pendingText += ch;
            i++;
        }
    }
    pushPending();
    return segments;
}

/* 优化后的 Segment：将连续的普通文本合并为单个文本节点，只把 glyph 渲染为独立 span（显著减少 DOM 节点） */
const Segment = React.memo(function Segment({ seg, idx }) {
    const segStyle = seg.style || {};
    const className = (seg.classes || []).join(" ");
    const content = seg.obfText != null ? seg.obfText : seg.text;

    // 构造 runs：{type: 'text', text} 或 {type: 'glyph', codePoint} 或 {type: 'newline'}
    const runs = [];
    let buf = "";
    // 遍历字符串时使用 codePoint 以兼容代理对
    for (let i = 0; i < content.length; ) {
        const cp = content.codePointAt(i);
        const char = String.fromCodePoint(cp);
        
        if (char === '\n') {
            if (buf) {
                runs.push({ type: "text", text: buf });
                buf = "";
            }
            runs.push({ type: "newline" });
            i += 1;
        } else if (isGlyphCode(cp)) {
            if (buf) {
                runs.push({ type: "text", text: buf });
                buf = "";
            }
            runs.push({ type: "glyph", codePoint: cp });
            i += cp > 0xffff ? 2 : 1;
        } else {
            buf += char;
            i += cp > 0xffff ? 2 : 1;
        }
    }
    if (buf) runs.push({ type: "text", text: buf });

    return (
        <span key={idx} className={className} style={segStyle}>
            {runs.map((r, i) => {
                if (r.type === "newline") {
                    return <br key={`${idx}-br-${i}`} />;
                } else if (r.type === "text") {
                    // 连续普通文本一次性输出为字符串节点（React 会优化文本节点）
                    return <React.Fragment key={`${idx}-t-${i}`}>{r.text}</React.Fragment>;
                } else {
                    // glyph 单独渲染为带背景图的 span
                    return (
                        <span
                            key={`${idx}-g-${i}`}
                            className="mc-glyph"
                            style={getCachedGlyphStyle(r.codePoint)}
                            aria-hidden="true"
                        />
                    );
                }
            })}
        </span>
    );
});

export function MinecraftFormattedText({
                                           text = "",
                                           className = "",
                                           obfuscatedSeed = 0,
                                           style = {},
                                           ...rest
                                       }) {
    const segments = useMemo(() => parseMinecraftFormatting(text, obfuscatedSeed), [text, obfuscatedSeed]);

    return (
        <span className={`mc-formatted-text ${className || ""}`} style={style} {...rest}>
            {segments.map((seg, idx) => (
                <Segment seg={seg} idx={idx} key={idx} />
            ))}
        </span>
    );
}

export default React.memo(MinecraftFormattedText);
