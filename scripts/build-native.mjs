/**
 * 构建插件 native cdylib → lib/。
 * ------------------------------------------------------------------
 * 宿主按 `lib/Windows.dll` / `lib/macOS.dylib` / `lib/Linux.so` 加载插件动态库
 * （见 src-tauri/src/platform/native_plugins/core.rs）。本脚本在当前平台执行
 * `cargo build --release` 并把产物改名为宿主约定的文件名放进 lib/。
 *
 * 用法：node scripts/build-native.mjs
 * CI（三平台矩阵）不用本脚本，直接 cargo build 后按 outname 落位。
 */
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const nativeDir = path.join(repoRoot, 'native');

if (!fs.existsSync(path.join(nativeDir, 'Cargo.toml'))) {
  console.error('native/Cargo.toml 不存在：本插件未启用 native 层。');
  process.exit(1);
}

const result = spawnSync('cargo', ['build', '--release'], { stdio: 'inherit', cwd: nativeDir });
if (result.error || result.status !== 0) {
  console.error('cargo build --release 失败。');
  process.exit(result.status ?? 1);
}

const outName =
  process.platform === 'win32' ? 'Windows.dll' : process.platform === 'darwin' ? 'macOS.dylib' : 'Linux.so';
const libName =
  process.platform === 'win32'
    ? 'otools_easel_native.dll'
    : process.platform === 'darwin'
      ? 'libotools_easel_native.dylib'
      : 'libotools_easel_native.so';

const built = path.join(nativeDir, 'target', 'release', libName);
if (!fs.existsSync(built)) {
  console.error(`未找到构建产物：${built}`);
  process.exit(1);
}

const libDir = path.join(repoRoot, 'lib');
fs.mkdirSync(libDir, { recursive: true });
fs.copyFileSync(built, path.join(libDir, outName));
const size = (fs.statSync(path.join(libDir, outName)).size / 1048576).toFixed(1);
console.log(`native 就绪：lib/${outName}（${size} MB）`);
console.log('用 scripts/deploy-to-otools.ps1 同步到 otools 仓库即可在宿主内启用流式对话。');
