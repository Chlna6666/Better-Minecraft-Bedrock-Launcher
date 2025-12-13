import React from 'react';
import { motion } from 'framer-motion';

interface PageContainerProps {
    title: string;
    children?: React.ReactNode;
}

export const PageContainer: React.FC<PageContainerProps> = ({ title, children }) => {
    return (
        <motion.div
            initial={{ opacity: 0, y: 20, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -20, filter: 'blur(10px)' }}
            transition={{ duration: 0.4, ease: "easeOut" }}
            className="page-content glass"
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
        </motion.div>
    );
};