import {SourceAttempt} from './api';
import {sourceName} from './SourceSettings';
export function SourceAttempts({attempts,mode,source}:{attempts?:SourceAttempt[];mode?:string;source?:string}){
 if(!attempts?.length&&!source)return null;
 return <section className="source-provenance" aria-label="题源查询记录"><p>{mode&&<span>{mode==='auto'?'自动切换':'手动选择'} · </span>}{source?<strong>结果来自 {sourceName(source)}</strong>:<strong>没有可用的查询结果</strong>}</p>{attempts?.length?<details><summary>查看题源尝试（{attempts.length} 次）</summary><ol>{attempts.map((a,index)=><li key={a.source+index}><strong>{sourceName(a.source)}</strong><span className={'attempt-'+a.status}>{a.status==='ok'?`找到 ${a.file_count??'若干'} 份文件`:a.status==='empty'?'未找到文件':'查询失败'}</span>{a.error&&<p>{a.error}</p>}</li>)}</ol></details>:null}</section>
}
