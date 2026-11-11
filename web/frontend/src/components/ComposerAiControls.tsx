import { useCallback, useEffect, useRef, useState } from 'react';
import {
  fetchHostModelCatalog, hostAiApi, saveHostModelSelection, subscribeHostAiSelection,
} from '../lib/hostAi';
import type { HostAiModelCatalog, HostAiSelection } from '../lib/hostAi';
import SettingsSelect from './SettingsSelect';
import { IconRefresh } from './icons';
import { hostAgentApi, type AgentConfigOption } from '../lib/hostAgent';
import { loadHostTasks } from '../lib/store';

const FOLLOW_DEFAULT: HostAiSelection = { model: null, reasoningEffort: null };
const EFFORT_LABELS: Record<string, string> = {
  none: '无', minimal: '最少', low: '低', medium: '中', high: '高', xhigh: '极高', max: '最高',
};

export default function ComposerAiControls({runtime='codex',sessionId}:{runtime?:string;sessionId?:string}) {
  const [mode,setMode]=useState<'loading'|'host'|'native'|'acp'|'error'>(runtime==='api'?'host':'loading');
  useEffect(()=>{
    let active=true;if(runtime==='api'){setMode('host');return;}
    setMode('loading');
    try {void hostAgentApi().providers().then(providers=>{if(!active)return;const provider=providers.find(p=>p.id===runtime);setMode(!provider?'error':provider.adapter==='acp'?'acp':provider.configuration==='native'?'native':'host');}).catch(()=>{if(active)setMode('error');});}catch{setMode('error');}
    return ()=>{active=false;};
  },[runtime]);
  if(mode==='host')return <HostModelControls/>;
  if(mode==='acp')return <AcpControls sessionId={sessionId}/>;
  return <div className="composer-ai-controls"><span role="status" className="composer-ai-status">{mode==='native'?'此 Codex 使用原生配置，模型和推理强度由引擎默认设置决定。':mode==='loading'?'读取 Agent 配置…':'Agent 配置不可用，请更新或配置 Otools 宿主。'}</span></div>;
}
function AcpControls({sessionId}:{sessionId?:string}) {
  const [options,setOptions]=useState<AgentConfigOption[]>([]);
  const [note,setNote]=useState('开始 Agent 会话后可读取引擎配置；当前使用引擎原生默认。');
  const [busy,setBusy]=useState(false);
  const task=Object.values(loadHostTasks()).filter(task=>task.sessionId===sessionId).sort((a,b)=>b.created-a.created)[0];
  const refresh=async()=>{
    if(!task)return;setBusy(true);
    try {const api=hostAgentApi();if(!api.configOptions)throw new Error('请更新 Otools 以读取 ACP 会话配置');const value=await api.configOptions({sessionId:task.hostSessionId});setOptions(value.options);setNote(value.options.length?'引擎会话配置':'此引擎未提供可选模型或推理配置，使用其原生默认。');}
    catch(error){setNote(String(error));}finally{setBusy(false);}
  };
  useEffect(()=>{void refresh();},[task?.hostSessionId]);
  return <div className="composer-ai-controls" role="group" aria-label="ACP 会话配置">
    {options.map(option=><SettingsSelect key={option.id} label={option.name} value={option.currentValue} disabled={busy || task?.status==='running'} options={option.options.map(item=>({value:item.value,label:item.name}))} onChange={value=>{
      if(!task)return;setBusy(true);void hostAgentApi().setConfigOption({sessionId:task.hostSessionId,configId:option.id,value}).then(result=>setOptions(result.options)).catch(error=>setNote(String(error))).finally(()=>setBusy(false));
    }}/>)}
    <button type="button" className="composer-ai-refresh" disabled={busy || !task} onClick={()=>void refresh()}>读取引擎配置</button>
    <span role="status" className="composer-ai-status">{note}</span>
  </div>;
}
function HostModelControls() {
  const [catalog, setCatalog] = useState<HostAiModelCatalog | null>(null);
  const [selection, setSelection] = useState<HostAiSelection>(FOLLOW_DEFAULT);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [failedSelection, setFailedSelection] = useState<HostAiSelection | null>(null);
  const [note, setNote] = useState('');
  const mounted = useRef(false);
  const ready = useRef(false);
  const savingRef = useRef(false);
  const savedSelection = useRef<HostAiSelection>(FOLLOW_DEFAULT);
  const request = useRef(0);
  const selectionVersion = useRef(0);

  const refresh = useCallback(async () => {
    if (savingRef.current) return;
    const sequence = ++request.current;
    const version = selectionVersion.current;
    ready.current = false;
    setLoading(true);
    setLoadError('');
    let hostAvailable = false;
    try {
      hostAiApi();
      hostAvailable = true;
      const value = await fetchHostModelCatalog();
      if (!mounted.current || sequence !== request.current) return;
      setCatalog(value);
      // A preference saved while the catalog was loading is newer than its snapshot.
      if (version === selectionVersion.current) {
        savedSelection.current = value.selection;
        setSelection(value.selection);
      }
      ready.current = true;
      if (!value.models.length) {
        setLoadError('请先在 Otools 设置中配置 AI 服务');
      }
    } catch {
      if (!mounted.current || sequence !== request.current) return;
      setLoadError(hostAvailable ? '模型读取失败，请刷新重试' : '请在 Otools 中打开并更新宿主');
    } finally {
      if (mounted.current && sequence === request.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    const unsubscribe = subscribeHostAiSelection((value) => {
      selectionVersion.current += 1;
      savedSelection.current = value;
      setSelection(value);
      setFailedSelection(null);
      setNote('已保存 · 下条消息生效');
    });
    void refresh();
    const onFocus = async () => {
      if (!ready.current || savingRef.current) return;
      const version = ++selectionVersion.current;
      try {
        const value = await hostAiApi().getSettings();
        if (!mounted.current || version !== selectionVersion.current || savingRef.current) return;
        savedSelection.current = value;
        setSelection(value);
      } catch {
        // Keep the last confirmed selection; explicit refresh reports host errors.
      }
    };
    window.addEventListener('focus', onFocus);
    return () => {
      mounted.current = false;
      ready.current = false;
      request.current += 1;
      selectionVersion.current += 1;
      unsubscribe();
      window.removeEventListener('focus', onFocus);
    };
  }, [refresh]);

  const save = async (next: HostAiSelection) => {
    if (savingRef.current || !ready.current) return;
    const restoringDefault = selection.reasoningEffort !== null && next.reasoningEffort === null;
    savingRef.current = true;
    selectionVersion.current += 1;
    setSaving(true);
    setSelection(next);
    setFailedSelection(null);
    setNote('');
    try {
      const value = await saveHostModelSelection(next);
      if (!mounted.current) return;
      savedSelection.current = value;
      setSelection(value);
      setNote(restoringDefault ? '已保存 · Codex 恢复默认需新对话' : '已保存 · 下条消息生效');
    } catch {
      if (!mounted.current) return;
      setSelection(savedSelection.current);
      setFailedSelection(next);
    } finally {
      savingRef.current = false;
      if (mounted.current) setSaving(false);
    }
  };

  const model = catalog?.models.find((item) => item.id === selection.model);
  const missingModel = !!catalog && selection.model !== null && !model;
  const levels = model?.reasoningLevels ?? [];
  const unsupportedEffort = !!catalog && selection.reasoningEffort !== null
    && !levels.some((level) => level.effort === selection.reasoningEffort);
  const busy = loading || saving;
  const disabled = busy || !catalog || !!loadError;
  const defaultModelLabel = '选择模型';
  const status = saving ? '保存中…' : failedSelection ? '保存失败，请重试后再发送'
    : loading ? '读取中…' : loadError || (missingModel ? '当前模型不可用，请切换或刷新'
      : unsupportedEffort ? '当前推理程度不可用，请重新选择' : note || '下条消息生效');
  const error = !busy && (!!failedSelection || !!loadError || missingModel || unsupportedEffort);

  return (
    <div className="composer-ai-controls" role="group" aria-label="消息模型设置" aria-busy={busy}>
      <SettingsSelect label="模型" className="composer-ai-model" value={selection.model ?? ''}
        title={`模型：${selection.model ?? defaultModelLabel} · 自动保存，下条消息生效`}
        disabled={disabled}
        options={[
          { value: '', label: !catalog && loading ? '读取模型…' : defaultModelLabel },
          ...(catalog?.models ?? []).map((item) => ({ value: item.id, label: item.id })),
          ...(missingModel && selection.model
            ? [{ value: selection.model, label: `${selection.model}（不可用）` }] : []),
        ]}
        onChange={(value) => {
          if ((value || null) !== selection.model) void save({ model: value || null, reasoningEffort: null });
        }} />
      <SettingsSelect label="推理程度" className="composer-ai-reasoning" value={selection.reasoningEffort ?? ''}
        title={selection.reasoningEffort === null
          ? `推理程度由服务决定${model?.defaultReasoningLevel ? ` · 服务建议 ${model.defaultReasoningLevel}` : ''} · Codex 恢复默认需新对话`
          : `推理程度：${selection.reasoningEffort} · 自动保存，下条消息生效`}
        disabled={disabled || missingModel || (!levels.length && selection.reasoningEffort === null)}
        options={[
          { value: '', label: '推理 · 默认' },
          ...levels.map((level) => ({ value: level.effort, label: `推理 · ${EFFORT_LABELS[level.effort] || level.effort}` })),
          ...(unsupportedEffort && selection.reasoningEffort
            ? [{ value: selection.reasoningEffort, label: `${selection.reasoningEffort}（不可用）` }] : []),
        ]}
        onChange={(value) => {
          if ((value || null) !== selection.reasoningEffort) void save({ ...selection, reasoningEffort: value || null });
        }} />
      <button type="button" className="composer-ai-refresh" aria-label="刷新模型列表" title="刷新模型列表"
        disabled={busy} onClick={() => void refresh()}><IconRefresh size={14} /></button>
      <span className={`composer-ai-status${error ? ' is-error' : ''}`} role="status" aria-live="polite">{status}</span>
      {failedSelection && <button type="button" className="composer-ai-retry" disabled={disabled}
        onClick={() => void save(failedSelection)}>重试</button>}
    </div>
  );
}
