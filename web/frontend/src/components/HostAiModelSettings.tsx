import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchHostModelCatalog, saveHostModelSelection, subscribeHostAiSelection } from '../lib/hostAi';
import type { HostAiModelCatalog, HostAiSelection } from '../lib/hostAi';
import SettingsSelect from './SettingsSelect';
import { hostAgentApi } from '../lib/hostAgent';

const EMPTY_SELECTION: HostAiSelection = { model: null, reasoningEffort: null };

export default function HostAiModelSettings({runtime='api'}:{runtime?:string}) {
  const [host,setHost]=useState(runtime==='api');
  const [note,setNote]=useState('读取执行引擎配置…');
  useEffect(()=>{
    let active=true;if(runtime==='api'){setHost(true);return;}
    setHost(false);
    try {void hostAgentApi().providers().then(providers=>{if(!active)return;const p=providers.find(p=>p.id===runtime);setHost(p?.adapter==='codex' && p.configuration!=='native');setNote(p?.adapter==='acp'?'ACP 使用引擎会话配置，请在对话中查看。':'此 Agent 使用原生模型配置，由 Otools Agent 管理维护。');}).catch(error=>{if(active)setNote(String(error));});}catch(error){setNote(String(error));}
    return ()=>{active=false;};
  },[runtime]);
  return host?<HostModelSettings/>:<p className="desc" role="status">{note}</p>;
}
function HostModelSettings() {
  const [catalog, setCatalog] = useState<HostAiModelCatalog | null>(null);
  const [selection, setSelection] = useState<HostAiSelection>(EMPTY_SELECTION);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [saveError, setSaveError] = useState('');
  const [note, setNote] = useState('');
  const request = useRef(0);

  const refresh = useCallback(async () => {
    const sequence = ++request.current;
    setLoading(true);
    setLoadError('');
    setSaveError('');
    setNote('');
    try {
      const value = await fetchHostModelCatalog();
      if (request.current !== sequence) return;
      setCatalog(value);
      setSelection(value.selection);
      if (!value.models.length) {
        setLoadError('宿主没有返回可用模型，请先在 Otools 设置中配置 AI 服务，再刷新模型列表。');
      }
    } catch (error) {
      if (request.current === sequence) {
        setLoadError(`读取失败：${error instanceof Error ? error.message : String(error)}`);
      }
    } finally {
      if (request.current === sequence) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    return () => { request.current += 1; };
  }, [refresh]);

  useEffect(() => subscribeHostAiSelection((value) => {
    setSelection(value);
    setCatalog(previous => previous ? { ...previous, selection: value } : previous);
    setSaveError('');
  }), []);

  const model = catalog?.models.find((item) => item.id === selection.model);
  const missingModel = selection.model !== null && !model;
  const levels = model?.reasoningLevels ?? [];
  const unsupportedReasoning = selection.reasoningEffort !== null
    && !levels.some((level) => level.effort === selection.reasoningEffort);
  const busy = loading || saving;
  const unavailable = !catalog || !!loadError;

  const save = async (next: HostAiSelection) => {
    const resetting = next.model === null && next.reasoningEffort === null;
    if (busy || (!resetting && (!catalog || loadError || missingModel || unsupportedReasoning))) return;
    const sequence = ++request.current;
    setSaving(true);
    setSaveError('');
    setNote('');
    try {
      const value = await saveHostModelSelection(next);
      if (request.current !== sequence) return;
      setSelection(value);
      setCatalog((previous) => previous ? { ...previous, selection: value } : previous);
      setNote('已保存 Easel 模型偏好，后续请求将使用此设置。');
    } catch (error) {
      if (request.current === sequence) {
        setSaveError(`保存失败：${error instanceof Error ? error.message : String(error)}`);
      }
    } finally {
      if (request.current === sequence) setSaving(false);
    }
  };

  const edit = (next: HostAiSelection) => {
    setSelection(next);
    setSaveError('');
    setNote('');
  };

  return (
    <section className="st-panel active harness-settings" aria-label="宿主模型设置" aria-busy={busy}>
      <div className="panel-top">
        <strong>宿主模型</strong>
        <span className="desc">Easel 专属偏好，由 Otools 保存</span>
        <span className="spacer" />
        <button type="button" className="btn btn-sm btn-primary"
          disabled={busy || unavailable || missingModel || unsupportedReasoning}
          onClick={() => void save(selection)}>{saving ? '保存中…' : '保存模型偏好'}</button>
        <button type="button" className="btn btn-sm" disabled={busy}
          onClick={() => void save(EMPTY_SELECTION)}>清除选择</button>
        <button type="button" className="btn btn-sm" disabled={busy}
          onClick={() => void refresh()}>{loading ? '读取中…' : '刷新模型列表'}</button>
      </div>
      <div className="settings-field">
        <span className="settings-field-label">模型</span>
        <div className="settings-field-body">
          <SettingsSelect label="宿主模型" value={selection.model ?? ''} disabled={busy || unavailable}
            options={[
              { value: '', label: '选择模型' },
              ...(catalog?.models ?? []).map((item) => ({ value: item.id, label: item.id })),
              ...(missingModel && selection.model
                ? [{ value: selection.model, label: `${selection.model}（当前不可用）` }] : []),
            ]}
            onChange={(value) => edit({ model: value || null, reasoningEffort: null })} />
          {missingModel && <p className="save-note err" role="alert">
            当前保存的模型 {selection.model} 不在宿主可用列表中。请选择可用模型，或刷新后再保存。
          </p>}
        </div>
      </div>
      <div className="settings-field">
        <span className="settings-field-label">推理强度</span>
        <div className="settings-field-body">
          <SettingsSelect label="推理强度" value={selection.reasoningEffort ?? ''} disabled={busy || unavailable || missingModel || selection.model === null}
            options={[
              { value: '', label: '由服务默认决定' },
              ...levels.map((level) => ({
                value: level.effort,
                label: level.description ? `${level.effort} · ${level.description}` : level.effort,
              })),
              ...(unsupportedReasoning && selection.reasoningEffort
                ? [{ value: selection.reasoningEffort, label: `${selection.reasoningEffort}（当前不可用）` }] : []),
            ]}
            onChange={(value) => edit({ ...selection, reasoningEffort: value || null })} />
          {!levels.length && !unavailable && !missingModel && <p className="desc">此模型未提供可选推理强度，由服务默认决定。</p>}
          {unsupportedReasoning && <p className="save-note err" role="alert">
            当前推理强度 {selection.reasoningEffort} 不在此模型支持的列表中，请重新选择或使用服务默认值。
          </p>}
        </div>
      </div>
      <p className="desc">仅聊天模式和 Codex 使用同一份 Easel 模型偏好，由 Otools 保存。模型服务和认证由 Otools 管理；清除选择会同时清除模型和推理强度选择。</p>
      <p className="desc">Codex 会保留会话中的推理强度；从指定强度清除选择时，请新建对话后继续。仅聊天模式下一次请求即可生效。</p>
      {loadError && <div className="save-note err" role="alert">{loadError}</div>}
      {saveError && <div className="save-note err" role="alert">{saveError}</div>}
      {note && <div className="save-note" role="status">{note}</div>}
    </section>
  );
}
