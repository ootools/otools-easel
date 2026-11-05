import assert from 'node:assert/strict';
import { test } from 'node:test';
import { uploadViaChunks } from '../src/lib/uploads.ts';

test('the original upload API can send large files as bounded sequential CMD chunks', async () => {
  const calls: { path: string; body: Record<string, unknown> }[] = [];
  const file = new File([new Uint8Array(1024 * 1024 + 3).fill(7)], 'clip.mp4');
  file.arrayBuffer = async () => { throw new Error('must not read whole file'); };
  const refs = await uploadViaChunks([file], 'session-A', true, async (path, options) => {
    const body = JSON.parse(options!.body as string);
    calls.push({ path, body });
    if (path.endsWith('/start')) return { uploadId: 'batch-a' };
    if (path.endsWith('/finish')) return { ok: true, files: [{ id: 'file-a', name: 'clip.mp4', path: '_inbox/s/b/clip.mp4' }] };
    return { received: (body.offset as number) + Buffer.from(body.contentBase64 as string, 'base64').length };
  });
  assert.equal(refs[0].name, 'clip.mp4');
  assert.equal(calls[0].body.sessionId, 'session-A');
  assert.equal(calls[0].body.local, true);
  const chunks = calls.filter(call => call.path.endsWith('/chunk'));
  assert.deepEqual(chunks.map(call => call.body.offset), [0, 1024 * 1024]);
  assert.deepEqual(chunks.map(call => Buffer.from(call.body.contentBase64 as string, 'base64').length), [1024 * 1024, 3]);
});

test('a failed or cancelled upload releases its partial native files', async () => {
  const calls: string[] = [];
  await assert.rejects(uploadViaChunks([new File(['abc'], 'a.txt')], 's', false, async path => {
    calls.push(path);
    if (path.endsWith('/start')) return { uploadId: 'partial' };
    if (path.endsWith('/chunk')) throw new Error('write failed');
    return { ok: true };
  }), /write failed/);
  assert.equal(calls.at(-1), '/api/upload/abort');
});
