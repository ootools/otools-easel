import { nativeRequest as request } from './nativeBridge.ts';
import { hostMediaApi, type MediaChannel } from './hostMedia.ts';
import { hostAiApi } from './hostAi.ts';
import { hostAgentApi, taskController, executeHostTask, startAgentTask, prepareTask, applyAgentPage, cancelHostSession, respondHostQuestion, hostQuestionStatuses, type PreparationInput } from './hostAgent.ts';
import { loadHostTasks, loadSessions, saveHostTask } from './store.ts';
import { uploadViaChunks } from './uploads.ts';

export interface StatusResponse {
  gateway: boolean;
  transport?: string;
  runtime?: string;
  skills: SkillItem[];
  personas: PersonaItem[];
}

export interface HarnessSettingsResponse {
  runtime: string;
  models: Record<string, string>;
  sandbox: string;
  codex_auto_approve: boolean;
  locked: boolean;
  agents: { id: string; label: string; protocol: string; installed: boolean; install_hint: string }[];
}

export async function fetchHarnessSettings(): Promise<HarnessSettingsResponse> {
  const settings=await request<HarnessSettingsResponse>('/api/settings/harness');
  const providers=await hostAgentApi().providers();
  return {...settings,agents:[{id:'api',label:'仅聊天模式',protocol:'Otools AI',installed:true,install_hint:''},...providers.map(provider=>({id:provider.id,label:provider.name,protocol:provider.adapter,installed:provider.available,install_hint:'在 Otools Agent 管理中安装或配置'}))]};
}

export async function saveHarnessSettings(runtime: string, model: string, sandbox: string, codexAutoApprove?: boolean): Promise<HarnessSettingsResponse> {
  await request('/api/settings/harness', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ runtime, model, sandbox, codex_auto_approve: codexAutoApprove }),
  });
  return fetchHarnessSettings();
}

export interface PersonaItem {
  name: string;
  description: string;
}

export interface PersonaDetail {
  name: string;
  content: string;
}

export interface SkillItem {
  name: string;
  description: string;
  layer: string;
  needsApi: boolean;
  apiConfigured: boolean;
  mediaChannel?: string;
}

export interface ApiKeySpec {
  env: string;
  label: string;
  required: boolean;
  secret: boolean;
  configured: boolean;
  masked: string;   // 脱敏值或非 secret 明文
  choices: string[];
}

export interface ApiProviderSpec {
  id: string;
  name: string;
  keys: ApiKeySpec[];
}

export interface ApiSpec {
  label: string;
  settings: ApiKeySpec[];
  providers: ApiProviderSpec[];
  channel?: string;
  managedByHost?: boolean;
}

export interface SkillDetail {
  name: string;
  layer: string;
  description: string;
  body: string;
  needsApi: boolean;
  apiConfigured: boolean;
  apiSpec: ApiSpec | null;
}

export type FileKind = 'text' | 'image' | 'video' | 'audio' | 'binary';

/** 顶层项目目录的 .easel.json 展示头（供内容库富展示）。 */
export interface OutputMeta {
  title?: string;
  summary?: string;
  platform?: string;
  kind?: string;             // article|xhs-note|video|cards|poster|audio|other
  status?: string;           // draft|ready|published
  tags?: string[];
  cover?: string;            // outputs 下相对路径（后端已解析存在性）
  deliverables?: string[];   // 成品文件名（项目根相对）
  deliverablePaths?: string[];  // 成品的 outputs 相对路径（后端已解析存在性）
}

/** 产物树节点：文件或目录（目录带 children，可无限嵌套点开）。 */
export interface OutputNode {
  name: string;
  type: 'dir' | 'file';
  path: string;              // outputs 下的相对路径，如 "short-drama/监控诡影/episodes/ep01"
  mtime?: number;
  kind?: FileKind;           // 仅 file
  size?: number;             // 仅 file
  children?: OutputNode[];   // 仅 dir
  fileCount?: number;        // 仅 dir：递归文件数
  meta?: OutputMeta;         // 仅顶层项目 dir
}

/** @deprecated 用 OutputNode（type==='file'）。保留别名减少改动面。 */
export type OutputFile = OutputNode;

