import assert from 'node:assert/strict';
import test from 'node:test';
import { fetchSkillDetail, fetchSkills } from '../src/lib/api.ts';

test('skill readiness comes from the plugin-owned channel choice, not old global defaults', async () => {
  const apiSpec = { label: 'AI Switch', settings: [], providers: [], channel: 'images', managedByHost: true };
  Object.assign(globalThis, { window: { otools: {
    ai: { models: async()=>({}), getSettings:async()=>({}), saveSettings:async()=>({}), capabilities:async()=>({configured:true}), media: {
      models: async()=>({models:[{id:'opaque-image'}],selection:{model:'opaque-image'}}),
      getSettings: async()=>({model:'opaque-image'}), saveSettings:async()=>({model:'opaque-image'}),
    } },
    invokeNative: async(_method: string, payload: { path: string }) => payload.path === '/api/skills'
      ? [{name:'ai-image-gen',description:'Image',layer:'produce',needsApi:true,apiConfigured:false,mediaChannel:'images'}]
      : {name:'ai-image-gen',description:'Image',layer:'produce',body:'',needsApi:true,apiConfigured:false,apiSpec},
  } } });
  assert.equal((await fetchSkillDetail('ai-image-gen')).apiConfigured, true);
  assert.equal((await fetchSkills())[0].apiConfigured, true);
});
