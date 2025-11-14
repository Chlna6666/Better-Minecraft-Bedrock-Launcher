/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./index.html",
    "./src/**/*.{js,jsx,ts,tsx,html}"
  ],
  theme: {
    extend: {
      animation: {
        progressIndeterminate: "progressIndeterminate 1.2s linear infinite",
      },
      keyframes: {
        progressIndeterminate: {
          "0%": { transform: "translateX(-150%)" },
          "100%": { transform: "translateX(300%)" },
        },
      },
      // 如需自定义颜色/字体在这里扩展
    },
  },
  plugins: [],
};
