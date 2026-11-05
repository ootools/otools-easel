import assert from 'node:assert/strict';
import { test } from 'node:test';
import { nativeRequest, type HostBridgeWindow, toBridgeBody } from '../src/lib/nativeBridge.ts';

function install(value: HostBridgeWindow) {
  (globalThis as unknown as { window: HostBridgeWindow }).window = value;
}

test('native requests preserve the original route, JSON body and raw error status', async () => {
  const calls: unknown[] = [];
  install({ otools: {
    invokeNativeRaw: async (method, payload) => {
      calls.push({ method, payload });
      return { ok: true, data: [{ name: '科技达人', description: '分享知识' }] };
    },
  } });
  assert.deepEqual(await nativeRequest('/api/personas'), [{ name: '科技达人', description: '分享知识' }]);
  await nativeRequest('/api/persona/%E7%A7%91%E6%8A%80/file', { method: 'PUT', body: JSON.stringify({ filename: 'style.md', content: '简洁' }) });
  assert.deepEqual(calls[1], { method: 'api', payload: { method: 'PUT', path: '/api/persona/%E7%A7%91%E6%8A%80/file', body: { filename: 'style.md', content: '简洁' } } });
  install({ otools: { invokeNativeRaw: async () => ({ ok: false, error: '非法路径', status: 403 }) } });
  await assert.rejects(nativeRequest('/api/output/%2e%2e/secret'), (error: Error & { status?: number }) => error.message === '非法路径' && error.status === 403);
});

test('an aborted request never dispatches and an in-flight abort rejects promptly', async () => {
  let dispatched = 0;
  install({ otools: { invokeNative: async () => { dispatched++; return new Promise(() => {}); } } });
  const before = new AbortController();
  before.abort();
  await assert.rejects(nativeRequest('/api/outputs', { signal: before.signal }), { name: 'AbortError' });
  assert.equal(dispatched, 0);
  const pending = new AbortController();
  const result = nativeRequest('/api/outputs', { signal: pending.signal });
  await Promise.resolve();
  pending.abort();
  await assert.rejects(result, { name: 'AbortError' });
});

test('FormData keeps repeated file fields and their exact bytes', async () => {
  const form = new FormData();
  form.append('sessionId', 's-A');
  form.append('files', new Blob(['第一条']), 'same.txt');
  form.append('files', new Blob(['第二条']), 'same.txt');
  const payload = await toBridgeBody(form) as { __formData: boolean; fields: Record<string, string>; files: { field: string; name: string; contentBase64: string }[] };
  assert.equal(payload.__formData, true);
  assert.deepEqual(payload.fields, { sessionId: 's-A' });
  assert.deepEqual(payload.files.map(file => file.name), ['same.txt', 'same.txt']);
  assert.deepEqual(payload.files.map(file => Buffer.from(file.contentBase64, 'base64').toString('utf8')), ['第一条', '第二条']);
});

test('fallback bridge receives unwrapped data without requiring native-only globals', async () => {
  install({ utools: { invokeNative: async () => ({ ok: true, files: [] }) } });
  assert.deepEqual(await nativeRequest('/api/upload/limits'), { ok: true, files: [] });
  install({});
  await assert.rejects(nativeRequest('/api/status'), /宿主桥不可用/);
});
