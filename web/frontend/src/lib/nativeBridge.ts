// Shared CMD transport for desktop plugins and the OTools remote runtime.
import type { HostAiApi } from './hostAi.ts';

export interface HostBridge {
  ai?: HostAiApi;
  invokeNative?: (method: string, payload?: unknown) => Promise<unknown>;
  invokeNativeRaw?: (method: string, payload?: unknown) => Promise<unknown>;
  listenNative?: (handler: (event: unknown) => void) => Promise<() => void | Promise<void>> | (() => void | Promise<void>);
}

export interface HostBridgeWindow {
  otools?: HostBridge;
  utools?: HostBridge;
  location?: { origin?: string };
  __TAURI_REMOTE_SERVICE__?: boolean;
  __OTOOLS_REMOTE_SERVICE__?: boolean;
  __TAURI_INTERNALS__?: { convertFileSrc?: (path: string, protocol: string) => string };
}

export function hostWindow(): HostBridgeWindow {
  return typeof window === 'undefined' ? {} : window as unknown as HostBridgeWindow;
}

export function otoolsBridge(): HostBridge | null {
  const target = hostWindow();
  return target.otools ?? target.utools ?? null;
}

export function throwIfAborted(signal?: AbortSignal | null): void {
  if (signal?.aborted) throw signal.reason ?? new DOMException('操作已取消', 'AbortError');
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

function abortable<T>(promise: Promise<T>, signal?: AbortSignal | null): Promise<T> {
  if (!signal) return promise;
  throwIfAborted(signal);
  return new Promise<T>((resolve, reject) => {
    const cancel = () => reject(signal.reason ?? new DOMException('操作已取消', 'AbortError'));
    signal.addEventListener('abort', cancel, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener('abort', cancel));
  });
}

export async function toBridgeBody(body?: BodyInit | null): Promise<unknown> {
  if (body == null) return undefined;
  if (typeof body === 'string') {
    try { return JSON.parse(body); } catch { return { raw: body }; }
  }
  if (body instanceof FormData) {
    const fields: Record<string, string> = {};
    const files: { field: string; name: string; contentBase64: string }[] = [];
    for (const [field, value] of body.entries()) {
      if (typeof value === 'string') fields[field] = value;
      else files.push({ field, name: value.name, contentBase64: bytesToBase64(new Uint8Array(await value.arrayBuffer())) });
    }
    return { __formData: true, fields, files };
  }
  if (body instanceof URLSearchParams) return Object.fromEntries(body.entries());
  return body;
}

function responseData<T>(value: unknown, raw: boolean): T {
  let data = value;
  if (raw && value && typeof value === 'object' && 'ok' in value) {
    const envelope = value as { ok: boolean; data?: unknown; error?: string; status?: number };
    if (!envelope.ok) throw Object.assign(new Error(envelope.error || '请求失败'), { status: envelope.status });
    data = envelope.data;
  }
  if (data && typeof data === 'object' && '__easelApiResponse' in data) {
    const response = data as { __easelApiResponse: boolean; status: number; body: unknown };
    if (response.status >= 400) {
      const detail = response.body as { detail?: string } | null;
      throw Object.assign(new Error(detail?.detail || `请求失败（${response.status}）`), { status: response.status });
    }
    data = response.body;
  }
  return data as T;
}

export async function nativeRequest<T = unknown>(path: string, options?: RequestInit): Promise<T> {
  throwIfAborted(options?.signal);
  const bridge = otoolsBridge();
  if (!bridge?.invokeNativeRaw && !bridge?.invokeNative) throw new Error('OTools 宿主桥不可用:请在 OTools 内打开本插件');
  const body = await toBridgeBody(options?.body);
  throwIfAborted(options?.signal);
  const payload = { method: (options?.method ?? 'GET').toUpperCase(), path, body };
  const raw = typeof bridge.invokeNativeRaw === 'function';
  const pending = raw ? bridge.invokeNativeRaw!('api', payload) : bridge.invokeNative!('api', payload);
  return responseData<T>(await abortable(pending, options?.signal), raw);
}
