import React, { useEffect } from 'react';
import ReactDOM from 'react-dom';
import './Modal.css';


function Modal({ open, title, children, onClose, footer, width = '520px' }) {
    useEffect(() => {
        function onKey(e) { if (e.key === 'Escape') onClose && onClose(); }
        if (open) document.addEventListener('keydown', onKey);
        return () => document.removeEventListener('keydown', onKey);
    }, [open, onClose]);


    if (!open) return null;
    return ReactDOM.createPortal(
        <div className="ui-modal-overlay" role="dialog" aria-modal="true">
            <div className="ui-modal" style={{ width }}>
                <div className="ui-modal-header">
                    <h3 className="ui-modal-title">{title}</h3>
                    <button className="ui-modal-close" onClick={onClose} aria-label="Close">✕</button>
                </div>
                <div className="ui-modal-body">{children}</div>
                {footer ? <div className="ui-modal-footer">{footer}</div> : null}
            </div>
        </div>,
        document.body
    );
}


export default Modal;