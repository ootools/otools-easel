import assert from 'node:assert/strict';
import { test } from 'node:test';
import { acquireMedia, mediaPath, loadMediaText, type MediaRequest } from '../src/lib/media.ts';

const bytes = new TextEncoder().encode('预览内容');
const request: MediaRequest = async (_path, options) => {
  const query = new URL(_path, 'http://test').searchParams;
  if (!query.has('offset')) return { path: '项目/文案.txt', absolutePath: '/host/easel/outputs/项目/文案.txt', mimeType: 'text/plain', size: bytes.length, mtime: 1, chunkSize: 4 };
  const offset = Number(query.get('offset'));
  const part = bytes.subarray(offset, offset + 4);
  return { path: '项目/文案.txt', offset, contentBase64: Buffer.from(part).toString('base64'), eof: offset + part.length === bytes.length };
};

const environment = (value: Record<string, unknown>) => { (globalThis as unknown as { window: unknown }).window = value; };

test('media logical paths reject escapes and keep the original artifact address form', () => {
  assert.equal(mediaPath('/api/media/%E9%A1%B9%E7%9B%AE/cover.png?v=1'), '项目/cover.png');
  assert.equal(mediaPath('项目/cover.png'), '项目/cover.png');
  for (const path of ['/api/media/%2e%2e/secret', '/api/media/%2fetc/passwd', 'a\\b', 'a/../b', 'file:///etc/passwd']) assert.throws(() => mediaPath(path));
});

test('remote media uses the existing host file service, never the remote clients filesystem', async () => {
  environment({ location: { origin: 'https://otools.example' }, __TAURI_REMOTE_SERVICE__: true });
  const lease = await acquireMedia('项目/video.mp4', undefined, request);
  const url = new URL(lease.url);
  assert.equal(url.origin, 'https://otools.example');
  assert.equal(url.pathname, '/__tauri_remote_service_file__');
  assert.equal(url.searchParams.get('path'), '/host/easel/outputs/项目/文案.txt');
  lease.release();
});

test('native media uses the provided convertFileSrc capability instead of a server', async () => {
  let converted = '';
  environment({ __TAURI_INTERNALS__: { convertFileSrc: (path: string, protocol: string) => { converted = path; return `${protocol}://localhost/file`; } } });
  const lease = await acquireMedia('项目/video.mp4', undefined, request);
  assert.equal(converted, '/host/easel/outputs/项目/文案.txt');
  assert.equal(lease.url, 'asset://localhost/file');
  lease.release();
});

test('bridge-only preview reconstructs split UTF-8 bytes and releases the blob URL', async () => {
  environment({});
  const text = await loadMediaText('项目/文案.txt', undefined, request);
  assert.equal(text, '预览内容');
  const lease = await acquireMedia('项目/文案.txt', undefined, request);
  assert.match(lease.url, /^blob:/);
  assert.equal(await (await fetch(lease.url)).text(), '预览内容');
  lease.release();
  await assert.rejects(fetch(lease.url));
});

test('media loading fails on zero progress or cancellation instead of hanging', async () => {
  environment({});
  const stalled: MediaRequest = async (path) => path.includes('offset=') ? { offset: 0, contentBase64: '', eof: false } : { size: 4, mimeType: 'text/plain', chunkSize: 4 };
  await assert.rejects(loadMediaText('项目/file.txt', undefined, stalled), /进度|无效/);
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(acquireMedia('项目/file.txt', controller.signal, request), { name: 'AbortError' });
});
