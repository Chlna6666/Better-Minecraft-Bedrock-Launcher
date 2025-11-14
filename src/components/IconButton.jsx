import React from 'react';
import './IconButton.css';

function IconButton({
                        icon,
                        title,
                        size = 'md',          // 支持 'sm', 'md', 'lg', 或 自定义数字 (number)
                        color = 'currentColor', // 支持传入颜色，默认继承文字颜色
                        className = '',
                        style = {},
                        ...props
                    }) {
    // 如果 size 是数字，则动态设置宽高
    const sizeStyle =
        typeof size === 'number'
            ? { width: size, height: size }
            : null;

    return (
        <button
            className={`ui-icon-btn ui-icon-btn-${typeof size === 'string' ? size : 'md'} ${className}`}
            title={title}
            style={{ color, ...sizeStyle, ...style }}
            {...props}
        >
            {icon}
        </button>
    );
}

export default IconButton;
