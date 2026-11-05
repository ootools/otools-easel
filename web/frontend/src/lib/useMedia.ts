import { useEffect, useState } from 'react';
import { acquireMedia, prepareHtmlPreview } from './media.ts';

export function useMediaUrl(path: string | undefined, version?: number) {
  const [value, setValue] = useState({ path: '', url: '', error: '' });
  useEffect(() => {
    const controller = new AbortController();
    let release: (() => void) | undefined;
    if (!path) return;
    void acquireMedia(path, controller.signal).then(lease => {
      if (controller.signal.aborted) { lease.release(); return; }
      release = lease.release;
      setValue({ path, url: lease.url, error: '' });
    }).catch(error => {
      if (!controller.signal.aborted) setValue({ path, url: '', error: error instanceof Error ? error.message : '预览加载失败' });
    });
    return () => { controller.abort(); release?.(); };
  }, [path, version]);
  return value.path === path ? value : { path: path ?? '', url: '', error: '' };
}

export function useHtmlPreview(path: string | undefined, version?: number) {
  const [value, setValue] = useState({ path: '', source: '', error: '' });
  useEffect(() => {
    const controller = new AbortController();
    let release: (() => void) | undefined;
    if (!path) return;
    void prepareHtmlPreview(path, controller.signal).then(preview => {
      if (controller.signal.aborted) { preview.release(); return; }
      release = preview.release;
      setValue({ path, source: preview.source, error: '' });
    }).catch(error => {
      if (!controller.signal.aborted) setValue({ path, source: '', error: error instanceof Error ? error.message : '预览加载失败' });
    });
    return () => { controller.abort(); release?.(); };
  }, [path, version]);
  return value.path === path ? value : { path: path ?? '', source: '', error: '' };
}
