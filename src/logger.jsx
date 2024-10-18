// src/logger.js
import { invoke } from '@tauri-apps/api/core';

/**
 * Logs a message with the specified level.
 *
 * @param {string} level - The log level ('Info', 'Warning', 'Error', 'Debug').
 * @param {string} message - The message to log.
 */
export function logMessage(level, message) {
    invoke('log', { level, message })
        .then(response => {
            console.log('Log successful:', response);
        })
        .catch(error => {
            console.error('Log failed:', error);
        });
}
