import {useEffect,useState} from 'react';
import {request,Subject} from './api';
export function useCatalogue(sourceMode='auto'){
 const [subjects,setSubjects]=useState<Subject[]>([]),[source,setSource]=useState(''),[error,setError]=useState(''),[loading,setLoading]=useState(true),[revision,setRevision]=useState(0);
 useEffect(()=>{const abort=new AbortController();let active=true;setLoading(true);setError('');
 request<{subjects:Subject[];source?:string}>('/api/catalog/subjects',{signal:abort.signal}).then(r=>{if(!active)return;if(!Array.isArray(r.subjects))throw new Error('题源目录响应格式不正确');setSubjects(r.subjects);setSource(r.source??'')}).catch(e=>{if(active){setError((e as Error).message);setSubjects([]);setSource('')}}).finally(()=>{if(active)setLoading(false)});
 return()=>{active=false;abort.abort()};},[revision,sourceMode]);
 return {subjects,source,error,loading,reload:()=>setRevision(v=>v+1)};
}
