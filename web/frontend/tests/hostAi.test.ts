import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fetchHostModelCatalog, saveHostModelSelection, resolveHostTurnSelection, subscribeHostAiSelection } from '../src/lib/hostAi.ts';
import { nativeRequest, type HostBridgeWindow } from '../src/lib/nativeBridge.ts';
const install = (window: HostBridgeWindow) => { (globalThis as unknown as {window:HostBridgeWindow}).window = window; };
const selection = { model: 'gateway/account/model-x', reasoningEffort: 'xhigh' };
const model = { id: selection.model, ownedBy:'ai-switch',capabilities:[],reasoningLevels:[{effort:'xhigh',description:'Deep'}],defaultReasoningLevel:'xhigh',contextWindow:200000 };
function bridge() {
  return { models: async () => ({ models:[model],defaultModel:'default',selection }), getSettings:async()=>selection,
    saveSettings:async(value:typeof selection)=>value, capabilities:async()=>({configured:true,model:'default'}) };
}
test('catalog and settings use host AI without writing gateway configuration', async () => {
  const saved:unknown[]=[];const ai=bridge();ai.saveSettings=async value=>{saved.push(value);return value;};
  install({otools:{ai}});
  assert.deepEqual((await fetchHostModelCatalog()).models[0].reasoningLevels,[{effort:'xhigh',description:'Deep'}]);
  await saveHostModelSelection(selection);assert.deepEqual(saved,[selection]);
});
test('unselected plugin model does not inherit an obsolete host default', async () => {
  const ai=bridge();ai.getSettings=async()=>({model:null,reasoningEffort:null});
  install({otools:{ai}});await assert.rejects(resolveHostTurnSelection(), /选择模型/);
});
test('native preparation and profile baseline never request host credentials or selection', async () => {
  const calls:any[]=[];let reads=0;const ai=bridge();ai.getSettings=async()=>{reads++;return selection;};
  install({otools:{ai,invokeNativeRaw:async(_method,payload)=>{calls.push(payload);return {ok:true,data:{ok:true}};}}});
  for(const route of ['/api/agent/prepare','/api/profile/build']) {
    await nativeRequest(route,{method:'POST',body:JSON.stringify({message:'hello'})});
    assert.equal(calls.at(-1).body.hostAi,undefined);
  }
  await nativeRequest('/api/outputs');assert.equal(reads,0);
});
test('sending immediately after model switch waits for its saved selection', async () => {
  let release!: () => void;
  let current = selection;
  let reads = 0;
  const ai = bridge();
  ai.saveSettings = async value => { await new Promise<void>(resolve => { release = resolve; }); current = value; return value; };
  ai.getSettings = async () => { reads++; return current; };
  install({ otools: { ai } });
  const observed: unknown[] = [];
  const off = subscribeHostAiSelection(value => observed.push(value));
  const next = { model: 'gateway/new-model', reasoningEffort: 'high' };
  const saving = saveHostModelSelection(next);
  const resolving = resolveHostTurnSelection();
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(reads, 0);
  release();
  await saving;
  assert.deepEqual(await resolving, next);
  assert.deepEqual(observed, [next]);
  off();
});
test('failed preference save blocks a stale send until retry succeeds', async () => {
  const ai = bridge();
  ai.saveSettings = async () => { throw new Error('save unavailable'); };
  install({ otools: { ai } });
  await assert.rejects(saveHostModelSelection(selection), /save unavailable/);
  await assert.rejects(resolveHostTurnSelection(), /save unavailable/);
  ai.saveSettings = async value => value;
  await saveHostModelSelection(selection);
  assert.deepEqual(await resolveHostTurnSelection(), selection);
});
