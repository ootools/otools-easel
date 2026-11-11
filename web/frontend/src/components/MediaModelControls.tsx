import { useEffect, useRef, useState } from 'react';
import { hostMediaApi, saveMediaModel, type MediaCatalog, type MediaChannel } from '../lib/hostMedia';
export default function MediaModelControls({ channel, onSaved }: { channel: MediaChannel; onSaved?: () => void }) {
  const [catalog, setCatalog] = useState<MediaCatalog>();
  const [model, setModel] = useState('');
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [note, setNote] = useState('');
  const generation = useRef(0);
  async function refresh() {
    const current = ++generation.current; setLoading(true); setNote('');
    try { const next = await hostMediaApi().models({ channel }); if (current !== generation.current) return; setCatalog(next); setModel(next.selection.model ?? ''); }
    catch (error) { if (current === generation.current) setNote(error instanceof Error ? error.message : '模型列表读取失败'); }
    finally { if (current === generation.current) setLoading(false); }
  }
  useEffect(() => { setCatalog(undefined); setModel(''); void refresh(); return () => { generation.current++; }; }, [channel]);
  async function choose(value: string) {
    if (saving) return;
    const previous = model; const current = generation.current;
    setSaving(true); setModel(value); setNote('');
    try { const saved = await saveMediaModel(channel, value || null); if (current === generation.current) { setModel(saved.model ?? ''); setNote('已保存到 Easel · 下次任务生效'); onSaved?.(); } }
    catch (error) { if (current === generation.current) { setModel(previous); setNote(error instanceof Error ? error.message : '保存失败，请重试'); } }
    finally { if (current === generation.current) setSaving(false); }
  }
  return <section className="st-panel active" aria-busy={loading || saving}>
    <div className="panel-top"><span className="pill">AI Switch</span><span className="desc">地址与 Key 由宿主从插件自动获取，仅在这里选择模型。</span></div>
    <div className="board" style={{ padding: 16 }}>
      <label className="field-label" style={{ display: 'block', marginBottom: 8 }}>媒体模型
        <select className="field" aria-label="媒体模型" value={model} disabled={loading || saving || !catalog} onChange={event => void choose(event.target.value)} style={{ display: 'block', width: '100%', marginTop: 8 }}>
          <option value="">选择模型</option>
          {model && !catalog?.models.some(item => item.id === model) && <option value={model} disabled>{model}（不可用）</option>}
          {catalog?.models.map(item => <option key={item.id} value={item.id}>{item.id}</option>)}
        </select>
      </label>
      <button className="btn btn-sm" type="button" disabled={loading || saving} onClick={() => void refresh()}>{loading ? '读取中…' : '刷新模型'}</button>
      <p className="foot-note">只列出网关声明支持此通道的模型。未选择模型时不会使用宿主默认或自动挑选。</p>
      <p role="status" className="foot-note">{note || (saving ? '保存中…' : '')}</p>
    </div>
  </section>;
}
