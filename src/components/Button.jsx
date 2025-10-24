import React, { forwardRef } from 'react';
import './Button.css';

const Spinner = () => (
    <svg className="ui-btn-spinner" viewBox="0 0 50 50" width="16" height="16" aria-hidden="true">
        <circle cx="25" cy="25" r="20" fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" strokeDasharray="31.4 31.4" transform="rotate(-90 25 25)" />
    </svg>
);

const Button = forwardRef(function Button({
                                              id,
                                              type = 'button',
                                              children,
                                              variant = 'primary',
                                              size = 'md',
                                              className = '',
                                              onClick,
                                              disabled = false,
                                              loading = false,
                                              iconLeft,
                                              iconRight,
                                              fullWidth = false,
                                              title,
                                              ariaLabel,
                                              fontSize,      // new
                                              padding,       // new
                                              bg,            // new background override
                                              background,    // alias for bg
                                              color,         // text/icon color override
                                              borderColor,   // border override
                                              ...props
                                          }, ref) {

    const isDisabled = disabled || loading;

    const cls = [
        'ui-btn',
        `ui-btn-${variant}`,
        `ui-btn-${size}`,
        fullWidth ? 'ui-btn-block' : '',
        className
    ].filter(Boolean).join(' ');

    // compute inline style overrides
    const inlineStyle = {};
    const resolvedBg = background || bg;
    if (typeof fontSize !== 'undefined' && fontSize !== null) {
        inlineStyle.fontSize = typeof fontSize === 'number' ? `${fontSize}px` : fontSize;
    }
    if (padding) inlineStyle.padding = padding;

    if (resolvedBg) inlineStyle.background = resolvedBg;
    if (color) inlineStyle.color = color;
    if (borderColor) inlineStyle.borderColor = borderColor;

    // Accessibility attributes
    const accessibility = {
        'aria-disabled': isDisabled || undefined,
        'aria-busy': loading || undefined,
    };

    return (
        <button
            id={id}
            ref={ref}
            type={type}
            className={cls}
            onClick={isDisabled ? undefined : onClick}
            disabled={isDisabled}
            title={title}
            aria-label={ariaLabel}
            style={inlineStyle}
            {...accessibility}
            {...props}
        >
            {loading ? <span className="ui-btn-spinner-wrap"><Spinner /></span> : null}
            {!loading && iconLeft ? <span className="ui-btn-icon-left">{iconLeft}</span> : null}
            <span className="ui-btn-content">{children}</span>
            {!loading && iconRight ? <span className="ui-btn-icon-right">{iconRight}</span> : null}
        </button>
    );
});

export default Button;
