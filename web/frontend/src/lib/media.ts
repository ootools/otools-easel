import { hostWindow, nativeRequest, throwIfAborted } from './nativeBridge.ts';

export type MediaRequest = (path: string, options?: RequestInit) => Promise<unknown>;
export interface MediaLease { url: string; mimeType: string; release: () => void }
interface MediaInfo { absolutePath?: string; mimeType: string; size: number; mtime?: number; chunkSize: number }
interface MediaChunk { offset: number; contentBase64: string; eof: boolean }

export function mediaPath(value: string): string {
  const prefixed = value.startsWith('/api/media/');
  const path = prefixed ? decodeURIComponent(value.slice('/api/media/'.length).split(/[?#]/)[0]) : value;
  if (!path || path.startsWith('/') || path.includes('\\') || path.includes(':') || Array.from(path).some(character => character.charCodeAt(0) < 32)
      || path.split('/').some(segment => !segment || segment === '.' || segment === '..')) throw new Error('媒体路径无效');
  return path;
}

export function resolveEmbeddedPath(documentPath: string, value: string): string | null {
  const source = value.trim();
  if (!source || source.startsWith('#') || /^(https?:|data:|blob:)/i.test(source) || source.startsWith('//')) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(source)) throw new Error('媒体资源协议无效');
  if (source.startsWith('/api/media/')) return mediaPath(source);
  const relative = decodeURIComponent(source.split(/[?#]/)[0]);
  if (!relative) return null;
  if (relative.startsWith('/') || relative.includes('\\') || relative.includes(':')) throw new Error('媒体资源路径无效');
  const parts = mediaPath(documentPath).split('/').slice(0, -1);
  for (const part of relative.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') {
      if (!parts.length) throw new Error('媒体资源超出内容库范围');
      parts.pop();
    } else parts.push(part);
  }
  return mediaPath(parts.join('/'));
}

export interface HtmlPreview { source: string; release: () => void }

export async function prepareHtmlPreview(path: string, signal?: AbortSignal): Promise<HtmlPreview> {
  const logical = mediaPath(path);
  const html = await loadMediaText(logical, signal);
  const document = new DOMParser().parseFromString(html, 'text/html');
  document.querySelectorAll('base').forEach(element => element.remove());
  const leases: MediaLease[] = [];
  const loaded = new Map<string, Promise<string>>();
  const resource = async (parent: string, value: string, depth = 0): Promise<string> => {
    const relative = resolveEmbeddedPath(parent, value);
    if (!relative) return value;
    if (loaded.size >= 100 || depth > 4) throw new Error('预览资源过多或引用层级过深');
    let result = loaded.get(relative);
    if (!result) {
      result = (async () => {
        if (/\.css$/i.test(relative)) {
          const css = await loadMediaText(relative, signal);
          const converted = await rewriteCss(relative, css, depth + 1);
          const url = URL.createObjectURL(new Blob([converted], { type: 'text/css' }));
          leases.push({ url, mimeType: 'text/css', release: () => URL.revokeObjectURL(url) });
          return url;
        }
        const lease = await acquireMedia(relative, signal);
        leases.push(lease);
        return lease.url;
      })();
      loaded.set(relative, result);
    }
    return result;
  };
  const rewriteCss = async (parent: string, css: string, depth: number): Promise<string> => {
    const matches = [...css.matchAll(/url\(\s*(["']?)([^)"']+)\1\s*\)|@import\s+(["'])([^"']+)\3/g)];
    let result = css;
    for (const match of matches.reverse()) {
      const target = await resource(parent, (match[2] ?? match[4]).trim(), depth);
      const replacement = match[4] ? `@import "${target}"` : `url("${target}")`;
      result = result.slice(0, match.index) + replacement + result.slice(match.index! + match[0].length);
    }
    return result;
  };
  const release = () => { leases.splice(0).forEach(lease => lease.release()); };
  try {
    for (const element of document.querySelectorAll('[src], [poster], link[href], [style], style')) {
      for (const attribute of ['src', 'poster', ...(element.tagName === 'LINK' ? ['href'] : [])]) {
        const value = element.getAttribute(attribute);
        if (value) element.setAttribute(attribute, await resource(logical, value));
      }
      const style = element.getAttribute('style');
      if (style) element.setAttribute('style', await rewriteCss(logical, style, 0));
      if (element.tagName === 'STYLE') element.textContent = await rewriteCss(logical, element.textContent ?? '', 0);
    }
    throwIfAborted(signal);
    return { source: '<!doctype html>' + document.documentElement.outerHTML, release };
  } catch (error) {
    release();
    throw error;
  }
}

function route(path: string): string {
  return '/api/media/' + mediaPath(path).split('/').map(encodeURIComponent).join('/');
}

async function metadata(path: string, signal: AbortSignal | undefined, request: MediaRequest): Promise<MediaInfo> {
  throwIfAborted(signal);
  const value = await request(route(path), { signal }) as MediaInfo;
  throwIfAborted(signal);
  if (!value || !Number.isSafeInteger(value.size) || value.size < 0 || typeof value.mimeType !== 'string'
      || !Number.isSafeInteger(value.chunkSize) || value.chunkSize < 1) throw new Error('媒体信息无效');
  return value;
}

async function readChunks(path: string, info: MediaInfo, signal: AbortSignal | undefined, request: MediaRequest): Promise<Uint8Array[]> {
  const chunks: Uint8Array[] = [];
  let offset = 0;
  while (offset < info.size) {
    throwIfAborted(signal);
    const length = Math.min(info.chunkSize, 1024 * 1024, info.size - offset);
    const response = await request(`${route(path)}?offset=${offset}&length=${length}`, { signal }) as MediaChunk;
    throwIfAborted(signal);
    if (!response || response.offset !== offset || typeof response.contentBase64 !== 'string') throw new Error('媒体块响应无效');
    const decoded = atob(response.contentBase64);
    const bytes = Uint8Array.from(decoded, character => character.charCodeAt(0));
    if (bytes.length === 0 || bytes.length > length) throw new Error('媒体读取没有进度或块大小无效');
    chunks.push(bytes);
    offset += bytes.length;
    if (response.eof !== (offset === info.size)) throw new Error('媒体长度已变化，请重新加载');
  }
  return chunks;
}

export async function loadMediaText(path: string, signal?: AbortSignal, request: MediaRequest = nativeRequest): Promise<string> {
  const info = await metadata(path, signal, request);
  if (info.size > 20 * 1024 * 1024) throw new Error('预览文档过大，请下载后打开');
  const chunks = await readChunks(path, info, signal, request);
  const decoder = new TextDecoder('utf-8', { fatal: true });
  return chunks.map(chunk => decoder.decode(chunk, { stream: true })).join('') + decoder.decode();
}

export async function acquireMedia(path: string, signal?: AbortSignal, request: MediaRequest = nativeRequest): Promise<MediaLease> {
  const info = await metadata(path, signal, request);
  const host = hostWindow();
  if (info.absolutePath && (host.__TAURI_REMOTE_SERVICE__ || host.__OTOOLS_REMOTE_SERVICE__)) {
    const origin = host.location?.origin;
    if (!origin || !/^https?:\/\//.test(origin)) throw new Error('远程宿主地址无效');
    return { url: `${origin}/__tauri_remote_service_file__?path=${encodeURIComponent(info.absolutePath)}`, mimeType: info.mimeType, release: () => {} };
  }
  if (info.absolutePath && host.__TAURI_INTERNALS__?.convertFileSrc) {
    return { url: host.__TAURI_INTERNALS__.convertFileSrc(info.absolutePath, 'asset'), mimeType: info.mimeType, release: () => {} };
  }
  const chunks = await readChunks(path, info, signal, request);
  throwIfAborted(signal);
  const blob = new Blob(chunks.map(chunk => chunk.buffer as ArrayBuffer), { type: info.mimeType });
  const url = URL.createObjectURL(blob);
  let released = false;
  return { url, mimeType: info.mimeType, release: () => { if (!released) { released = true; URL.revokeObjectURL(url); } } };
}
