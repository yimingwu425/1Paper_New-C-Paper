import {useEffect,useRef,useState} from 'react';
import {Download,FolderOpen} from 'lucide-react';
import {request,PaperFile,fileLabel} from './api';
export type Plan={token:string;files:(PaperFile&{destination:string})[];queries:{year:number;season:string;status:'ok'|'empty'|'failed';file_count:number;error?:string}[];complete:boolean;count:number;save_directory:string;merge_folders:boolean};
export function DownloadPlan({plan,onStarted}:{plan:Plan;onStarted:()=>void}){
 const [accept,setAccept]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(''),[uncertain,setUncertain]=useState(false);const lock=useRef(false),alive=useRef(true);const failed=plan.queries.filter(q=>q.status==='failed');
 useEffect(()=>{alive.current=true;return()=>{alive.current=false}},[]);
 useEffect(()=>{setAccept(false);setError('');setUncertain(false)},[plan.token]);
 async function start(){if(lock.current||uncertain||(!plan.complete&&!accept))return;lock.current=true;setBusy(true);setError('');try{const r=await request<{job:{id:string}}>('/api/jobs',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({preview_token:plan.token,accept_partial:accept})});if(!r.job?.id)throw new Error('队列响应缺少任务标识');if(alive.current)onStarted()}catch(e){if(alive.current){setUncertain(true);setError((e as Error).message+' 请先到下载管理核对是否已创建任务；此处不自动重发。')}}finally{if(alive.current)setBusy(false);lock.current=false}}
 return <section className="download-plan" aria-label="下载清单预览"><div className="section-heading"><h2>预览清单</h2><strong>{plan.count} 份文件</strong></div><p className="plan-path"><FolderOpen size={16}/><span>保存到：{plan.save_directory}</span></p><p className="quiet">{plan.merge_folders?'合并年份，平铺保存':'按年份与 QP / MS 分类保存'} · 这是本次任务锁定的保存位置</p>
 {failed.length>0&&<div className="query-failures" role="alert"><strong>{failed.length} 个年份 / 考季查询失败，当前清单不完整</strong>{failed.map(q=><p key={q.year+q.season}>{q.year} · {q.season}：{q.error||'查询失败'}</p>)}<label className="check-label"><input aria-label="仅下载已成功查询的部分" type="checkbox" checked={accept} disabled={busy||uncertain} onChange={e=>setAccept(e.target.checked)}/>我知道有遗漏，仅下载成功查询的 {plan.count} 份</label></div>}
 <details className="query-summary"><summary>查看所有查询结果（{plan.queries.length} 项）</summary>{plan.queries.map(q=><p key={q.year+q.season}>{q.year} · {q.season}：{q.status==='failed'?'失败':q.status==='empty'?'未找到文件':q.file_count+' 份'}{q.error?' · '+q.error:''}</p>)}</details>
 <div className="plan-files">{plan.files.map(f=><div key={f.id}><span>{fileLabel(f.kind)}</span><strong>{f.filename}</strong><small>{f.destination}</small></div>)}</div>
 {error&&<p className="precheck-error" role="alert">{error}</p>}<button className="primary" disabled={busy||uncertain||!plan.count||(!plan.complete&&!accept)} onClick={()=>void start()}><Download size={17}/>{busy?'正在创建任务…':uncertain?'请到下载管理核对任务':`开始下载 ${plan.count} 份`}</button></section>
}