export interface OutputContent {
  path: string;
  content: string;
  kind: FileKind;
  isBinary: boolean;
}

export interface SkillResponse {
  response: string;
}

export interface ProfileBuildResponse {
  created: boolean;
  name: string;
  async?: boolean;
  status?: string;
  log?: string;
  turnId?:string;
  preparation?:PreparationInput;
}

export interface ProfileBuildStatus {
  state: 'pending' | 'running' | 'done' | 'failed' | 'unknown';
  log: string;
  runId?:string; sessionId?:string; turnId?:string;
}

export interface BootstrapStatus {
  modelConfigured: boolean;
  authVerified: boolean;
  envKeyOk: boolean;
  routeOk: boolean;
  routeDetail: string;
  gatewayUp: boolean;
  runtime?: string;
  installWarnings: unknown[];
}

export async function fetchSettingsBootstrap(): Promise<BootstrapStatus> {
  const boot=await request<BootstrapStatus>('/api/settings/bootstrap');
  const configured=boot.runtime==='api' ? (await hostAiApi().capabilities()).configured : (await hostAgentApi().providers()).some(provider=>provider.id===boot.runtime && provider.available);
  return {...boot,modelConfigured:configured,routeOk:configured,gatewayUp:configured};
}

export function fetchUploadLimits(): Promise<{ ok: boolean; max_mb: number }> {
  return request('/api/upload/limits');
}

export async function fetchStatus(): Promise<StatusResponse> {
  const status=await request<StatusResponse>('/api/status');
  try {status.gateway=(await fetchSettingsBootstrap()).modelConfigured;} catch {status.gateway=false;}
  return status;
}

export function fetchPersonas(): Promise<PersonaItem[]> {
  return request<PersonaItem[]>('/api/personas');
}

export function fetchPersonaDetail(name: string): Promise<PersonaDetail> {
  return request<PersonaDetail>(`/api/persona/${encodeURIComponent(name)}`);
}

export interface PersonaFile {
  filename: string;
  content: string;
}

export function fetchPersonaFiles(name: string): Promise<{ name: string; files: PersonaFile[] }> {
  return request(`/api/persona/${encodeURIComponent(name)}/files`);
}

export function savePersonaFile(name: string, filename: string, content: string): Promise<{ ok: boolean }> {
  return request(`/api/persona/${encodeURIComponent(name)}/file`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ filename, content }),
  });
}

export function deletePersona(name: string): Promise<{ ok: boolean; deleted: string }> {
  return request(`/api/persona/${encodeURIComponent(name)}`, { method: 'DELETE' });
}

// ---- 热点雷达 ----
import { parseTrends, parseTrendSources, parseTrendManagement } from './trendValidation.ts';

async function trendRequest(url: string, options: RequestInit = {}, timeout = 10000): Promise<unknown> {
  // 流式读取、4MiB 上限与容错 JSON 解析已下沉到 Rust 端。
  return request<unknown>(url, { ...options, signal: options.signal ?? AbortSignal.timeout(timeout) });
}

export interface TrendItem {
  id: string; title: string; hot: string; url: string; published_at: string | null;
  heat: { value: number | null; text: string; unit: string };
}
export interface TrendGroup {
  platform: string; label: string; items: TrendItem[]; ok: boolean;
  status: 'fresh' | 'stale' | 'error'; stale: boolean; fetched_at: string | null;
  last_attempt_at: number; error: { code: string; message: string } | null;
}
export interface TrendSource {
  id: string; label: string; type: 'python' | 'http' | 'rss'; default: boolean; refresh_seconds: number;
}
export function fetchTrendSources() {
  return trendRequest('/api/trends/sources').then(parseTrendSources);
}
export function fetchTrends(platforms?: string, limit = 12) {
  return trendRequest(`/api/trends?limit=${limit}${platforms === undefined ? '' : `&platforms=${encodeURIComponent(platforms)}`}`, {}, 130000).then(parseTrends);
}
export interface TrendManagedSource {
  id: string; label: string; type: TrendSource['type'] | 'unknown'; enabled: boolean; default: boolean; builtin: boolean;
}
export function fetchTrendManagement() {
  return trendRequest('/api/trends/manage').then(parseTrendManagement);
}
export async function updateTrendSource(value: Record<string, unknown>): Promise<{ id: string }> {
  const result = await trendRequest('/api/trends/manage', {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(value),
  }, 15000);
  if (!result || typeof result !== 'object' || !('ok' in result) || result.ok !== true || !('id' in result) || typeof result.id !== 'string' || !/^[a-z0-9][a-z0-9_-]{0,63}$/.test(result.id)) throw new Error('保存结果异常，请刷新数据源列表确认。');
  return { id: result.id };
}

