import {request} from './api';
export type SourceMode='auto'|'frankcie'|'easypaper'|'pastpapers'|'papacambridge';
export type Settings={download_dir:string;merge_folders:boolean;source_mode?:SourceMode;download_concurrency?:number;requests_per_second?:number};
// Read before a partial edit so one settings panel cannot reset another panel's values.
let settingsWrite:Promise<unknown>=Promise.resolve();
let pendingSettingsWrites=0;
function serializeSettings<T>(work:()=>Promise<T>,publish?:(result:T)=>void):Promise<T>{
 const run=async()=>{try{const result=await work();publish?.(result);return result}finally{pendingSettingsWrites--}};
 const idle=pendingSettingsWrites++===0;
 const operation=idle?run():settingsWrite.catch(()=>{}).then(run);
 settingsWrite=operation;return operation;
}
export function saveSettings(changes:Partial<Settings>,publish?:(settings:Settings)=>void){
 return serializeSettings(async()=>{
  const current=await request<Settings>('/api/settings');
  return request<Settings>('/api/settings',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({...current,...changes})});
 },publish);
}
export type DirectoryChoice={cancelled:boolean;settings:Settings};
export function chooseDirectory(publish?:(result:DirectoryChoice)=>void){
 return serializeSettings(()=>request<DirectoryChoice>('/api/preferences/choose-directory',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'}),publish);
}
