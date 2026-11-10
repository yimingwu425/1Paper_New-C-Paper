export type PaperFile={id:string;filename:string;kind:string;source_url:string};
export type PaperGroup={id:string;number:string;files:PaperFile[]};
export type SearchResult={source:string;groups:PaperGroup[];warnings?:string[]};
export async function request<T>(path:string,init?:RequestInit):Promise<T>{
 const response=await fetch(path,init); const body=await response.text();
 let data;try{data=JSON.parse(body)}catch{throw new Error(response.ok?'服务返回了非 JSON 内容':`服务暂不可用（${response.status}）`)}
 if(!response.ok)throw new Error(data.error||data.message||`请求失败（${response.status}）`);return data as T;
}
export type Subject={code:string;name:string};
export function resolveSubject(query:string,catalogue:Subject[]=[]):Subject|null{const q=query.trim().toLowerCase();return catalogue.find(s=>s.code===q||s.name.toLowerCase()===q)||( /^\d{4}$/.test(q)?{code:q,name:'科目 '+q}:null)}
export const fileLabel=(kind:string)=>({qp:'题卷',ms:'评分标准',ci:'保密说明（CI）',gt:'分数线（GT）',er:'考官报告（ER）',in:'附页（IN）',ir:'其他资料（IR）',sr:'其他资料（SR）'}[kind.toLowerCase()]||'其他资料（'+kind.toUpperCase()+'）');