// ---- 内容排期 ----
export interface ScheduleItem {
  id: string; title: string; date: string; platform: string;
  time: string; status: string; note: string;
  kind?: string;          // content（内容/发布）| event（平台活动/节日/特殊日期）
  url?: string;           // 已发布链接
  source?: string;        // manual | publish-page | chat | scheduler
  event_type?: string;    // event 专属：节日/电商/平台活动/行业
  end_date?: string;      // event 专属：活动区间结束日
}
export type ScheduleInput = Omit<ScheduleItem, 'id'>;
export interface ScheduleContext {
  today?: string; window_days?: number; published_recent?: number;
  per_platform?: { platform: string; recent_count: number; last_date: string | null; days_since_last: number | null }[];
  upcoming_schedules?: ScheduleItem[];
  upcoming_events?: ScheduleItem[];
  suggestions?: string[];
}
export function fetchSchedule(): Promise<ScheduleItem[]> {
  return request('/api/schedule');
}
export function fetchScheduleContext(days = 14): Promise<ScheduleContext> {
  return request(`/api/schedule/context?days=${days}`);
}
export function createSchedule(item: ScheduleInput): Promise<ScheduleItem> {
  return request('/api/schedule', {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(item),
  });
}
export function updateSchedule(id: string, item: ScheduleInput): Promise<ScheduleItem> {
  return request(`/api/schedule/${encodeURIComponent(id)}`, {
    method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(item),
  });
}
export function deleteSchedule(id: string): Promise<{ ok: boolean }> {
  return request(`/api/schedule/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

// ---- 选题库 ----
export interface Idea {
  id: string; title: string; note: string; source: string; status: string; created: number;
}
export type IdeaInput = { title: string; note?: string; source?: string; status?: string };
export function fetchIdeas(): Promise<Idea[]> { return request('/api/ideas'); }
export function createIdea(item: IdeaInput): Promise<Idea> {
  return request('/api/ideas', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(item) });
}
export function updateIdea(id: string, item: IdeaInput): Promise<Idea> {
  return request(`/api/ideas/${encodeURIComponent(id)}`, { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(item) });
}
export function deleteIdea(id: string): Promise<{ ok: boolean }> {
  return request(`/api/ideas/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

async function mediaReady(channel: string): Promise<boolean> {
  if (!['images', 'video', 'tts', 'stt', 'embeddings'].includes(channel)) return false;
  try {
    const catalog = await hostMediaApi().models({ channel: channel as MediaChannel });
    return !!catalog.selection.model && catalog.models.some(model => model.id === catalog.selection.model);
  } catch { return false; }
}
export async function fetchSkills(): Promise<SkillItem[]> {
  const skills = await request<SkillItem[]>('/api/skills');
  const readiness = new Map<string, Promise<boolean>>();
  return Promise.all(skills.map(async skill => {
    if (!skill.mediaChannel) return skill;
    if (!readiness.has(skill.mediaChannel)) readiness.set(skill.mediaChannel, mediaReady(skill.mediaChannel));
    return { ...skill, apiConfigured: await readiness.get(skill.mediaChannel)! };
  }));
}

export async function fetchSkillDetail(name: string): Promise<SkillDetail> {
  const detail = await request<SkillDetail>(`/api/skill/${encodeURIComponent(name)}`);
  if (detail.apiSpec?.managedByHost && detail.apiSpec.channel) return { ...detail, apiConfigured: await mediaReady(detail.apiSpec.channel) };
  return detail;
}

/** 保存 API key 到项目根 .env（仅注册表内变量）。返回各需 API 的 skill 是否已就绪。 */
export function saveEnv(updates: Record<string, string>): Promise<{ ok: boolean; skills: Record<string, boolean> }> {
  return request('/api/env', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ updates }),
  });
}

