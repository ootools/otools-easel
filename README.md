# otools-easel · Easel 内容工坊

OTools 独立插件：把 [ZJU-REAL/Easel](https://github.com/ZJU-REAL/Easel) 的「画像驱动社媒内容工作流」移植为 OTools 插件。

当前进度：

- **账号画像**：定位 / 风格 / 受众 / 平台 / 偏好红线 / 长期记忆 六维管理，对话时作为上下文注入（移植 Easel 画像体系与 `_template`）。
- **技能库**：内置 Easel 118 个技能的 SKILL.md 与 references（发现 / 策划 / 文字创作等），可浏览、可检索、可一键带入对话。
- **对话工作台**：多会话管理；系统提示按 Easel prompt 栈组装（SOUL → AGENTS → 运行时说明 → 画像/技能）。
- **流式对话（native）**：`native/` Rust cdylib 直连宿主 AI 配置的 OpenAI 兼容端点（含 Azure 部署线协议），真多轮 messages + SSE 逐 token 流式 + 可中止；推理模型 reasoning 增量单独折叠展示。构建：`node scripts/build-native.mjs`（CI 三平台矩阵自动产出）。
- **执行引擎接缝**：`web/src/lib/agentClient.ts` 暴露统一 AgentClient 接口——native 不可用时自动回退仅聊天模式（宿主 `otools_ai_generate_text` 单轮接口）。后续切换到 OTools 宿主 Agent 服务（codex app-server）后即可运行技能脚本与文件产物。

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
