import React from 'react';

interface PageContainerProps {
    title: string;
    children?: React.ReactNode;
}

export const PageContainer: React.FC<PageContainerProps> = ({ title, children }) => {
    return (
        <div
            className="page-content glass bm-anim-page-in"
            style={{
                marginTop: '100px',
                marginLeft: 'auto',
                marginRight: 'auto',
                width: '90%',
                maxWidth: '1000px',
                height: 'calc(100vh - 120px)',
                borderRadius: '24px',
                padding: '40px',
                boxSizing: 'border-box',
                overflowY: 'auto',
                position: 'relative',
                zIndex: 10
            }}
        >
            <h1 style={{ marginBottom: '20px' }}>{title}</h1>
            {children}
        </div>
    );
};