export async function executeSkill(skill: string, input: string, persona?: string): Promise<SkillResponse> {
  const sessionId=`skill-${crypto.randomUUID()}`;
  const response=await executeHostTask({message:input.trim() || `请执行技能 ${skill}`,skillName:skill,persona,sessionId,requireAgent:true},crypto.randomUUID(),{onToken:()=>{}},taskController());
  return {response};
}

export async function runAgent(message: string, persona?: string): Promise<{response:string}> {
  const response=await executeHostTask({message,persona,sessionId:`tool-${crypto.randomUUID()}`,requireAgent:true},crypto.randomUUID(),{onToken:()=>{}},taskController());
  return {response};
}

export async function buildProfile(name:string,form:Record<string,unknown>):Promise<ProfileBuildResponse> {
  const built=await request<ProfileBuildResponse>('/api/profile/build',{method:'POST',body:JSON.stringify({name,form})});
  if(!built.preparation || !built.turnId) throw new Error('请更新 Easel 原生组件以启用宿主画像增强');
  let hostStarted=false;
  try {
    const prepared=await prepareTask(built.preparation);
    if(prepared.runtime==='api') throw new Error('基线画像已创建；增强画像需要在设置中选择 Agent');
    const task=await startAgentTask(built.preparation,prepared,built.turnId,taskController());
    task.profileName=name;saveHostTask(task);hostStarted=true;
    await request(`/api/profile/build/status/${encodeURIComponent(name)}`,{method:'PUT',body:JSON.stringify({runId:task.runId,sessionId:task.hostSessionId,hostStatus:'running'})});
    return {...built,status:'running'};
  } catch(error) {
    const log=error instanceof Error?error.message:String(error);
    if(hostStarted)return {...built,status:'running',log};
    await request(`/api/profile/build/status/${encodeURIComponent(name)}`,{method:'PUT',body:JSON.stringify({hostStatus:'failed',log})});
    return {...built,status:'failed',log};
  }
}

const profileReads=new Map<string,Promise<ProfileBuildStatus>>();
export function profileBuildStatus(name:string):Promise<ProfileBuildStatus> {
  const current=profileReads.get(name);if(current)return current;
  const pending=readProfileBuildStatus(name).finally(()=>profileReads.delete(name));profileReads.set(name,pending);return pending;
}
async function readProfileBuildStatus(name:string):Promise<ProfileBuildStatus> {
  const path=`/api/profile/build/status/${encodeURIComponent(name)}`;
  const status=await request<ProfileBuildStatus>(path);
  if(status.state==='done' || status.state==='failed') return status;
  // The native linkage is authoritative across page closes. The local task record
  // also recovers a close between host start and native linkage persistence.
  let task=status.turnId?loadHostTasks()[status.turnId]:undefined;
  const id=status.runId || task?.runId;
  if(!id) return status;
  if(!status.runId) await request(path,{method:'PUT',body:JSON.stringify({runId:id,sessionId:task?.hostSessionId,hostStatus:'running'})});
  task ??= {turnId:status.turnId || id,sessionId:`profile-${name}`,hostSessionId:status.sessionId || '',runId:id,after:0,text:'',thinking:'',questions:[],permissions:[],status:'running',created:Date.now(),profileName:name};
  const page=await hostAgentApi().read({id,after:task.after});
  await applyAgentPage(task,page,{onToken:()=>{}});
  if(page.done) return request(path,{method:'PUT',body:JSON.stringify({runId:id,hostStatus:page.status,log:page.status==='completed'?'AI 增强完成':`增强未完成 (${page.status})；基线画像仍可用`})});
  return {...status,state:'running',runId:id};
}

export function fetchOutputs(): Promise<OutputNode[]> {
  return request<OutputNode[]>('/api/outputs');
}

export function fetchArtifactArchive(path: string, signal?: AbortSignal): Promise<{ paths: string[] }> {
  return request<{ paths: string[] }>(`/api/artifacts/archive?path=${encodeURIComponent(path)}`, { signal });
}

