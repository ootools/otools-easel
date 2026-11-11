export interface HostAiSelection {
  model: string | null;
  reasoningEffort: string | null;
}
export interface HostAiModelInfo {
  id: string;
  ownedBy: string;
  capabilities: string[];
  reasoningLevels: Array<{ effort: string; description: string }>;
  defaultReasoningLevel: string | null;
  contextWindow: number | null;
}
export interface HostAiModelCatalog {
  models: HostAiModelInfo[];
  defaultModel: string | null;
  selection: HostAiSelection;
}
export interface HostAiApi {
  models(): Promise<HostAiModelCatalog>;
  getSettings(): Promise<HostAiSelection>;
  saveSettings(settings: HostAiSelection): Promise<HostAiSelection>;
  capabilities(): Promise<{ configured: boolean; model?: string }>;
  consume?(request:{messages:Array<{role:'system'|'user'|'assistant';content:Array<{type:'text';text:string}>}>;model?:string;reasoningEffort?:string},options?:{signal?:AbortSignal;onEvent?:(event:{sequence:number;type:'delta';text:string}|{sequence:number;type:'done'|'cancelled'}|{sequence:number;type:'error';error:string})=>void}):Promise<{id:string;text:string}>;
}

export function hostAiApi(): HostAiApi {
  const target = typeof window === 'undefined' ? undefined : window as unknown as {
    otools?: { ai?: HostAiApi };
    utools?: { ai?: HostAiApi };
  };
  const api = target?.otools?.ai ?? target?.utools?.ai;
  if (!api?.models || !api.getSettings || !api.saveSettings || !api.capabilities) {
    throw new Error('请更新 Otools 宿主以使用统一模型和推理强度设置');
  }
  return api;
}

const listeners = new Set<(selection: HostAiSelection) => void>();
let pendingSave: Promise<HostAiSelection> | undefined;

export function subscribeHostAiSelection(listener: (selection: HostAiSelection) => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export async function fetchHostModelCatalog(): Promise<HostAiModelCatalog> {
  return hostAiApi().models();
}

export function saveHostModelSelection(settings: HostAiSelection): Promise<HostAiSelection> {
  const api = hostAiApi();
  const next = { model: settings.model, reasoningEffort: settings.reasoningEffort };
  const predecessor = pendingSave;
  pendingSave = (async () => {
    await predecessor?.catch(() => {});
    const saved = await api.saveSettings(next);
    for (const listener of listeners) {
      try { listener({ ...saved }); } catch { /* A view cannot invalidate a saved preference. */ }
    }
    return saved;
  })();
  // Keep a rejected save as the send gate until a later save succeeds.
  void pendingSave.catch(() => {});
  return pendingSave;
}

export async function resolveHostTurnSelection(): Promise<HostAiSelection> {
  const api = hostAiApi();
  for (;;) {
    const saving = pendingSave;
    await saving;
    const selection = await api.getSettings();
    const capabilities = await api.capabilities();
    if (saving !== pendingSave) continue;
    const model = selection.model;
    if (!capabilities.configured) throw new Error('请先在 Otools 中连接 AI 服务');
    if (!model?.trim()) throw new Error('请在输入框下选择模型');
    return { model, reasoningEffort: selection.reasoningEffort };
  }
}
