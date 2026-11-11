import assert from 'node:assert/strict';
import { test } from 'node:test';
import { streamChat } from '../src/lib/api.ts';
import { nativeRequest } from '../src/lib/nativeBridge.ts';

test('old hosts explicitly refuse agent execution while files remain available',async()=>{
  const calls:string[]=[];
  Object.assign(globalThis,{window:{otools:{invokeNative:async(_method:string,payload:any)=>{
    calls.push(payload.path);
    if(payload.path==='/api/agent/prepare')return {runtime:'codex',providerId:'codex',instructions:'',prompt:'hello',messages:[],sandbox:'read-only',network:false};
    if(payload.path==='/api/outputs')return [{name:'saved.txt'}];
    throw new Error('unexpected route');
  }}},localStorage:{getItem:()=>null,setItem:()=>{}}});
  const error=await new Promise<Error>(resolve=>streamChat('hello',undefined,'old',()=>{},()=>assert.fail('unexpected completion'),resolve));
  assert.match(error.message,/更新 Otools/);
  assert.deepEqual(await nativeRequest('/api/outputs'),[{name:'saved.txt'}]);
  assert.deepEqual(calls,['/api/agent/prepare','/api/outputs']);
});
