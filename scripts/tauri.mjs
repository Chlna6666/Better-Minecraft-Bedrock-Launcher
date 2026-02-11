import { spawnSync } from "node:child_process";
import fs from "node:fs";

function ensureSevenZipOnPath(env) {
  if (process.platform !== "win32") return env;
  const sevenZipDir = "C:\\Program Files\\7-Zip";
  const pathVar = String(env.PATH || env.Path || "");
  const parts = pathVar.split(";");
  const has = parts.some((p) => p.trim().toLowerCase() === sevenZipDir.toLowerCase());
  if (!has) {
    env.PATH = `${sevenZipDir};${pathVar}`;
  }
  return env;
}

function ensureLibclangOnWindows(env) {
  if (process.platform !== "win32") return env;
  if (env.LIBCLANG_PATH) return env;

  const candidates = [
    "C:\\Program Files\\LLVM\\bin",
    "C:\\Program Files (x86)\\LLVM\\bin",
  ];

  for (const dir of candidates) {
    if (fs.existsSync(`${dir}\\libclang.dll`) || fs.existsSync(`${dir}\\clang.dll`)) {
      env.LIBCLANG_PATH = dir;
      return env;
    }
  }

  return env;
}

function applyThunkDownloadProxy(env) {
  if (process.platform !== "win32") return env;

  const proxy = String(env.BMCBL_GITHUB_PROXY || "").trim();
  if (!proxy) return env;

  const prefix = proxy.endsWith("/") ? proxy : `${proxy}/`;

  if (!env.VC_LTL_URL) {
    env.VC_LTL_URL = `${prefix}https://github.com/Chuyu-Team/VC-LTL5/releases/download/v5.2.2/VC-LTL-Binary.7z`;
  }
  if (!env.YY_THUNKS_URL) {
    env.YY_THUNKS_URL = `${prefix}https://github.com/Chuyu-Team/YY-Thunks/releases/download/v1.1.7/YY-Thunks-Objs.zip`;
  }

  return env;
}

const args = process.argv.slice(2);
const env = ensureLibclangOnWindows(
  applyThunkDownloadProxy(ensureSevenZipOnPath({ ...process.env })),
);

const result = spawnSync("tauri", args, {
  stdio: "inherit",
  env,
  shell: true,
});

process.exit(result.status ?? 1);
