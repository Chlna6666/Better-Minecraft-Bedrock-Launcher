import React from 'react';
import './TextArea.css';


function TextArea(props) {
    return <textarea className="ui-textarea" {...props} />;
}


export default TextArea;