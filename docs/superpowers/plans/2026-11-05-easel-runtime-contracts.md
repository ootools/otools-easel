# Easel 工作台契约、环境与媒体接入实施计划

> 执行方式：延续用户已确定的 Rust CMD 忠实移植，当前会话直接实施；以测试验证每项交付，不自动提交或发布。

**Goal:** 接通原版 React 内容库、素材上传、环境/模型设置，并修复这些页面依赖的 Rust 响应契约。

**Architecture:** React 继续构建为插件 `dist/index.html`，不新增 localhost 服务、不跳转。原版 `lib/api.ts` 的函数名与页面交互保留，传输适配收敛到桥层。Rust native 路由保持上游请求和响应形状；仅 Codex 引擎与宿主 ai-switch 接入是用户明确指定的产品差异。

**Spec:** 用户当前会话中的忠实移植要求；`reference/app.py`、`reference/easel/persona.py` 与原版 React API/组件是契约真相源。

## Global Constraints
- 不裁剪或重新设计页面与工作流；不以同名路由、构建成功替代功能验收。
- 桌面与 WEB 远程使用相同 CMD 数据接口；媒体使用宿主已有文件访问能力，必要时分块桥读取。
- 测试只操作新建临时目录与本地 mock 服务，不读取/写入真实密钥，不调用付费媒体，不执行自动发布。
- 当前未提交的忠实移植改动全部保留，不做 reset/clean；本轮在新本地分支继续，不重建脏工作区。
- 未完成的发布/账号/归因 API 仍明确失败，文档不得写成全部可用。

## Review Focus
- 顶层数组与对象、snake_case 与 camelCase 的不匹配不能被 TypeScript 类型断言掩盖。
- 路径百分号编码、Windows 分隔符、目录穿越、符号链接必须在原生端校验。
- 同名上传不得覆盖；附件引用必须绑定会话；文件过大或操作失败需清理本次临时文件。
- HTML 预览保留 sandbox，不引入宿主密钥或跨会话文件；视频/音频在 WEB 远程使用宿主文件服务而非本机地址。
- 安装任务仅允许固定配方，限制日志、运行时与并发，结束后重新检测，失败如实返回。

## Task 1: 内容库/画像/上传契约
**Files:** `native/src/router.rs`, 新建 `native/src/files.rs`, `native/tests/workbench_contracts.rs`, `native/tests/support/mod.rs`。
**Interfaces:** 保留 `/api/personas` 顶层数组；`/api/persona/{name}/files` 返回 `{name,files:[{filename,content}]}`；`/api/outputs` 返回嵌套节点数组；`/api/output/{path}` 返回 `{path,content,kind,isBinary}`；上传返回 `{ok,files:[{id,name,path}]}`。
- [ ] 先写测试并确认旧实现因响应不匹配或缺路由失败。
- [ ] 修复画像与技能详情的契约、顺序、名称和安全校验。
- [ ] 实现项目树、展示元数据、文本读取、安全删除、媒体元数据与分块读取。
- [ ] 实现普通/本地上传、同名保留、50MiB 普通上传上限与会话引用校验。
- [ ] 运行 Rust 套件，记录原有 warnings/未覆盖功能。

## Task 2: 环境与模型设置
**Files:** 新建 `native/src/settings.rs`, `native/src/environment.rs`, `native/src/process.rs`; 修改 `native/src/router.rs`, `harness.rs`, `chat_routes.rs`；测试 `native/tests/settings_contracts.rs`。
**Interfaces:** 按原版 `/api/settings/bootstrap`, `/api/settings/harness`, `/api/settings/models*`, `/api/settings/proxy`, `/api/env/tools|install|job` 字段返回。保留模型通道表与环境安装任务页面；Codex 之外 Agent 不声称可执行。
- [ ] 写 bootstrap/harness 保存与校验、模型脱敏、代理校验、未知安装 ID 的失败测试。
- [ ] 实现统一配置读取，优先宿主 ai-switch，保留原版设置交互；不静默更改真实用户配置。
- [ ] Rust 实现固定工具配方的检测、受控安装进程、stdout/stderr 并行抽干、超时杀进程与任务快照。
- [ ] 使用临时 mock 可执行程序/HTTP 服务验证，不实际安装模型或依赖。

## Task 3: React 桥和媒体访问
**Files:** 新建 `web/frontend/src/lib/nativeBridge.ts`, `media.ts`, 媒体 hook；修改 `api.ts`, `App.tsx`, `ArtifactGallery.tsx`, `OutputsPage.tsx` 等媒体消费点；新建 `web/frontend/tests/*.test.ts`。
**Interfaces:** API 导出保持原版；媒体的逻辑路径保留给产物解析，渲染边界异步换成验证后的宿主文件 URL/Blob；bootstrap 与 upload limits 不再绕过桥。
- [ ] 先验证桥调用的 method/path/body、FormData 重复文件与取消/错误语义。
- [ ] 将漏掉的 fetch 与媒体 `<img>/<video>/<iframe>/下载` 换到边界适配，保持 UI。
- [ ] 保留 HTML sandbox，解析并适配本地相对资源。
- [ ] 运行 Node 前端测试、TypeScript 和生产构建。

## Task 4: 验证与记录
- [ ] Rust 全测试 + 前端全测试 + build-web/build-native + 插件打包验证。
- [ ] 在真实浏览器中仅测试临时桥接环境，验证内容库/设置/媒体交互，不将 mock 冒烟称为真实宿主验收。
- [ ] 发起一次独立只读审查；若 provider 不可用，明确说明限制。
- [ ] 修正本轮可证明的缺陷，记录尚未验证的桌面/远程宿主链路与剩余 API，不做成功夸大。
