import { useCallback, useEffect, useRef, useState } from 'react';
import EnvBoard from './EnvBoard';
import HarnessSettings from './HarnessSettings';
import HostAiModelSettings from './HostAiModelSettings';
import MediaModelControls from './MediaModelControls';
import type { JobView } from './EnvBoard';
import { fetchEnvTools, startEnvInstall, fetchEnvJob, fetchProxySettings, saveProxySettings } from '../lib/api';
import type { EnvTool, ProxySettings } from '../lib/api';
import { IconSlidersHorizontal, IconPackage, IconEllipsis } from './settingsIcons';

interface Props { onClose: () => void; banner?: string }
type Sec = 'model' | 'env' | 'more';
type Chan = 'chat' | 'transcribe' | 'speech' | 'image' | 'video' | 'music';
const CHANNELS: { id: Chan; label: string }[] = [
  { id: 'chat', label: '对话与脚本' }, { id: 'transcribe', label: '语音转写' },
  { id: 'speech', label: '配音' }, { id: 'image', label: '生图' },
  { id: 'video', label: '视频' }, { id: 'music', label: '音乐' },
];
async function fetchWithRetry<T>(fn: () => Promise<T>, tries = 4, timeoutMs = 18000): Promise<T> {
  let lastErr: unknown = new Error('取数失败');
  for (let i = 0; i < tries; i += 1) {
    try {
      return await new Promise<T>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('请求超时')), timeoutMs);
        fn().then(value => { clearTimeout(timer); resolve(value); }, error => { clearTimeout(timer); reject(error); });
      });
    } catch (error) { lastErr = error; }
  }
  throw lastErr;
}

