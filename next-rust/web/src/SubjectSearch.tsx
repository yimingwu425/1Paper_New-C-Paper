import {useEffect, useRef, useState} from 'react';
import {ArrowRight, Check, RefreshCw, Search} from 'lucide-react';
import {request, resolveSubject, Subject} from './api';
import {useCatalogue} from './useCatalogue';
import {sourceName} from './SourceSettings';
import {SubjectFlow} from './SubjectFlow';
export type SearchQuery = {subject: Subject; year: string; season: string};
type Season = {season: string; status: 'available' | 'empty' | 'unknown'; file_count: number; error?: string};
export const seasonName = (value: string) => ({Mar: '2–3 月', Jun: '5–6 月', Nov: '10–11 月'}[value] || value);
export function SubjectSearch({loading, compact, onSearch, onInvalidate, onSelected, initialSubject, sourceMode='auto'}: {
  loading: boolean; compact: boolean; onSearch: (query: SearchQuery) => void; onInvalidate: () => void; onSelected?:(s:Subject|null)=>void; initialSubject?:Subject|null; sourceMode?:string;
}) {
  const {subjects:catalogue,source:catalogueSource,error:catalogueError,reload:reloadCatalogue}=useCatalogue(sourceMode);
  const [query, setQuery] = useState(''), [subject, setSubject] = useState<Subject | null>(null);
  const [year, setYear] = useState(String(new Date().getFullYear())), [season, setSeason] = useState('');
  const [seasons, setSeasons] = useState<Season[]>([]), [checking, setChecking] = useState(false), [error, setError] = useState('');
  const [retry, setRetry] = useState(0),[manualSeason,setManualSeason]=useState(false);
  const availabilityId = useRef(0), controller = useRef<AbortController | null>(null);
  const yearInput = useRef<HTMLInputElement>(null), focusYear = useRef(false);
  useEffect(() => {if(subject && focusYear.current){yearInput.current?.focus();focusYear.current=false}}, [subject]);
  useEffect(()=>{if(initialSubject){setSubject({...initialSubject});setQuery(initialSubject.code+' '+initialSubject.name);onSelected?.(initialSubject)}},[initialSubject?.code]);
  useEffect(() => {
    const id = ++availabilityId.current; controller.current?.abort();
    setSeasons([]); setSeason(''); setManualSeason(false); setError(''); setChecking(false);
    if (!subject || !/^\d{4}$/.test(year) || Number(year) < 2000 || Number(year) > 2099) return;
    const abort = new AbortController(); controller.current = abort; setChecking(true);
    request<{seasons: Season[]; latest_available_season: string | null}>(`/api/catalog/availability?subject=${subject.code}&year=${year}`, {signal: abort.signal})
      .then(r => {if (id !== availabilityId.current) return; if (!Array.isArray(r.seasons)) throw new Error('可用考季响应格式不正确');
        setSeasons(r.seasons); setSeason(r.latest_available_season && r.seasons.some(s => s.season === r.latest_available_season && s.status === 'available') ? r.latest_available_season : '');
      }).catch(e => {if (id === availabilityId.current && !abort.signal.aborted) setError((e as Error).message)})
      .finally(() => {if (id === availabilityId.current) setChecking(false)});
    return () => {availabilityId.current++; abort.abort()};
  }, [subject, year, retry, sourceMode]);
  function invalidate() {onInvalidate(); availabilityId.current++; controller.current?.abort(); setChecking(false)}
  function choose(value: string) {
    const chosen = resolveSubject(value, catalogue);
    if (!chosen) {setError('请输入四位科目代码，或从题源目录选择科目。'); return}
    invalidate(); focusYear.current = true; setSubject({...chosen}); onSelected?.(chosen); setQuery(chosen.code + ' ' + chosen.name); setError('');
  }
  const normalized = query.toLowerCase().trim();
  const suggestions = catalogue.filter(s => !normalized || s.code.includes(normalized) || s.name.toLowerCase().includes(normalized)).slice(0, 12);
  const available = seasons.find(s => s.season === season)?.status === 'available';
  return <section className={'hero ' + (compact ? 'compact' : '')}>
    <h1>找卷</h1>
    <p>输入科目，确认年份与考季后查询。</p>
    <form className="search-hero" onSubmit={e => {e.preventDefault(); choose(subject ? subject.code : query)}}>
      <Search size={21}/><input aria-label="科目名称或代码" placeholder="搜索题源科目名称或四位代码" value={query}
        onKeyDown={e => {if(e.key==='Enter' && (e.nativeEvent.isComposing || e.keyCode===229)) e.preventDefault()}}
        onChange={e => {invalidate(); setQuery(e.target.value); setSubject(null); onSelected?.(null); setSeason(''); setSeasons([])}}/>
      <button aria-label="确认科目" type="submit"><ArrowRight size={21}/></button>
    </form>
    {!subject && <><SubjectFlow subjects={suggestions} filtered={!!normalized} onChoose={choose}/>
      <p className="catalogue-note">{catalogueError ? `题源目录暂不可用：${catalogueError}。仍可手输四位代码。` : catalogue.length ? `${catalogueSource?sourceName(catalogueSource)+' ':''}实际目录 · ${catalogue.length} 门科目 · 输入名称筛选` : '正在读取科目目录…'}</p>{catalogueError && <button className="text-button" onClick={reloadCatalogue}><RefreshCw size={13}/>重新读取目录</button>}</>}
    {subject && <div className="conditions"><div className="subject-confirmed"><Check size={15}/>{subject.name}<span>{subject.code}</span></div>
      <label>查询年份<input ref={yearInput} aria-label="年份" type="number" min="2000" max="2099" value={year} onChange={e => {invalidate(); setYear(e.target.value); setSeason(''); setSeasons([])}}/></label>
      <label>{manualSeason?'手动选择考季':'已核对考季'}<select aria-label="考季" value={season} disabled={checking} onChange={e => {onInvalidate(); setSeason(e.target.value)}}>
        <option value="">{checking ? '正在检查这一年…' : manualSeason?'请选择考季（尚未确认有卷）':'选择已确认有卷的考季'}</option>
        {(manualSeason?['Mar','Jun','Nov'].map(season=>seasons.find(s=>s.season===season)??{season,status:'unknown',file_count:0}):seasons.filter(s => s.status === 'available')).map(s => <option key={s.season} value={s.season}>{seasonName(s.season)} · {s.status==='available'?s.file_count+' 份':s.status==='empty'?'未找到':'未确认'}</option>)}
      </select></label>
      <button className="primary" disabled={loading || checking || (!available&&!(manualSeason&&season))} onClick={() => onSearch({subject, year, season})}>
        {loading ? '正在查找…' : '查找试卷'}{loading ? <RefreshCw className="spin" size={17}/> : <ArrowRight size={17}/>}</button>
      <label className="check-label"><input type="checkbox" checked={manualSeason} disabled={checking} onChange={e=>{onInvalidate();setManualSeason(e.target.checked);setSeason('')}}/>手动选择未确认考季</label><div className="availability-note">{checking ? <span role="status">正在核对 {year} 年的三个考季；尚未打开试卷结果。</span> : <>
        <span>仅核对所填年份，不代表最新收录年份。{seasons.map(s => `${seasonName(s.season)}：${s.status === 'available' ? '有卷' : s.status === 'empty' ? '未找到' : '未确认'}`).join('；')}</span>
        <button className="text-button" onClick={() => {invalidate(); setRetry(v => v + 1)}}><RefreshCw size={13}/>重新核对</button>
      </>}</div>
    </div>}
    {error && <p className="precheck-error" role="alert">{error}</p>}
  </section>;
}
