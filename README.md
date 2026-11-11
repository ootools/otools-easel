# otools-easel · Easel 内容工坊

OTools 独立插件：把 [ZJU-REAL/Easel](https://github.com/ZJU-REAL/Easel) 的「画像驱动社媒内容工作流」移植为 OTools 插件。

当前进度：

- **账号画像**：定位 / 风格 / 受众 / 平台 / 偏好红线 / 长期记忆 六维管理，对话时作为上下文注入（移植 Easel 画像体系与 `_template`）。
- **技能库**：内置 Easel 118 个技能的 SKILL.md 与 references（发现 / 策划 / 文字创作等），可浏览、可检索、可一键带入对话。
- **对话工作台**：多会话管理；系统提示按 Easel prompt 栈组装（SOUL → AGENTS → 运行时说明 → 画像/技能）。
- **统一 AI 和 Agent 执行**：前端通过宿主 SDK 使用普通聊天与 Codex／ACP。插件原生层只准备提示、资产与工作台数据，不再启动本地 Codex 或自行完成推理。
- **模型与推理程度**：输入框下随时切换，偏好按 Easel 插件来源与 UUID 保存；发送等待最新保存，运行中的请求保留启动快照。Codex 宿主配置使用同一选择；ACP／原生配置沿用引擎自身设置。媒体通道独立选择模型，连接凭据由宿主获取。
- **任务与审批**：宿主统一提供工作区、后台任务、取消、事件恢复、工具授权及审批；导入技能资产保留用户已有修改，关闭视图不等于终止后台任务。画像增强和工具型调用也通过统一任务服务执行。
- **验证边界**：需要提供统一 Agent SDK 的新版 Otools。模型调用可能收费；当前只有回归夹具和 CLI 握手验证，不代表真实推理或真实 ACP 任务已验收。

## 上游与许可

- 上游项目：[ZJU-REAL/Easel](https://github.com/ZJU-REAL/Easel)（Apache-2.0）。本仓库捆绑的 `web/public/easel/`（agents/workspace、skills、profiles/_template）来自上游，遵循 [LICENSE](./LICENSE)（Apache-2.0）并保留其声明。
- 插件脚手架来自 [ootools/otools-plugin-template](https://github.com/ootools/otools-plugin-template)（`scripts/`、`.github/workflows/`、清单结构）。

## 开发

```bash
# 前端（web/，Vite + Vue3）
cd web && npm install && npm run dev   # http://127.0.0.1:6493（plugin.json devUrl）

# native cdylib（流式对话；宿主按 lib/<平台> 文件名加载）
node scripts/build-native.mjs          # 产物 lib/macOS.dylib 等
cd native && cargo test                # 协议与 SSE 端到端测试

# 构建插件 dist/（自动：安装依赖 → vite build --outDir ../dist）
node scripts/build-web.mjs

# 本地预览 dist/
node scripts/serve-dist.mjs [端口]

# 同步到 OTools 主仓库本地试跑（自动带上 dist/ 与 lib/）
pwsh -File scripts/deploy-to-otools.ps1 -OtoolsRoot /path/to/OTools
```

## 目录结构

```
otools-easel/
├── plugin.json              # 插件清单(native.enabled)
├── logo.svg
├── native/                  # Rust cdylib:流式对话(OpenAI 兼容 + Azure 线协议、SSE、中止)
├── lib/                     # 构建产物:宿主约定的动态库文件名(不入库)
├── scripts/                 # 模板构建/打包/发布脚本(build-web 已支持 Vite,新增 build-native)
├── .github/workflows/       # push 构建(三平台 native 矩阵 + pack&verify)/ tag 发布 CI
└── web/
    ├── public/easel/        # 上游资产:agents/workspace、skills、profiles/_template
    └── src/
        ├── lib/             # 宿主桥、AI 客户端(执行引擎接缝)、prompt 栈、画像/会话/技能
        └── components/      # 对话 / 画像 / 技能库
```
