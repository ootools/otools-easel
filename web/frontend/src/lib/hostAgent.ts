import { mediaModelsForTurn } from './hostMedia.ts';
import { nativeRequest, otoolsBridge, throwIfAborted } from './nativeBridge.ts';
import { hostAiApi, resolveHostTurnSelection } from './hostAi.ts';
import { loadHostTasks, saveHostTask, type HostTask } from './store.ts';
import type { ChatQuestion, ChatQuestionClosed, UploadedFile } from './api.ts';

// Boundary types follow host/vendor/otools-plugin-sdk/src/agent-types.ts.
export interface AgentCapabilities { models:boolean; reasoning:boolean; resume:boolean; permissions:boolean; questions:boolean; tools:boolean; filesystem:boolean; terminal:boolean; sandbox:boolean }
export interface AgentProvider { id:string; name:string; adapter:'codex'|'acp'; configuration?:'host'|'native'; source:'managed'|'external'|'configured'; available:boolean; capabilities:AgentCapabilities }
export interface AgentEvent { sequence:number; type:'text'|'reasoning'|'tool'|'permission'|'question'|'status'|'done'|'cancelled'|'error'; text?:string; itemId?:string; data?:unknown; error?:string }
export interface AgentRead { id:string; events:AgentEvent[]; done:boolean; after:number; status:'running'|'completed'|'cancelled'|'failed'|'interrupted'; text?:string }
export interface AgentApi {
  providers():Promise<AgentProvider[]>;
  health(options:{providerId:string;refresh?:boolean}):Promise<{status:string;checks:Array<{id:string;status:string;message:string}>}>;
  openManager(options?:{providerId?:string}):Promise<void>;
  tools():Promise<Array<{name:string;title?:string;description?:string}>>;
  workspace(options?:{workspaceId?:string}):Promise<{id:string;name:string}>;
  workspaceFiles(options:{workspaceId:string;path?:string}):Promise<Array<{path:string;size:number;directory:boolean}>>;
  syncAssets(options:{workspaceId:string;files:Array<{path:string;contentBase64:string}>}):Promise<void>;
  openSession(options:{providerId:string;clientSessionId?:string;workspaceId?:string;instructions?:string;sandbox?:'read-only'|'workspace-write';network?:boolean;background?:boolean;tools?:string[]}):Promise<{id:string;providerId:string;workspaceId:string;capabilities:AgentCapabilities;status:string}>;
  configOptions(options:{sessionId:string}):Promise<{options:AgentConfigOption[]}>;
  setConfigOption(options:{sessionId:string;configId:string;value:string}):Promise<{options:AgentConfigOption[]}>;
  start(options:{sessionId:string;prompt:string;model?:string;reasoningEffort?:string|null;timeoutSecs?:number;requestId?:string;mediaModels?:Record<string,string>}):Promise<{id:string;sessionId:string}>;
  read(options:{id:string;after?:number}):Promise<AgentRead>;
  cancel(options:{id:string}):Promise<void>;
  closeSession(options:{sessionId:string}):Promise<void>;
  respondQuestion(options:{id:string;answers:Record<string,string[]>}):Promise<void>;
  openPermission(options:{id:string}):Promise<void>;
}
export interface AgentConfigOption { id:string;name:string;category?:string;currentValue:string;options:Array<{value:string;name:string}> }
export interface PreparationInput { message:string;persona?:string;sessionId:string;skillName?:string;attachments?:UploadedFile[];messages?:Array<{role:'user'|'assistant';content:string}>;requireAgent?:boolean }
export interface PreparedTask { runtime:string;providerId:string;instructions:string;prompt:string;messages:Array<{role:'system'|'user'|'assistant';content:Array<{type:'text';text:string}>}>;sandbox:'read-only'|'workspace-write';network:boolean;assets?:Array<{path:string;contentBase64:string}> }
export interface TaskCallbacks { onToken:(text:string)=>void;onThinking?:(text:string)=>void;onActivity?:(text:string)=>void;onQuestion?:(question:ChatQuestion)=>void;onQuestionClosed?:(closed:ChatQuestionClosed)=>void;onHeartbeat?:(note:string)=>void }
export type TaskController = AbortController & { detach():void; detached:boolean };
export function taskController():TaskController {
  const controller = new AbortController() as TaskController;
  controller.detached = false;
  controller.detach = () => { controller.detached = true; controller.abort(); };
  return controller;
}
export function hostAgentApi():AgentApi {
  const api = otoolsBridge()?.agent;
  if (!api?.providers || !api.workspace || !api.openSession || !api.start || !api.read || !api.cancel || !api.openPermission || !api.respondQuestion) {
    throw new Error('请更新 Otools 宿主以使用统一 Agent 服务；文件浏览仍可使用');
  }
  return api;
}
export function selectedAgentTools():string[] {
  try { const value:unknown=JSON.parse(localStorage.getItem('easel_agent_tools') || '[]'); return Array.isArray(value) ? value.filter((name):name is string=>typeof name==='string').slice(0,64) : []; } catch { return []; }
}
export function saveAgentTools(names:string[]):void { localStorage.setItem('easel_agent_tools',JSON.stringify(names)); }

