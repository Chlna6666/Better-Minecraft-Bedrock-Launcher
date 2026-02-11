import React, { memo, useEffect, useState } from 'react';

interface BackgroundLayerProps {
    url: string;
    opacity?: number;
    blur?: number;
}

const BackgroundLayer = memo(({ url, opacity = 1, blur = 0 }: BackgroundLayerProps) => {
    const [mounted, setMounted] = useState(false);

    useEffect(() => {
        const id = requestAnimationFrame(() => setMounted(true));
        return () => cancelAnimationFrame(id);
    }, []);

    // 即使没有 URL，也渲染一个带背景色的层，方便调试
    // if (!url) return null; // [修改] 注释掉这行，防止无图时彻底消失

    return (
        <div
            className="bm-bg-layer"
            style={{
                position: 'fixed',
                top: 0,
                left: 0,
                width: '100vw',
                height: '100vh',
                // [修改] zIndex 从 -10 改为 0。
                // 因为它是 App 中的第一个组件，自然会在最底层。负值可能被 #root 背景遮挡。
                zIndex: 0,

                // [修改] 增加兜底背景色 (深灰色)，如果图片没加载出来，至少能看到这个颜色
                backgroundColor: '#1a1a1a',

                backgroundImage: url ? `url("${url}")` : 'none',
                backgroundSize: 'cover',
                backgroundPosition: 'center',
                backgroundRepeat: 'no-repeat',

                opacity: mounted ? opacity : 0,
                transition: 'opacity 800ms cubic-bezier(0.2, 0, 0, 1)',

                // 性能优化保持不变
                transform: 'translateZ(0)',
                willChange: 'transform',
                filter: blur > 0 ? `blur(${blur}px)` : 'none',
                pointerEvents: 'none',
            }}
        />
    );
}, (prev, next) => prev.url === next.url && prev.blur === next.blur);

export default BackgroundLayer;
