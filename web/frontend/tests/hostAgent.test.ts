import test from 'node:test';
import assert from 'node:assert/strict';
import { streamChat, runAgent, executeSkill, buildProfile, profileBuildStatus, answerQuestion } from '../src/lib/api.ts';

const capabilities = { models:true, reasoning:true, resume:true, permissions:true, questions:true, tools:true, filesystem:true, terminal:true, sandbox:true };
function fixture(runtime = 'codex') {
  const storage = new Map<string,string>();
  Object.assign(globalThis, { localStorage: { getItem: (k:string) => storage.get(k) ?? null, setItem:(k:string,v:string) => storage.set(k,v), removeItem:(k:string) => storage.delete(k) } });
  const starts:any[] = [], opens:any[] = [], prepares:any[] = [], cancels:string[] = [], permissions:string[] = [], answers:any[] = [];
  let profile:any = {state:'pending',log:'',turnId:'profile-turn', preparation:{message:'Enhance',persona:'Writer',skillName:'skill-profile-builder',sessionId:'profile-session'}};
  const events:any[] = [{sequence:1,type:'text',text:'Actual response'}, {sequence:2,type:'done'}];
  const agent = {
    providers: async () => [{id:'codex',name:'Codex',adapter:'codex',configuration:'host',source:'managed',available:true,capabilities}],
    workspace: async () => ({id:'workspace',name:'Easel'}), workspaceFiles: async () => [], syncAssets: async () => {},
    openSession: async (req:any) => {opens.push(req); return {id:'session',providerId:'codex',workspaceId:'workspace',capabilities,status:'idle'};},
    start: async (req:any) => {starts.push(req); return {id:`run-${starts.length}`,sessionId:'session'};},
    read: async ({id,after=0}:any) => ({id,events:events.filter(e => e.sequence > after),after:events.at(-1)?.sequence ?? after,done:events.at(-1)?.type === 'done',status:events.at(-1)?.type === 'done' ? 'completed' : 'running'}),
    cancel: async ({id}:any) => {cancels.push(id);}, closeSession:async()=>{},
    openPermission:async ({id}:any)=>{permissions.push(id);}, respondQuestion:async (value:any)=>{answers.push(value);},
  };
  const ai:any = {models:async()=>({models:[],defaultModel:'host-model',selection:{model:'host-model',reasoningEffort:'high'}}),getSettings:async()=>({model:'host-model',reasoningEffort:'high'}),saveSettings:async(value:any)=>value, capabilities:async()=>({configured:true,model:'host-model'}),
    consume:async (req:any,options:any)=>{ starts.push(req); options.onEvent({sequence:1,type:'delta',text:'Chat answer'}); return {id:'chat-run',text:'Chat answer'};}};
  const native = async (_method:string, payload:any) => {
    if (payload.path === '/api/agent/prepare') { prepares.push(payload.body); return {runtime,providerId:'codex',instructions:'Instructions',prompt:payload.body.message,messages:[{role:'system',content:[{type:'text',text:'Instructions'}]},{role:'user',content:[{type:'text',text:payload.body.message}]}],sandbox:'workspace-write',network:false,assets:[]}; }
    if (payload.path === '/api/profile/build') return {created:true,name:'Writer',async:true,...profile};
    if (payload.path.startsWith('/api/profile/build/status/')) { if(payload.method === 'PUT') profile = {...profile,...payload.body,state:payload.body.hostStatus==='completed'?'done':payload.body.hostStatus==='running'?'running':'failed'}; return profile; }
    throw new Error(`Unexpected native execution: ${payload.path}`);
  };
  Object.assign(globalThis,{window:{otools:{agent,ai,invokeNative:native}}});
  return {agent,ai,events,starts,opens,prepares,cancels,permissions,answers,get profile(){return profile;}};
}
const wait = () => new Promise(resolve=>setTimeout(resolve,10));
function chat(turnId='turn',resume=false) { let ctl:AbortController; const result = new Promise<string>((resolve,reject)=> {let text=''; ctl=streamChat('Hello',undefined,'chat',c=>{text+=c;},()=>resolve(text),reject,undefined,undefined,undefined,turnId,resume);}); return {result, get ctl(){return ctl!;}}; }

test('media-capable tasks snapshot plugin models without exposing any connection credentials', async () => {
  const f=fixture();
  f.ai.media={models:async()=>({models:[],selection:{model:null}}),getSettings:async({channel}:any)=>({model:channel==='images'?'selected-image':null}),saveSettings:async(value:any)=>value};
  await chat('media-snapshot').result;
  assert.deepEqual(f.starts[0].mediaModels,{images:'selected-image'});
  assert.equal(f.starts[0].apiKey,undefined);assert.equal(f.starts[0].baseUrl,undefined);
});

test('chat, one-shot and skill return host output and snapshot host selection',async()=>{
  const f=fixture();
  assert.equal(await chat().result,'Actual response');
  assert.equal((await runAgent('Analyze')).response,'Actual response');
  assert.equal((await executeSkill('persona-check','Review')).response,'Actual response');
  assert.equal(f.prepares[2].skillName,'persona-check');
  assert.equal(f.starts.length,3);
  assert.deepEqual(f.starts.map(r=>[r.model,r.reasoningEffort]),Array(3).fill(['host-model','high']));
  assert.equal(f.opens[0].background,true);
  assert.equal(f.opens[0].workspaceId,'workspace');
});