const interactionListeners=new Set<()=>void>();
const auxiliaryQuestions=new Map<string,ChatQuestion>();
export function interactionQuestions():ChatQuestion[] { return [...auxiliaryQuestions.values()]; }
export function subscribeInteractions(listener:()=>void):()=>void {interactionListeners.add(listener); return ()=>{interactionListeners.delete(listener);};}
function notifyInteractions() { for(const listener of interactionListeners) listener(); }
function deliverQuestion(question:ChatQuestion, callbacks:TaskCallbacks) {
  if(callbacks.onQuestion) callbacks.onQuestion(question);
  else {auxiliaryQuestions.set(question.id,question);notifyInteractions();}
}
export async function respondHostQuestion(id:string,answers:Record<string,string[]>):Promise<void> {
  await hostAgentApi().respondQuestion({id,answers});
  for(const task of Object.values(loadHostTasks())) {
    if(task.questions.some(q=>q.id===id)) { task.questions=task.questions.map(q=>q.id===id?{...q,status:'answered'}:q);saveHostTask(task); }
  }
  auxiliaryQuestions.delete(id);notifyInteractions();
}
export function hostQuestionStatuses(ids:string[]):Record<string,{status:string}> {
  const result:Record<string,{status:string}>={};
  for(const task of Object.values(loadHostTasks())) for(const question of task.questions) {
    if(ids.includes(question.id)) result[question.id]={status:question.status || (task.status==='running'?'pending':'closed')};
  }
  return result;
}

