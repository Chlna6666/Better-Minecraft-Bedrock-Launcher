import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { ViteImageOptimizer } from 'vite-plugin-image-optimizer';

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [
    react(),
    // --- 图片压缩配置 (修复版) ---
    ViteImageOptimizer({
      // 自动压缩 public 目录
      includePublic: true,
      //要在控制台看压缩结果就设为 true，不想看就设为 false
      logStats: true, 

      // PNG/JPEG 保持平衡配置
      png: {
        quality: 80,
      },
      jpeg: {
        quality: 75,
      },
      jpg: {
        quality: 75,
      },
      webp: {
        lossless: false,
        quality: 80,
      },

      // --- 修复 2: SVG 配置符合 SVGO v3 标准 ---
      svg: {
        multipass: true,
        plugins: [
          {
            name: 'preset-default',
            params: {
              overrides: {
                // 这里不要再配置 removeViewBox，否则会报错
                cleanupNumericValues: false,
              },
            },
          },
          // 关键：removeViewBox 现在是独立插件
          // active: false 表示“不移除 ViewBox”，防止图标变形
          {
            name: 'removeViewBox',
            active: false,
          },
        ],
      },
    }),
  ],

  build: {
    // 调大警告阈值 (4MB)
    chunkSizeWarningLimit: 4000,
    
    // --- 代码压缩配置 ---
    minify: 'terser', 
    terserOptions: {
      compress: {
        drop_console: true,  // 移除 console.log
        drop_debugger: true, // 移除 debugger
      },
      format: {
        comments: false,     // 移除注释
      },
    },

    rollupOptions: {
      // --- 忽略第三方库的 eval 警告 ---
      onwarn(warning, warn) {
        if (warning.code === 'EVAL' && warning.id && (warning.id.includes('file-type') || warning.id.includes('music-metadata'))) {
          return;
        }
        warn(warning);
      },
    },
  },

  // --- Tauri 默认开发配置 ---
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: [
        "**/src-tauri/**",
        "**/target/**",
        "**/.git/**"
      ],
    },
  },
}));