test('plain chat uses host ai prepared messages without local execution',async()=>{
  const f=fixture('api');
  assert.equal(await chat().result,'Chat answer');
  assert.equal(f.opens.length,0);
  assert.equal(f.starts[0].messages[0].role,'system');
});

test('explicit abort cancels even when host start returns late',async()=>{
  const f=fixture(); let release:(v:any)=>void=()=>{};
  f.agent.start=async()=>new Promise(resolve=>{release=resolve;});
  const c=chat('late'); await wait(); c.ctl.abort(); release({id:'late-run',sessionId:'session'}); await wait();
  assert.deepEqual(f.cancels,['late-run']);
});

test('replay deduplicates overlap and detach preserves background execution',async()=>{
  const f=fixture(); f.events.splice(0,2,{sequence:1,type:'text',text:'First'});
  const c=chat('resume-turn'); await wait();
  assert.equal(typeof (c.ctl as any).detach,'function'); (c.ctl as any).detach();
  assert.deepEqual(f.cancels,[]);
  f.agent.read=async({id}:any)=>({id,events:[{sequence:1,type:'text',text:'First'},{sequence:2,type:'text',text:' last'},{sequence:3,type:'done'}],after:3,done:true,status:'completed'});
  assert.equal(await chat('resume-turn',true).result,'First last');
  assert.equal(f.starts.length,1);
});

test('permissions open host UI and question option ids pass through',async()=>{
  const f=fixture();
  f.events.splice(0,2,{sequence:1,type:'permission',data:{id:'permit'}},{sequence:2,type:'question',data:{id:'question',runId:'run-1',questions:[{id:'choice',question:'Pick',options:[{id:'yes-id',label:'Yes'}]}]}},{sequence:3,type:'text',text:'Done'},{sequence:4,type:'done'});
  await chat().result;
  assert.deepEqual(f.permissions,['permit']);
  await answerQuestion({questionId:'question',answers:{choice:['yes-id']}});
  assert.deepEqual(f.answers,[{id:'question',answers:{choice:['yes-id']}}]);
});

test('profile enhancement persists host id and is done only after completed host read',async()=>{
  const f=fixture(); f.events.splice(0,2);
  const built=await buildProfile('Writer',{});
  assert.equal(built.created,true);
  assert.equal(f.profile.runId,'run-1');
  assert.equal((await profileBuildStatus('Writer')).state,'running');
  f.events.push({sequence:1,type:'done'});
  assert.equal((await profileBuildStatus('Writer')).state,'done');
});


test('abort while opening closes the idle session and does not start a run',async()=>{
  const f=fixture();const closed:string[]=[];let release:(value:any)=>void=()=>{};
  f.agent.openSession=async()=>new Promise(resolve=>{release=resolve;});
  f.agent.closeSession=async({sessionId}:any)=>{closed.push(sessionId);};
  const c=chat('abort-open');await wait();c.ctl.abort();release({id:'idle-session',providerId:'codex',workspaceId:'workspace',capabilities,status:'idle'});await wait();
  assert.equal(f.starts.length,0);assert.deepEqual(closed,['idle-session']);
});

test('read failure cancels owned execution and reports failure',async()=>{
  const f=fixture();f.agent.read=async()=>{throw new Error('cursorGap');};
  await assert.rejects(chat('read-error').result,/cursorGap/);
  assert.deepEqual(f.cancels,['run-1']);
});

test('native Codex provider never receives host gateway model or reasoning',async()=>{
  const f=fixture();f.agent.providers=async()=>[{id:'codex',name:'Codex',adapter:'codex',configuration:'native',source:'external',available:true,capabilities}];
  f.ai.getSettings=async()=>{throw new Error('must not read host selection');};
  assert.equal(await chat('native-codex').result,'Actual response');
  assert.equal(f.starts[0].model,undefined);assert.equal(f.starts[0].reasoningEffort,undefined);
});


test('ACP receives only native configuration and no Codex sandbox assertions',async()=>{
  const f=fixture();f.agent.providers=async()=>[{id:'codex',name:'ACP fixture',adapter:'acp',configuration:'native',source:'configured',available:true,capabilities}];
  f.ai.getSettings=async()=>{throw new Error('must not read host selection');};
  assert.equal(await chat('acp').result,'Actual response');
  assert.equal(f.opens[0].sandbox,undefined);assert.equal(f.opens[0].network,undefined);
  assert.equal(f.starts[0].model,undefined);
});

test('profile read recovers native run linkage after browser storage is cleared',async()=>{
  const f=fixture();f.events.splice(0,2);await buildProfile('Writer',{});
  localStorage.removeItem('easel_host_tasks');
  f.events.push({sequence:1,type:'done'});
  assert.equal((await profileBuildStatus('Writer')).state,'done');
});