export async function prepareTask(input:PreparationInput,signal?:AbortSignal):Promise<PreparedTask> {
  return nativeRequest('/api/agent/prepare',{method:'POST',body:JSON.stringify(input),signal});
}
export async function startAgentTask(input:PreparationInput,prepared:PreparedTask,turnId:string,controller:TaskController):Promise<HostTask> {
  const api=hostAgentApi();
  const providers=await api.providers();
  const provider=providers.find(p=>p.id===prepared.providerId);
  if(!provider?.available) throw new Error('此 Agent 尚不可用，请在 Otools Agent 管理中安装或配置');
  // Resolve after pending preference saves, snapshot once before starting Codex.
  const selection=provider.adapter==='codex' && provider.configuration!=='native' ? await resolveHostTurnSelection() : undefined;
  throwIfAborted(controller.signal);
  const workspace=await api.workspace();
  throwIfAborted(controller.signal);
  if(prepared.assets?.length) {
    const existing=await api.workspaceFiles({workspaceId:workspace.id});
    const files=prepared.assets.filter(file=>!existing.some(item=>item.path===file.path));
    if(files.length) await api.syncAssets({workspaceId:workspace.id,files});
  }
  throwIfAborted(controller.signal);
  const session=await api.openSession({clientSessionId:input.sessionId,providerId:provider.id,workspaceId:workspace.id,instructions:prepared.instructions,...(provider.adapter==='codex'?{sandbox:prepared.sandbox,network:prepared.network}:{}),background:true,tools:selectedAgentTools()});
  if(controller.signal.aborted) {
    if(session.status==='idle') await api.closeSession({sessionId:session.id});
    throwIfAborted(controller.signal);
  }
  let mediaModels: Record<string, string> | undefined;
  if (provider.adapter === 'codex' && (hostAiApi() as unknown as { media?: unknown }).media) mediaModels = await mediaModelsForTurn();
  throwIfAborted(controller.signal);
  const started=await api.start({sessionId:session.id,prompt:prepared.prompt,...(selection?{model:selection.model!,reasoningEffort:selection.reasoningEffort}:{}),...(mediaModels ? { mediaModels } : {}),requestId:turnId,timeoutSecs:7200});
  const task:HostTask={turnId,sessionId:input.sessionId,hostSessionId:started.sessionId,runId:started.id,after:0,text:'',thinking:'',questions:[],permissions:[],status:'running',created:Date.now()};
  try { saveHostTask(task); } catch(error) {await api.cancel({id:started.id});throw error;}
  if(controller.signal.aborted && !controller.detached) await api.cancel({id:started.id});
  throwIfAborted(controller.signal);
  return task;
}