export function fetchOutputContent(path: string): Promise<OutputContent> {
  return request<OutputContent>(`/api/output/${path.split('/').map(encodeURIComponent).join('/')}`);
}

/** 媒体文件（图片/视频/音频/HTML/PDF）的原样 URL，用于 <img>/<video>/iframe/下载。 */
export function mediaUrl(path: string): string {
  return `/api/media/${path.split('/').map(encodeURIComponent).join('/')}`;
}

/** 删除内容库里的文件或整个项目目录（系统数据受保护，后端会拒）。 */
export function deleteOutput(path: string): Promise<{ ok: boolean; deleted: string }> {
  return request(`/api/output/${path.split('/').map(encodeURIComponent).join('/')}`, { method: 'DELETE' });
}

export interface UploadedFile { id: string; name: string; path: string; }

/** OpenClaw ask_user 问答题（SSE question 事件 payload）。 */
export interface ChatQuestionOption { id?:string; label: string; description?: string; }
export interface ChatQuestionItem {
  questionId: string;
  header?: string;
  question: string;
  options?: ChatQuestionOption[];
  multiSelect?: boolean;
  allowCustom?:boolean;
}
export interface ChatQuestion {
  runId?:string;
  id: string;              // gateway question record id (ask_...)
  questions: ChatQuestionItem[];
  expiresAtMs?: number;
  status?: string;
}
export interface ChatQuestionClosed { id: string; status: string; }
export async function answerQuestion(
  payload: { questionId: string; answers: Record<string, string[]>; resolvedBy?: string },
): Promise<{ ok: boolean; error?: string; status?: string; expired?: boolean }> {
  try {await respondHostQuestion(payload.questionId,payload.answers);return {ok:true,status:'answered'};}
  catch(error) {return {ok:false,error:error instanceof Error?error.message:String(error)};}
}

export async function questionStatus(questionIds:string[]):Promise<Record<string,{status:string}>> {
  return hostQuestionStatuses(questionIds);
}
/** 上传素材到当前会话的隔离 inbox（分块 CMD 通道），返回后端可校验的附件引用。 */
export function uploadFiles(files: File[], sessionId: string): Promise<UploadedFile[]> {
  return uploadViaChunks(files, sessionId, false);
}

/** 超限文件复制通道：与 uploadFiles 同构的附件引用（本机复制通道，无大小上限）。 */
export interface AdoptedFile { id: string; name: string; path: string }
export function adoptOversize(files: File[], sessionId: string): Promise<AdoptedFile[]> {
  return uploadViaChunks(files, sessionId, true);
}

export function deleteSession(sessionKey: string): Promise<{ deleted: boolean }> {
  return request<{ deleted: boolean }>(`/api/session/${encodeURIComponent(sessionKey)}`, {
    method: 'DELETE',
  });
}

// ---- 账号登录 ----
export interface AccountItem {
  platform: string;
  name: string;
  backend: string;      // xhs | web | biliup | unsupported
  supported: boolean;
  loggedIn: boolean;
  note: string;
}

export interface LoginStart {
  mode: 'qr' | 'terminal' | 'credentials';   // credentials = 凭证式（公众号 AppID/AppSecret）
  state?: string;       // starting | qr_ready | success | expired | error | unknown
  message?: string;
  qr?: string;          // outputs 下相对路径，用 mediaUrl() 取图
  qrTs?: number;
  configured?: boolean; // 凭证式：是否已配置
}

// ---- 凭证式账号（微信公众号 AppID/AppSecret）----
export interface CredentialStatus {
  configured: boolean;
  appIdMasked: string;
  name: string;
  author: string;
}

/** 读取凭证式平台（公众号）已配置状态（AppSecret 不回传）。 */
export function getCredentials(platform: string): Promise<CredentialStatus> {
  return request<CredentialStatus>(`/api/accounts/${encodeURIComponent(platform)}/credentials`);
}

