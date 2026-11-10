import {useEffect,useRef,useState} from 'react';
import {Settings,SourceMode,saveSettings} from './settings';
export const sourceName=(source:string)=>({auto:'自动切换',frankcie:'FrankCIE',easypaper:'EasyPaper',pastpapers:'PastPapers',papacambridge:'PapaCambridge'}[source]||source);
export function SourceSettings({settings,onSaved}:{settings:Settings|null;onSaved:(settings:Settings)=>void}){
 const [mode,setMode]=useState<SourceMode>(settings?.source_mode??'auto');
 const [concurrency,setConcurrency]=useState(String(settings?.download_concurrency??4));
 const [rate,setRate]=useState(String(settings?.requests_per_second??5));
 const [busy,setBusy]=useState(false),[error,setError]=useState(''),[saved,setSaved]=useState(false);
 const lock=useRef(false),alive=useRef(true);
 useEffect(()=>{alive.current=true;return()=>{alive.current=false}},[]);
 useEffect(()=>{setMode(settings?.source_mode??'auto');setConcurrency(String(settings?.download_concurrency??4));setRate(String(settings?.requests_per_second??5))},[settings?.source_mode,settings?.download_concurrency,settings?.requests_per_second]);
 const valid=/^\d+$/.test(concurrency)&&Number(concurrency)>=1&&Number(concurrency)<=16&&/^\d+$/.test(rate)&&Number(rate)>=1&&Number(rate)<=20;
 const dirty=mode!==(settings?.source_mode??'auto')||Number(concurrency)!==(settings?.download_concurrency??4)||Number(rate)!==(settings?.requests_per_second??5);
 function changed(){setSaved(false);setError('')}
 async function save(){
  if(lock.current||!settings||!valid||!dirty)return;lock.current=true;setBusy(true);setError('');setSaved(false);
  try{await saveSettings({source_mode:mode,download_concurrency:Number(concurrency),requests_per_second:Number(rate)},result=>{onSaved(result);if(alive.current)setSaved(true)})}
  catch(e){if(alive.current)setError((e as Error).message)}
  finally{lock.current=false;if(alive.current)setBusy(false)}
 }
 return <section className="source-settings" aria-label="题源与下载节奏"><div className="section-heading"><div><h2>题源与下载节奏</h2><p className="quiet">找不到时有后备来源，也给上游站点留一点余地。</p></div><span className="settings-badge">新任务生效</span></div><fieldset disabled={!settings||busy}><label className="source-select">查询题源<select aria-label="查询题源" value={mode} onChange={e=>{changed();setMode(e.target.value as SourceMode)}}>{(['auto','frankcie','easypaper','pastpapers','papacambridge'] as SourceMode[]).map(value=><option key={value} value={value}>{sourceName(value)}</option>)}</select></label><p className="source-explanation">{mode==='auto'?'按顺序尝试 FrankCIE → EasyPaper → PastPapers → PapaCambridge；空结果或查询失败时尝试下一个。每次查询都会保留尝试记录。':`只查询 ${sourceName(mode)}，不会自动改用其他来源。没有结果时，可切回「自动切换」再查询。`}</p><div className="transfer-settings"><label>同时下载文件数<input aria-label="同时下载文件数" type="number" min="1" max="16" step="1" value={concurrency} onChange={e=>{changed();setConcurrency(e.target.value)}}/><small>1–16 个，建议 4 个</small></label><label>每秒请求上限<input aria-label="每秒请求上限" type="number" min="1" max="20" step="1" value={rate} onChange={e=>{changed();setRate(e.target.value)}}/><small>1–20 次/秒，建议 5 次/秒</small></label></div></fieldset><p className="quiet">请求频率不是下载带宽。遇到站点限流时，队列会显示等待状态；提高并发不一定更快。</p><p className="quiet">应用后需重新查询或预览。已创建的任务保留原设置，不会自动重启或重试。</p>{!valid&&<p className="precheck-error" role="alert">请输入范围内的整数：并发 1–16，请求频率 1–20 次/秒。</p>}{error&&<p className="precheck-error" role="alert">保存失败：{error}。修改仍保留，可再试一次。</p>}{saved&&<p className="settings-success" role="status">题源与下载设置已保存。</p>}<button className="secondary" disabled={!settings||busy||!valid||!dirty} onClick={()=>void save()}>{busy?'正在应用…':'应用题源与下载设置'}</button></section>
}
