import type { AudioHTMLAttributes, ImgHTMLAttributes, VideoHTMLAttributes, AnchorHTMLAttributes } from 'react';
import { useHtmlPreview, useMediaUrl } from '../lib/useMedia';
import { acquireMedia } from '../lib/media';

export function MediaImage({ path, version, ...props }: ImgHTMLAttributes<HTMLImageElement> & { path: string; version?: number }) {
  const media = useMediaUrl(path, version);
  return <img {...props} src={media.url || undefined} data-media-path={path} title={media.error || props.title} />;
}

export function MediaVideo({ path, version, ...props }: VideoHTMLAttributes<HTMLVideoElement> & { path: string; version?: number }) {
  const media = useMediaUrl(path, version);
  return <video {...props} src={media.url || undefined} data-media-path={path} title={media.error || props.title} />;
}

export function MediaAudio({ path, version, ...props }: AudioHTMLAttributes<HTMLAudioElement> & { path: string; version?: number }) {
  const media = useMediaUrl(path, version);
  return <audio {...props} src={media.url || undefined} data-media-path={path} title={media.error || props.title} />;
}

export function MediaFrame({ path, title, html = false, version, style }: { path: string; title: string; html?: boolean; version?: number; style?: React.CSSProperties }) {
  const media = useMediaUrl(html ? undefined : path, version);
  const preview = useHtmlPreview(html ? path : undefined, version);
  const error = html ? preview.error : media.error;
  if (error) return <div className="artifact-hint" role="status">预览加载失败：{error}</div>;
  if (html) return <iframe srcDoc={preview.source || undefined} title={title} sandbox="allow-scripts" allow="clipboard-write" style={style} />;
  return <iframe src={media.url || undefined} title={title} sandbox="" style={style} />;
}

export function MediaLink({ path, version, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { path: string; version?: number }) {
  const media = useMediaUrl(path, version);
  return <a {...props} href={media.url || undefined} title={media.error || props.title} aria-disabled={!media.url || undefined}
    onClick={event => { if (!media.url) event.preventDefault(); props.onClick?.(event); }} />;
}

export async function downloadMedia(path: string, filename: string) {
  const media = await acquireMedia(path);
  const link = document.createElement('a');
  link.href = media.url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(media.release, 60_000);
}