/** 保存公众号 AppID/AppSecret（后端会调官方接口验证连通性）。 */
export function saveCredentials(
  platform: string,
  payload: { appId: string; appSecret: string; name?: string; author?: string },
): Promise<{ ok: boolean; message: string }> {
  return request(`/api/accounts/${encodeURIComponent(platform)}/credentials`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
}

/** 启动「公众号后台」扫码登录（数据中心取数用，独立于 AppID 凭证）。返回二维码状态。 */
export function startMpLogin(platform: string): Promise<LoginStatus> {
  return request<LoginStatus>(`/api/accounts/${encodeURIComponent(platform)}/mp-login`, { method: 'POST' });
}

/** 轮询公众号后台扫码登录状态。 */
export function mpLoginStatus(platform: string): Promise<LoginStatus> {
  return request<LoginStatus>(`/api/accounts/${encodeURIComponent(platform)}/mp-login/status`,
    { signal: AbortSignal.timeout(8000) });
}

export interface LoginStatus {
  mode: 'qr';
  state: string;
  message: string;
  qr: string;
  qrTs?: number;
}

export function fetchAccounts(): Promise<AccountItem[]> {
  return request<AccountItem[]>('/api/accounts');
}

export interface AccountWhoami {
  loggedIn: boolean;
  name: string;
  avatar: string;   // 头像 URL（http）或空
}

/** 真校验某平台登录态 + 拉昵称/头像（后端起 headless 浏览器，数秒）。 */
export function accountWhoami(platform: string, force = false): Promise<AccountWhoami> {
  return request<AccountWhoami>(`/api/accounts/${encodeURIComponent(platform)}/whoami${force ? '?force=true' : ''}`);
}

/** 退出登录：删该平台持久化登录态。 */
export function logoutAccount(platform: string): Promise<{ ok: boolean; deleted: string[] }> {
  return request(`/api/logout/${encodeURIComponent(platform)}`, { method: 'POST' });
}

export interface PublishResult {
  ok?: boolean;
  message: string;
  detail?: string;
  async?: boolean;   // true = 异步发布（抖音，可能触发短信验证），需轮询 publishStatus
  pending?: boolean;
}

export interface PublishingPolicy {
  mode: 'manual' | 'automated';
  automaticSubmission: boolean;
}

export function fetchPublishingPolicy(): Promise<PublishingPolicy> {
  return request('/api/publishing-policy');
}

/** Create a local export only; no social platform is contacted. */
export function exportManualPackage(platform: string, payload: {
  title: string; body: string; tags: string; media: string[]; reviewed: boolean;
}): Promise<{ path: string; filename: string; published: false }> {
  return request(`/api/manual-publish/${encodeURIComponent(platform)}/export`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(payload),
  });
}

/** 一键发布到某平台（真发布，--exec）。media 为 outputs 相对路径数组。
 * 抖音返回 {async:true}，需轮询 publishStatus；其他平台同步返回结果。
 * draft=true 表示存草稿（仅今日头条支持，落后台草稿箱不公开发布）。 */
