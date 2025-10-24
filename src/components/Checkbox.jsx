import React from 'react';
import './Checkbox.css';


function Checkbox({ id, checked = false, onChange, label, disabled = false }) {
    return (
        <label className={`ui-checkbox ${disabled ? 'disabled' : ''}`} htmlFor={id}>
            <input id={id} type="checkbox" checked={checked} onChange={(e) => onChange && onChange(e.target.checked)} disabled={disabled} />
            <span className="ui-checkbox-box" aria-hidden="true"></span>
            {label ? <span className="ui-checkbox-label">{label}</span> : null}
        </label>
    );
}


export default Checkbox;