function asQuestion(event:AgentEvent):ChatQuestion|undefined {
  const value=event.data as {id?:string;runId?:string;questions?:Array<{id:string;header?:string;question:string;options?:Array<{id:string;label:string;description?:string}>;allowCustom?:boolean}>}|undefined;
  if(!value?.id || !Array.isArray(value.questions)) return;
  return {id:value.id,runId:value.runId,status:'pending',questions:value.questions.map(q=>({questionId:q.id,header:q.header,question:q.question,options:q.options,allowCustom:q.allowCustom}))};
}
export async function applyAgentPage(task:HostTask,page:AgentRead,callbacks:TaskCallbacks):Promise<void> {
  const api=hostAgentApi();
  const saved=loadHostTasks()[task.turnId];
  for(const question of task.questions) if(saved?.questions.some(q=>q.id===question.id && q.status==='answered')) question.status='answered';
  for(const event of page.events) {
    if(event.sequence<=task.after) continue;
    if(event.sequence!==task.after+1) throw new Error('宿主事件记录出现缺口，无法可靠续接；请查看 Agent 管理');
    if(event.type==='text') {task.text+=event.text || '';callbacks.onToken(event.text || '');}
    if(event.type==='reasoning') {task.thinking=(task.thinking+(event.text || '')).slice(-4000);callbacks.onThinking?.(event.text || '');}
    if(event.type==='tool' || event.type==='status') callbacks.onActivity?.(event.text || 'Agent 运行中');
    if(event.type==='permission') {
      const id=(event.data as {id?:string}|undefined)?.id;
      if(!id) throw new Error('宿主授权事件缺少标识');
      if(!task.permissions.includes(id)) {await api.openPermission({id});task.permissions.push(id);}
    }
    if(event.type==='question') {
      const question=asQuestion(event);
      if(question && !task.questions.some(q=>q.id===question.id)) {task.questions.push(question);deliverQuestion(question,callbacks);}
    }
    if(event.type==='error') {task.status='failed';task.error=event.error || event.text || 'Agent 执行失败';}
    if(event.type==='cancelled') task.status='cancelled';
    task.after=event.sequence;
    if(task.text.length>2_000_000 || task.after>100_000) throw new Error('任务输出超过本地恢复上限，请在 Agent 管理中查看');
  }
  if(page.after<task.after) throw new Error('宿主返回了无效的恢复游标');
  if(page.done) {
    task.status=page.status;
    for(const question of task.questions) {
      if(question.status==='pending') {question.status='closed';callbacks.onQuestionClosed?.({id:question.id,status:'closed'});}
      auxiliaryQuestions.delete(question.id);
    }
    notifyInteractions();
  }
  saveHostTask(task);
}
function delay(signal:AbortSignal):Promise<void> { return new Promise(resolve=>{const finish=()=>{clearTimeout(timer);signal.removeEventListener('abort',finish);resolve();};const timer=setTimeout(finish,150);signal.addEventListener('abort',finish,{once:true});}); }
const observedRuns=new Set<string>();
export function isHostTaskObserved(id:string):boolean {return observedRuns.has(id);}
export async function observeAgentTask(task:HostTask,callbacks:TaskCallbacks,controller:TaskController,replay=false):Promise<string> {
  const api=hostAgentApi();
  observedRuns.add(task.runId);
  if(replay) {
    if(task.text) callbacks.onToken(task.text);
    if(task.thinking) callbacks.onThinking?.(task.thinking);
    for(const q of task.questions) if(q.status==='pending') deliverQuestion(q,callbacks);
  }
  let cancellation:Promise<void>|undefined;
  const cancel=()=>{if(!controller.detached) cancellation ??= api.cancel({id:task.runId}).catch(()=>{});};
  controller.signal.addEventListener('abort',cancel,{once:true});
  try {
    while(task.status==='running') {
      throwIfAborted(controller.signal);
      const page=await api.read({id:task.runId,after:task.after});
      throwIfAborted(controller.signal);
      await applyAgentPage(task,page,callbacks);
      if(!page.done) {callbacks.onHeartbeat?.('宿主任务运行中');await delay(controller.signal);}
    }
    if(task.status!=='completed') throw new Error(task.error || `Agent 任务${task.status==='cancelled'?'已取消':'未完成'} (${task.status})`);
    return task.text;
  } catch(error) {
    if(!controller.detached && task.status==='running') {
      cancel();await cancellation;
      task.status=controller.signal.aborted?'cancelled':'failed';
      task.error=error instanceof Error?error.message:String(error);
      saveHostTask(task);
    }
    throw error;
  } finally {observedRuns.delete(task.runId);controller.signal.removeEventListener('abort',cancel);}
}
export async function executeHostTask(input:PreparationInput,turnId:string,callbacks:TaskCallbacks,controller:TaskController,resume=false):Promise<string> {
  if(resume) {
    const task=loadHostTasks()[turnId];
    if(!task) throw new Error('未找到宿主任务关联，旧版任务无法恢复，请重新发送');
    return observeAgentTask(task,callbacks,controller,true);
  }
  const prepared=await prepareTask(input,controller.signal);
  if(prepared.runtime==='api') {
    if(input.requireAgent) throw new Error('此操作需要 Agent 文件与工具能力，请先在设置中选择 Agent');
    const ai=hostAiApi();
    if(!ai.consume) throw new Error('请更新 Otools 宿主以使用统一 AI 服务');
    const selection=await resolveHostTurnSelection();
    throwIfAborted(controller.signal);
    const result=await ai.consume({messages:prepared.messages,model:selection.model!,...(selection.reasoningEffort?{reasoningEffort:selection.reasoningEffort}:{})},{signal:controller.signal,onEvent:event=>{if(event.type==='delta')callbacks.onToken(event.text);}});
    return result.text;
  }
  const task=await startAgentTask(input,prepared,turnId,controller);
  return observeAgentTask(task,callbacks,controller);
}
export async function cancelHostSession(sessionId:string):Promise<boolean> {
  const api=hostAgentApi();let stopped=false;
  for(const task of Object.values(loadHostTasks())) if(task.sessionId===sessionId && task.status==='running') {await api.cancel({id:task.runId});stopped=true;}
  return stopped;
}
