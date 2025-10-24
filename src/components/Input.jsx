import React, { forwardRef } from 'react';
import './Input.css';

/**
 * Props:
 *  - prefix, suffix: ReactNode
 *  - className: applied to wrapper (.ui-input-wrapper)
 *  - inputClassName: applied to inner <input> (.ui-input)
 *  - size: 'sm' | 'md' | 'lg' (default 'md')
 *  - fullWidth: boolean -> wrapper width:100%
 *  - style: inline style for wrapper
 *  - inputStyle: inline style for inner input
 *  - ...props forwarded to <input>
 */
const Input = forwardRef(function Input({
                                            prefix,
                                            suffix,
                                            className = '',
                                            inputClassName = '',
                                            size = 'md',
                                            fullWidth = false,
                                            style = {},
                                            inputStyle = {},
                                            ...props
                                        }, ref) {
    const wrapperCls = [
        'ui-input-wrapper',
        className,
        `ui-input-size-${size}`,
        fullWidth ? 'ui-input-full' : ''
    ].filter(Boolean).join(' ');

    const innerCls = ['ui-input', inputClassName].filter(Boolean).join(' ');

    return (
        <div className={wrapperCls} style={style}>
            {prefix ? <span className="ui-input-prefix">{prefix}</span> : null}
            <input
                ref={ref}
                className={innerCls}
                style={inputStyle}
                {...props}
            />
            {suffix ? <span className="ui-input-suffix">{suffix}</span> : null}
        </div>
    );
});

export default Input;
