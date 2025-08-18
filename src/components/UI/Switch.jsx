import React from "react";
import "./Switch.css";

function Switch({ id, checked, onChange }) {
    return (
        <div className="switch">
            <input
                id={id}
                className="switch-input"
                type="checkbox"
                checked={checked}
                onChange={onChange}
            />
            <span className="slider round"></span>
        </div>
    );
}

export default Switch;