export default function SettingsPanel({ onClose, banner = '' }: Props) {
  const [sec, setSec] = useState<Sec>('model');
  const [chan, setChan] = useState<Chan>('chat');
  const [selectedHarness, setSelectedHarness] = useState('');
  const [tools, setTools] = useState<EnvTool[]>([]);
  const [python, setPython] = useState('');
  const [envLoading, setEnvLoading] = useState(true);
  const [envError, setEnvError] = useState('');
  const [jobs, setJobs] = useState<Record<string, JobView>>({});
  const runningRef = useRef(false);
  const batchMode = useRef(false);
  const refreshEnv = useCallback(async (force = false) => {
    setEnvLoading(true); setEnvError('');
    try { const value = await fetchWithRetry(() => fetchEnvTools(force)); setTools(value.tools || []); setPython(value.python || ''); }
    catch (error) { setEnvError(error instanceof Error ? `环境体检失败：${error.message}` : '环境体检失败'); }
    finally { setEnvLoading(false); }
  }, []);
  useEffect(() => { if (sec === 'env') void refreshEnv(); }, [sec, refreshEnv]);
  const settleJobs = useCallback(async () => {
    await refreshEnv(true);
    setJobs(current => Object.fromEntries(Object.entries(current).filter(([, job]) => job.state === 'fail')));
  }, [refreshEnv]);
  const pollJob = useCallback((id: string, jobId: string) => new Promise<void>(resolve => {
    let failures = 0;
    let checking = false;
    const timer = setInterval(async () => {
      if (checking) return;
      checking = true;
      try {
        const state = await fetchEnvJob(jobId);
        const last = (state.lines || [])[state.lines.length - 1] || '';
        setJobs(current => ({ ...current, [id]: { state: state.state, line: last.trim().slice(0, 140), detail: state.result?.detail || null } }));
        if (state.state !== 'running') { clearInterval(timer); resolve(); }
      } catch { failures += 1; if (failures >= 3) { clearInterval(timer); resolve(); } }
      finally { checking = false; }
    }, 1500);
  }), []);
  const installOne = useCallback(async (id: string) => {
    setJobs(current => ({ ...current, [id]: { state: 'running', line: '启动中…' } }));
    try { const { jobId } = await startEnvInstall(id); await pollJob(id, jobId); }
    catch (error) { setJobs(current => ({ ...current, [id]: { state: 'fail', line: '', detail: error instanceof Error ? error.message : '启动失败' } })); }
    if (!batchMode.current) await settleJobs();
  }, [pollJob, settleJobs]);
  const installMany = useCallback(async (ids: string[]) => {
    runningRef.current = true; batchMode.current = true;
    setJobs(current => ({ ...current, ...Object.fromEntries(ids.map(id => [id, { state: 'running', line: '排队中…' }])) }));
    for (const id of ids) await installOne(id);
    batchMode.current = false; runningRef.current = false; await settleJobs();
  }, [installOne, settleJobs]);
  const anyRunning = runningRef.current || Object.values(jobs).some(job => job.state === 'running');
  const okCount = tools.filter(tool => tool.state === 'ok').length;
  const total = tools.length;

  const [proxyCfg, setProxyCfg] = useState<ProxySettings | null>(null);
  const [proxyInput, setProxyInput] = useState('');
  const [proxyBusy, setProxyBusy] = useState(false);
  const [proxyMsg, setProxyMsg] = useState('');
  const [proxyErr, setProxyErr] = useState('');
  useEffect(() => {
    if (sec !== 'more') return;
    let active = true;
    fetchProxySettings().then(value => { if (active) { setProxyCfg(value); setProxyInput(value.proxy); setProxyErr(''); setProxyMsg(''); } })
      .catch(() => { if (active) setProxyErr('读取代理配置失败'); });
    return () => { active = false; };
  }, [sec]);
  const saveProxy = useCallback(async (value?: string) => {
    const proxy = (value ?? proxyInput).trim();
    setProxyBusy(true); setProxyErr(''); setProxyMsg('');
    try {
      const saved = await saveProxySettings(proxy);
      setProxyCfg({ proxy: saved.proxy, source: (saved.source as ProxySettings['source']) || 'none' });
      setProxyInput(saved.proxy);
      setProxyMsg(saved.proxy ? `已保存：${saved.proxy} —— 立即生效，无需重启` : '已清除代理配置');
    } catch (error) { setProxyErr(error instanceof Error ? error.message : '保存失败'); }
    finally { setProxyBusy(false); }
  }, [proxyInput]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape' && !event.defaultPrevented) onClose(); };
    window.addEventListener('keydown', onKey); return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  const localReady = tools.find(tool => tool.id === 'fw')?.state === 'ok' && tools.find(tool => tool.id === 'model')?.state === 'ok';

  return <div className="settings-overlay" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="settings-panel" role="dialog" aria-modal="true" aria-label="设置">
      <div className="settings-head">
        <div><h2 className="settings-title">设置</h2><div className="settings-sub">模型选择 · 环境安装 · 更多设置</div></div>
        <div className="settings-actions"><button className="settings-close" onClick={onClose} title="关闭（Esc）">✕</button></div>
      </div>
      {banner && <div role="status" style={{ margin: '0 16px 12px', padding: '10px 12px', borderRadius: 8, background: 'var(--warn-bg, #fff7ed)', color: 'var(--warn-fg, #9a3412)', border: '1px solid var(--warn-border, #fed7aa)', fontSize: 13, lineHeight: 1.5 }}>{banner}</div>}
      <div className="settings-body">
        <nav className="settings-nav">
          <button className={`snav${sec === 'model' ? ' active' : ''}`} onClick={() => setSec('model')}><IconSlidersHorizontal size={16} />模型选择<small>插件内记忆</small></button>
          <button className={`snav${sec === 'env' ? ' active' : ''}`} onClick={() => setSec('env')}><IconPackage size={16} />环境安装<small>{total ? (okCount === total ? '全就绪' : `${okCount}/${total}`) : '…'}</small></button>
          <button className={`snav${sec === 'more' ? ' active' : ''}`} onClick={() => setSec('more')}><IconEllipsis size={16} />更多设置</button>
        </nav>
        <div className="settings-main">
          {sec === 'model' && <section className="st-sec active">
            <div className="tabbar">{CHANNELS.map(channel => <button key={channel.id} className={`tab${chan === channel.id ? ' active' : ''}`} onClick={() => setChan(channel.id)}><span className="cdot" />{channel.label}</button>)}</div>
            {chan === 'chat' && <><HarnessSettings onChange={setSelectedHarness} onSelect={setSelectedHarness} /><HostAiModelSettings runtime={selectedHarness || 'api'} /></>}
            {chan === 'image' && <MediaModelControls channel="images" />}
            {chan === 'video' && <MediaModelControls channel="video" />}
            {chan === 'speech' && <MediaModelControls channel="tts" />}
            {chan === 'transcribe' && <><MediaModelControls channel="stt" /><p className="foot-note">字幕与本地 Whisper 流程保留；本地能力请到“环境安装”检测和安装。{localReady ? '本地组件已就绪。' : ''}</p></>}
            {chan === 'music' && <div className="board"><div className="empty">当前宿主未提供音乐生成接口，不会使用其他通道替代。</div></div>}
          </section>}
          {sec === 'env' && <section className="st-sec active"><EnvBoard tools={tools} python={python} loading={envLoading} error={envError} jobs={jobs} anyRunning={anyRunning} onRefresh={() => void refreshEnv(true)} onInstall={installOne} onInstallMany={installMany} /></section>}
          {sec === 'more' && <section className="st-sec active">
            <div className="panel-top"><span className={`pill ${proxyCfg?.proxy ? 'ok' : 'off'}`}><span className="dot" />{proxyCfg?.proxy ? '代理已配置' : '代理未配置'}</span><span className="desc">网络代理与 AI Switch 上游账号设置分别管理</span></div>
            <div className="board" style={{ padding: 14 }}>
              <label className="field-label" style={{ display: 'block', marginBottom: 6 }}>网络代理（EASEL_PROXY）</label>
              <input className="field" style={{ width: '100%', boxSizing: 'border-box' }} value={proxyInput} disabled={proxyCfg?.source === 'env' || proxyBusy} onChange={event => { setProxyInput(event.target.value); setProxyErr(''); setProxyMsg(''); }} placeholder="http://127.0.0.1:7890 或 socks5://127.0.0.1:1080" />
              {proxyCfg?.source === 'env' && <div className="foot-note">启动环境已固定代理；修改请调整启动环境后重启。</div>}
              {proxyErr && <div className="env-error">{proxyErr}</div>}
              {proxyMsg && <div className="foot-note">{proxyMsg}</div>}
              <div style={{ display: 'flex', gap: 8, marginTop: 10 }}>
                <button className="btn btn-sm btn-primary" disabled={proxyBusy || proxyCfg?.source === 'env'} onClick={() => void saveProxy()}>{proxyBusy ? '保存中…' : '保存'}</button>
                {proxyCfg?.proxy && proxyCfg.source === 'envfile' && <button className="btn btn-sm btn-ghost" disabled={proxyBusy} onClick={() => { setProxyInput(''); void saveProxy(''); }}>清除代理</button>}
              </div>
              <div className="foot-note">保存后用于后续登录和发布子进程；当前：{proxyCfg?.proxy || '未配置，回退系统代理'}</div>
            </div>
          </section>}
        </div>
      </div>
      <div className="settings-foot">模型选择仅保存为 Easel 偏好；地址与 Key 从 AI Switch 插件自动获取。切换执行引擎后请新建对话。</div>
    </div>
  </div>;
}
