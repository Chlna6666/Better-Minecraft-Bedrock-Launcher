export function formatBytes(bytes, options = {}) {
    const { decimals = 2, defaultBytes = 0, defaultText = "未知" } = options;

    let b;
    if (typeof bytes === "number" && !Number.isNaN(bytes) && isFinite(bytes)) {
        b = Math.max(0, Math.floor(bytes));
    } else if (typeof defaultBytes === "number" && isFinite(defaultBytes)) {
        b = Math.max(0, Math.floor(defaultBytes));
    } else {
        return defaultText;
    }

    if (b === 0) return "0 B";

    const k = 1024;
    const dm = Math.max(0, decimals);
    const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
    const i = Math.floor(Math.log(b) / Math.log(k));
    const value = parseFloat((b / Math.pow(k, i)).toFixed(dm));
    return `${value} ${sizes[i]}`;
}


export function parseSizeToBytes(sizeStr, fallback = 0) {
    if (!sizeStr || typeof sizeStr !== "string") return fallback;
    const m = sizeStr.trim().match(/^([\d.,]+)\s*(b|kb|mb|gb|tb|pb)?$/i);
    if (!m) return fallback;
    const num = parseFloat(m[1].replace(',', '.'));
    if (Number.isNaN(num)) return fallback;
    const unit = (m[2] || "b").toLowerCase();
    const map = { b: 1, kb: 1024, mb: 1024 ** 2, gb: 1024 ** 3, tb: 1024 ** 4, pb: 1024 ** 5 };
    return Math.round(num * (map[unit] || 1));
}
