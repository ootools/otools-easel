import { bytesToBase64, nativeRequest, throwIfAborted } from './nativeBridge.ts';

interface UploadedFile { id: string; name: string; path: string }
export type UploadRequest = (path: string, options?: RequestInit) => Promise<unknown>;
const CHUNK_SIZE = 1024 * 1024;

export async function uploadViaChunks(files: File[], sessionId: string, local: boolean, request: UploadRequest = nativeRequest, signal?: AbortSignal): Promise<UploadedFile[]> {
  throwIfAborted(signal);
  const started = await request('/api/upload/start', {
    method: 'POST', signal,
    body: JSON.stringify({ sessionId, local, files: files.map(file => ({ name: file.name, size: file.size })) }),
  }) as { uploadId: string };
  if (!started || typeof started.uploadId !== 'string') throw new Error('上传任务响应无效');
  try {
    for (let index = 0; index < files.length; index++) {
      const file = files[index];
      for (let offset = 0; offset < file.size; offset += CHUNK_SIZE) {
        throwIfAborted(signal);
        const bytes = new Uint8Array(await file.slice(offset, offset + CHUNK_SIZE).arrayBuffer());
        throwIfAborted(signal);
        const written = await request('/api/upload/chunk', {
          method: 'POST', signal,
          body: JSON.stringify({ uploadId: started.uploadId, index, offset, contentBase64: bytesToBase64(bytes) }),
        }) as { received: number };
        if (written.received !== offset + bytes.length) throw new Error('上传块写入进度无效');
      }
    }
    const completed = await request('/api/upload/finish', { method: 'POST', signal, body: JSON.stringify({ uploadId: started.uploadId }) }) as { ok: boolean; files: UploadedFile[] };
    if (!completed.ok || !Array.isArray(completed.files)) throw new Error('上传结果无效');
    return completed.files;
  } catch (error) {
    await request('/api/upload/abort', { method: 'POST', body: JSON.stringify({ uploadId: started.uploadId }) }).catch(() => {});
    throw error;
  }
}
