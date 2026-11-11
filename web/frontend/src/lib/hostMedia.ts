import { hostAiApi } from './hostAi.ts';
export type MediaChannel = 'images' | 'video' | 'tts' | 'stt' | 'embeddings';
export interface MediaCatalog { models: Array<{ id: string; capabilities: string[] }>; selection: { model: string | null } }
export interface HostMediaApi {
  models(options: { channel: MediaChannel }): Promise<MediaCatalog>;
  getSettings(options: { channel: MediaChannel }): Promise<{ model: string | null }>;
  saveSettings(options: { channel: MediaChannel; model: string | null }): Promise<{ model: string | null }>;
}
export function hostMediaApi(): HostMediaApi {
  const api = (hostAiApi() as unknown as { media?: HostMediaApi }).media;
  if (!api?.models || !api.getSettings || !api.saveSettings) throw new Error('请更新 Otools 以使用 AI Switch 媒体模型选择');
  return api;
}
const saves = new Map<MediaChannel, Promise<{ model: string | null }>>();
export function saveMediaModel(channel: MediaChannel, model: string | null): Promise<{ model: string | null }> {
  const previous = saves.get(channel);
  const saved = (async () => { await previous?.catch(() => {}); return hostMediaApi().saveSettings({ channel, model }); })();
  saves.set(channel, saved); void saved.catch(() => {});
  return saved;
}
export async function mediaModelsForTurn(): Promise<Partial<Record<MediaChannel, string>>> {
  const api = hostMediaApi();
  const channels: MediaChannel[] = ['images', 'video', 'tts', 'stt', 'embeddings'];
  for (;;) {
    const versions = new Map(saves);
    const values: Partial<Record<MediaChannel, string>> = {};
    for (const channel of channels) {
      await versions.get(channel);
      const choice = await api.getSettings({ channel });
      if (choice.model) values[channel] = choice.model;
    }
    if (channels.every(channel => versions.get(channel) === saves.get(channel))) return values;
  }
}
