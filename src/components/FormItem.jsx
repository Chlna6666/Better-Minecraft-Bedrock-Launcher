import React from 'react';
import './FormItem.css';


function FormItem({ label, required = false, description, children }) {
    return (
        <div className="ui-form-item">
            {label ? (
                <label className="ui-form-label">{label} {required ? <span className="ui-form-required">*</span> : null}</label>
            ) : null}
            <div className="ui-form-control">{children}</div>
            {description ? <div className="ui-form-desc">{description}</div> : null}
        </div>
    );
}


export default FormItem;