export function publishNow(
  platform: string,
  payload: { title: string; body: string; media: string[]; tags?: string; draft?: boolean; persona?: string },
): Promise<PublishResult> {
  return request<PublishResult>(`/api/publish/${encodeURIComponent(platform)}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
}

export interface PublishStatus {
  mode: 'publish';
  state: string;     // starting | sms_required | verifying | success | error | unknown
  message: string;
}

/** 轮询异步发布状态（抖音）。 */
export function publishStatus(platform: string): Promise<PublishStatus> {
  return request<PublishStatus>(`/api/publish/${encodeURIComponent(platform)}/status`);
}

/** 发布触发短信墙时回填验证码。 */
export function submitPublishSms(platform: string, code: string): Promise<{ ok: boolean }> {
  return request(`/api/publish/${encodeURIComponent(platform)}/sms`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ code }),
  });
}

export function startLogin(platform: string): Promise<LoginStart> {
  return request<LoginStart>(`/api/login/${encodeURIComponent(platform)}?restart=true`, { method: 'POST' });
}

export function loginStatus(platform: string): Promise<LoginStatus> {
  return request<LoginStatus>(`/api/login/${encodeURIComponent(platform)}/status`,
    { signal: AbortSignal.timeout(8000) });
}

// ---- 归因层：账号创作数据 ----
export interface AnalyticsPlatform {
  platform: string;
  name: string;
  loggedIn: boolean;
}

export interface AccountAnalytics {
  platform: string;
  name: string;
  nickname: string;
  loggedIn: boolean;
  followers: number | null;
  likes: number | null;
  following: number | null;
  posts: number | null;
  metrics: { label: string; value: string | number; vs: string }[];
  notes: { title: string; url: string; cover?: string; stat?: string }[];
  growth: Record<'last' | 'day' | 'week' | 'month' | 'year',
    { followers: number | null; likes: number | null; posts: number | null; since_days: number | null } | null>;
  fetched_at: number;
  warning?: string;
}

/** 支持抓数据的平台 + 各自登录态。 */
export function fetchAnalyticsPlatforms(): Promise<AnalyticsPlatform[]> {
  return request<AnalyticsPlatform[]>('/api/analytics/platforms');
}

/** 抓某平台已登录账号的创作数据，客户端等待时限略长于后端。 */
export function fetchAccountAnalytics(platform: string): Promise<AccountAnalytics> {
  return request<AccountAnalytics>(`/api/analytics/${encodeURIComponent(platform)}`,
    { signal: AbortSignal.timeout(platform === 'wechat-oa' ? 195000 : 105000) });
}

/** 回填短信验证码（登录风控短信墙）：提交后 runner 读走填码继续登录。 */
export function submitLoginSms(platform: string, code: string): Promise<{ ok: boolean }> {
  return request(`/api/login/${encodeURIComponent(platform)}/sms`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ code }),
  });
}

/** 用户显式停止：终止后端正在跑的对话 agent，释放会话锁，令下一句能立刻发。 */
export async function stopChat(sessionId:string):Promise<{stopped:boolean}> {
  return {stopped:await cancelHostSession(sessionId)};
}

export function streamChat(
  message:string,persona:string|undefined,sessionId:string,
  onToken:(chunk:string)=>void,onDone:(sessionKey?:string)=>void,onError:(err:Error)=>void,
  onThinking?:(chunk:string)=>void,onActivity?:(status:string)=>void,_onInterrupted?:()=>void,
  turnId:string=crypto.randomUUID(),resumeOnly=false,onRecoveryUnavailable?:()=>void,
  attachments:UploadedFile[]=[],onQuestion?:(q:ChatQuestion)=>void,onHeartbeat?:(note:string)=>void,
  onQuestionClosed?:(q:ChatQuestionClosed)=>void,
):AbortController {
  const controller=taskController();
  const history=loadSessions().find(session=>session.id===sessionId)?.messages || [];
  const messages=history.slice(0,history.at(-1)?.role==='user'?-1:undefined).map(({role,content})=>({role,content}));
  void executeHostTask({message,persona,sessionId,attachments,messages},turnId,{onToken,onThinking,onActivity,onQuestion,onHeartbeat,onQuestionClosed},controller,resumeOnly)
    .then(()=>{if(!controller.signal.aborted)onDone(sessionId);})
    .catch(error=>{
      if(controller.signal.aborted)return;
      if(resumeOnly && !loadHostTasks()[turnId] && onRecoveryUnavailable)onRecoveryUnavailable();
      else onError(error instanceof Error?error:new Error(String(error)));
    });
  return controller;
}

export async function fetchLastTurn(sessionId:string,turnId?:string):Promise<{status:string;text:string;turn_id?:string}> {
  const task=turnId?loadHostTasks()[turnId]:Object.values(loadHostTasks()).filter(task=>task.sessionId===sessionId).sort((a,b)=>b.created-a.created)[0];
  if(!task)throw new Error('任务记录不存在');
  return {status:task.status==='completed'?'done':task.status,text:task.text,turn_id:task.turnId};
}

// ═══ 设置面板 · 环境安装（install_tool 引擎桥） ═══

export interface EnvTool {
  id: string;
  state: 'ok' | 'missing' | 'no_dir' | 'fail';
  version: string | null;
  detail: string | null;
  name: string;
  group: string;
  group_name?: string;
  desc: string;
  big?: boolean;
}

export interface EnvToolsResponse {
  python: string;
  tools: EnvTool[];
  cachedAt?: number;
}

export function fetchEnvTools(refresh = false): Promise<EnvToolsResponse> {
  return request(`/api/env/tools${refresh ? '?refresh=1' : ''}`);
}

export interface EnvJobResult {
  id: string;
  state: 'ok' | 'fail';
  version?: string | null;
  strategy?: string | null;
  detail?: string | null;
}

export interface EnvJob {
  jobId: string;
  id: string;
  state: 'running' | 'ok' | 'fail';
  lines: string[];
  result: EnvJobResult | null;
  started: number;
  ended: number | null;
}

export function startEnvInstall(id: string): Promise<{ jobId: string; id: string; state: string }> {
  return request('/api/env/install', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id }),
  });
}

export function fetchEnvJob(jobId: string): Promise<EnvJob> {
  return request(`/api/env/job/${encodeURIComponent(jobId)}`);
}

// ═══ 设置面板 · 模型通道（只读 + 真自测） ═══

export interface ModelRow {
  slot?: string;
  order: number;
  name: string;
  sub: string;
  type: string;
  model: string;
  baseUrl: string;
  keyMasked: string;
  role: string;
  result: string;
  protocol?: string;
  thinking?: boolean;
  thinkingFormat?: string;
  keyNew?: string;
  keyNew2?: string;
  key2Label?: string;
  key2Masked?: string;
  modelEditable?: boolean;
  baseEditable?: boolean;
  baseOptional?: boolean;
  adv?: boolean;
  deletable?: boolean;
}

export interface ModelSaveRow {
  slot: string;
  name?: string;
  model: string;
  baseUrl: string;
  key: string;
  key2?: string;
  primary?: boolean;
  /** 自定义供应商上游协议：openai（默认）| anthropic */
  protocol?: string;
  thinking?: boolean;
  thinkingFormat?: string;
}

export interface ModelSaveResponse extends ModelChannelsResponse {
  ok: boolean;
  note?: string;
}

export function saveModelConfig(channel: string, rows: ModelSaveRow[]): Promise<ModelSaveResponse> {
  return request('/api/settings/models/save', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ channel, rows }),
  });
}

export interface ModelChannelRows { rows: ModelRow[] }

export interface ModelChannelsResponse {
  transport?: 'api' | 'openclaw';
  channels: {
    chat: ModelChannelRows;
    transcribe: ModelChannelRows;
    image?: ModelChannelRows;
    video?: ModelChannelRows;
    music?: ModelChannelRows;
    speech?: ModelChannelRows;
  };
  primary: string;
}

export function fetchModelChannels(runtime?: string): Promise<ModelChannelsResponse> {
  return request(`/api/settings/models${runtime ? `?runtime=${encodeURIComponent(runtime)}` : ''}`);
}

// ── 网络代理设置（EASEL_PROXY；更多设置页） ─────────────────────────

export interface ProxySettings {
  proxy: string;                       // 生效中的代理地址，空串=未配置
  source: 'env' | 'envfile' | 'none';  // env=启动环境固定（只读）；envfile=.env 持久化
}

export function fetchProxySettings(): Promise<ProxySettings> {
  return request('/api/settings/proxy');
}

export function saveProxySettings(proxy: string): Promise<{ ok: boolean; proxy: string; source: string }> {
  return request('/api/settings/proxy', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ proxy }),
  });
}

// （保存接口见上方 saveModelConfig）

export interface SelftestResult { baseUrl: string; ok: boolean; ms: number; detail?: string }

export function runChannelSelftest(channel: string): Promise<{ channel: string; results: SelftestResult[]; testedAt: number }> {
  return request('/api/settings/models/selftest', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ channel }),
  });
}

export interface ModelsFetchResponse { baseUrl: string; models: string[]; fetchedAt: number }

/**
 * 拉取某供应商的模型列表（自定义供应商免手打模型名）。
 * key 留空时后端按 slot 从已存配置里取（用户不用为了拉列表重贴一遍 Key）。
 */
export function fetchAvailableModels(
  baseUrl: string, key: string, protocol: string, slot = '',
  name = '',
): Promise<ModelsFetchResponse> {
  return request('/api/settings/models/available', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ baseUrl, key, protocol, slot, name }),
  });
}
