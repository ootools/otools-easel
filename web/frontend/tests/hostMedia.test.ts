import assert from 'node:assert/strict';
import test from 'node:test';
import { hostMediaApi, mediaModelsForTurn, saveMediaModel } from '../src/lib/hostMedia.ts';
function fixture() {
  const settings = new Map<string, string>();
  const calls: unknown[] = [];
  const media = {
    models: async ({ channel }: { channel: string }) => ({ models: [{ id: `${channel}-opaque`, capabilities: ['image.generate'] }], selection: { model: settings.get(channel) ?? null } }),
    getSettings: async ({ channel }: { channel: string }) => ({ model: settings.get(channel) ?? null }),
    saveSettings: async ({ channel, model }: { channel: string; model: string | null }) => { calls.push({ channel, model }); if (model) settings.set(channel, model); else settings.delete(channel); return { model }; },
  };
  Object.assign(globalThis, { window: { otools: { ai: { media, models: async () => ({}), getSettings: async () => ({}), saveSettings: async () => ({}), capabilities: async () => ({ configured: true }) } } } });
  return { media, calls };
}
test('media models save only Easel owner preferences, not native or global credentials', async () => {
  const f = fixture();
  assert.equal((await hostMediaApi().models({ channel: 'images' })).models[0].id, 'images-opaque');
  await saveMediaModel('images', 'chosen-image');
  assert.deepEqual(await mediaModelsForTurn(), { images: 'chosen-image' });
  assert.deepEqual(f.calls, [{ channel: 'images', model: 'chosen-image' }]);
});
test('a new save to an already-read channel is included before a task snapshot resolves', async () => {
  const f = fixture(); let release!: () => void; let blocked = false;
  const read = f.media.getSettings;
  f.media.getSettings = async value => {
    if (value.channel === 'video' && !blocked) { blocked = true; await new Promise<void>(resolve => { release = resolve; }); }
    return read(value);
  };
  const turn = mediaModelsForTurn();
  await new Promise(resolve => setTimeout(resolve, 0));
  await saveMediaModel('images', 'new-image');
  release();
  assert.deepEqual(await turn, { images: 'new-image' });
});

test('media task snapshots wait for the latest selected model save', async () => {
  const f = fixture(); let release!: () => void;
  const save = f.media.saveSettings;
  f.media.saveSettings = async input => { await new Promise<void>(resolve => { release = resolve; }); return save(input); };
  const saving = saveMediaModel('video', 'chosen-video');
  const turn = mediaModelsForTurn();
  await new Promise(resolve => setTimeout(resolve, 0));
  release(); await saving;
  assert.deepEqual(await turn, { video: 'chosen-video' });
});
