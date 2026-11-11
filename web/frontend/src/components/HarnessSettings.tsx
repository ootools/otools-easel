import { useEffect, useState } from 'react';
import { fetchHarnessSettings, saveHarnessSettings } from '../lib/api';
import type { HarnessSettingsResponse } from '../lib/api';
import { hostAgentApi, selectedAgentTools, saveAgentTools } from '../lib/hostAgent';
import SettingsSelect from './SettingsSelect';

export default function HarnessSettings({ onChange, onSelect }: {onChange:(runtime:string)=>void;onSelect:(runtime:string)=>void}) {
  const [settings,setSettings]=useState<HarnessSettingsResponse|null>(null);
  const [runtime,setRuntime]=useState('api');
  const [sandbox,setSandbox]=useState('workspace-write');
  const [busy,setBusy]=useState(false);
  const [note,setNote]=useState('');
  const [refresh,setRefresh]=useState(0);
  const [tools,setTools]=useState<Array<{name:string;title?:string;description?:string}>>([]);
  const [selectedTools,setSelectedTools]=useState(selectedAgentTools);
  useEffect(()=>{
    let active=true;
    setBusy(true);
    fetchHarnessSettings().then(value=>{if(!active)return;setSettings(value);setRuntime(value.runtime);setSandbox(value.sandbox);onChange(value.runtime);onSelect(value.runtime);})
      .catch(error=>{if(active)setNote(String(error));}).finally(()=>{if(active)setBusy(false);});
    try {const api=hostAgentApi();if(api.tools)void api.tools().then(value=>{if(active)setTools(value);}).catch(error=>{if(active)setNote(String(error));});} catch(error){setNote(String(error));}
    return ()=>{active=false;};
  },[onChange,onSelect,refresh]);
  const selected=settings?.agents.find(agent=>agent.id===runtime);
  const save=async()=>{
    setBusy(true);setNote('');
    try {const value=await saveHarnessSettings(runtime,'',sandbox);setSettings(value);onChange(value.runtime);setNote('已保存，下个任务使用所选引擎；已有任务继续运行。');}
    catch(error){setNote(String(error));}finally{setBusy(false);}
  };
  const manage=async()=>{try {await hostAgentApi().openManager(runtime==='api'?{}:{providerId:runtime});}catch(error){setNote(String(error));}};
  return <section className="st-panel active harness-settings" aria-label="Agent 底座设置">
    <div className="panel-top"><strong>Agent 底座</strong><span className="spacer" />
      <button className="btn btn-sm" disabled={busy || !settings || settings.locked || !selected?.installed} onClick={()=>void save()}>保存底座</button>
      <button className="btn btn-sm" onClick={()=>void manage()}>管理 / 安装 Agent</button>
      <button className="btn btn-sm" disabled={busy} onClick={()=>setRefresh(value=>value+1)}>刷新</button>
    </div>
    <div className="settings-field"><span className="settings-field-label">执行引擎</span><div className="settings-field-body">
      <SettingsSelect label="执行引擎" value={runtime} disabled={busy || !settings || settings.locked}
        options={(settings?.agents || []).map(agent=>({value:agent.id,label:`${agent.label}${agent.installed?'':'（未安装 / 未配置）'}`}))}
        onChange={value=>{setRuntime(value);onSelect(value);}} />
      <p className="desc">仅聊天使用 Otools AI。Agent 的安装、认证、授权与后台运行由宿主管理。</p>
      {selected?.protocol==='acp' && <p className="desc">ACP 使用引擎自己的认证和会话配置，不套用宿主网关模型。会话配置在对话输入区读取。</p>}
    </div></div>
    {runtime!=='api' && <>{selected?.protocol==='codex' ? <div className="settings-field"><span className="settings-field-label">工作区权限</span><div className="settings-field-body">
      <SettingsSelect label="工作区权限" value={sandbox} disabled={busy || settings?.locked} options={[{value:'workspace-write',label:'工作区写入'},{value:'read-only',label:'只读'}]} onChange={setSandbox}/>
      <p className="desc">需要额外权限时打开宿主审批窗口，由用户确认。</p>
    </div></div> : <p className="desc">ACP 采用引擎原生权限策略；宿主不为其提供 Codex 沙箱或网络隔离。</p>}
    <div className="settings-field"><span className="settings-field-label">宿主工具</span><div className="settings-field-body">
      {tools.length===0 && <p className="desc">暂无已授权宿主工具。可在宿主管理中配置；Agent 自身的文件与终端工具不受此列表影响。</p>}
      {tools.map(tool=><label key={tool.name} className="desc" style={{display:'block'}} title={tool.description}>
        <input type="checkbox" checked={selectedTools.includes(tool.name)} onChange={event=>{const next=event.target.checked?[...selectedTools,tool.name]:selectedTools.filter(name=>name!==tool.name);saveAgentTools(next);setSelectedTools(next);}}/> {tool.title || tool.name}
      </label>)}
      <p className="desc">默认不附加宿主工具，选择后新任务生效。</p>
    </div></div></>}
    {note && <p role="status" className="save-note">{note}</p>}
  </section>;
}
