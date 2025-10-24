import React from 'react';
import './IconButton.css';


function IconButton({ icon, title, size = 'md', className = '', ...props }) {
    return (
        <button className={`ui-icon-btn ui-icon-btn-${size} ${className}`} title={title} {...props}>
            {icon}
        </button>
    );
}


export default IconButton;