import {useEffect, useRef, useState} from 'react';
import {ArrowUpRight, Check, RefreshCw} from 'lucide-react';
import {request} from './api';

export type StudyReceipt = {
  sent_for_precheck?: boolean;
  published?: boolean;
  receipt_id?: string;
  receipt_path?: string;
  operation_state?: string;
  port?: number;
  target_port?: number;
  imports?: {id: string; filename: string; hash?: string; status: string; message?: string; duplicate?: boolean}[];
  recognition_job?: {id: string; status: string};
  precheck_failed?: boolean;
  replayed?: boolean;
};
const statusLabel = (value: string) => ({
  awaiting_agent: '等待识别 Agent', needs_review: '等待人工核对', failed: '预检失败',
  pending: '等待处理', processing: '正在处理', submitted: '请求已提交，结果待核对',
  uncertain: '结果不确定，请核对接收端', completed: '处理完成（以实际入库核验为准）',
  complete: '已保存回执',
}[value] || value);
const validPort = (port: string) => /^\d+$/.test(port) && Number(port) >= 1024 && Number(port) <= 65535;

export function StudyReceiptView({receipt}: {receipt: StudyReceipt}) {
  return <div className="precheck-receipt" role="status">
    <strong>接收端回执 · 不作为正式入库证明</strong>
    {receipt.operation_state && <p>交接状态：{statusLabel(receipt.operation_state)}</p>}
    {receipt.precheck_failed && <p className="precheck-error">接收端预检存在失败项，请逐件核对，未认定整套成功。</p>}
    {receipt.port && <p>原目标：127.0.0.1:{receipt.port}</p>}
    {receipt.replayed && <p>这是此前保存的回执，未重新上传。</p>}
    {receipt.imports?.map(item => <p key={item.id || item.filename}>
      {item.filename}：{statusLabel(item.status)}{item.duplicate ? '（接收端识别为重复原件）' : ''}
      {item.message ? ' · ' + item.message : ''}
    </p>)}
    {receipt.recognition_job && <p>识别任务：{receipt.recognition_job.id} · {statusLabel(receipt.recognition_job.status)}</p>}
    {receipt.receipt_path && <p>本地回执：{receipt.receipt_path}</p>}
    <details><summary>查看原始响应</summary><pre>{JSON.stringify(receipt, null, 2)}</pre></details>
  </div>;
}

/** Never POST automatically, retry an ambiguous upload, or equate a queue receipt with publication. */
export function StudyPrecheck({fileId, fileIds, onSent}: {fileId?: string; fileIds?: string[]; onSent?: () => void}) {
  const identity = [...new Set(fileIds || (fileId ? [fileId] : []))].sort().join('|');
  const offered = identity ? identity.split('|') : [];
  const [selected, setSelected] = useState(offered);
  const [port, setPort] = useState('');
  const [confirmed, setConfirmed] = useState(false);
  const [verifiedTarget, setVerifiedTarget] = useState('');
  const [checking, setChecking] = useState(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  const [receipt, setReceipt] = useState<StudyReceipt | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const checkId = useRef(0);
  const checkAbort = useRef<AbortController | null>(null);
  const sendLock = useRef(false);
  const currentTarget = useRef('');
  const targetKey = `${port}|${[...selected].sort().join('|')}`;
  currentTarget.current = targetKey;
  const verified = verifiedTarget === targetKey && !!verifiedTarget;
  const valid = validPort(port) && selected.length > 0 && selected.length <= 20;

  useEffect(() => {
    setSelected(identity ? identity.split('|') : []);
    setConfirmed(false); setVerifiedTarget(''); setReceipt(null); setUncertain(false); setError('');
    checkId.current++; checkAbort.current?.abort(); setChecking(false);
  }, [identity]);
  useEffect(() => () => {checkId.current++; checkAbort.current?.abort()}, []);

  function invalidate() {
    checkId.current++; checkAbort.current?.abort(); setChecking(false);
    setConfirmed(false); setVerifiedTarget(''); setError('');
  }
  async function check() {
    if (!valid || sending || receipt || uncertain) return;
    checkAbort.current?.abort();
    const controller = new AbortController(); checkAbort.current = controller;
    const id = ++checkId.current, target = targetKey;
    setChecking(true); setError(''); setVerifiedTarget(''); setConfirmed(false);
    try {
      const r = await request<{verified: boolean}>(`/api/study/status?port=${Number(port)}`, {signal: controller.signal});
      if (id !== checkId.current || target !== currentTarget.current) return;
      if (!r.verified) throw new Error('这个端口尚未确认是学习软件。');
      setVerifiedTarget(target);
    } catch (e) {
      if (id === checkId.current && !controller.signal.aborted) setError((e as Error).message);
    } finally {if (id === checkId.current) setChecking(false)}
  }
  async function send() {
    if (!valid || !confirmed || !verified || sendLock.current || receipt || uncertain) return;
    const submittedTarget = targetKey;
    sendLock.current = true; setSending(true); setError(''); setConfirmed(false);
    try {
      const r = await request<StudyReceipt>('/api/study/import-batch', {
        method: 'POST', headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({port: Number(port), confirmed: true, ids: [...selected].sort()}),
      });
      if (submittedTarget === currentTarget.current) setReceipt(r);
      else setError('上一批发送已返回回执，请在本地交接记录核对；当前选择未作为该回执的发送对象。');
      onSent?.();
    } catch (e) {
      // A failed response does not prove that the receiver did not accept files.
      setUncertain(true);
      setError((e as Error).message + ' 本次发送结果待核对，已锁定重发；请查看本地交接记录及学习软件队列。');
      onSent?.();
    } finally {setSending(false); sendLock.current = false}
  }
  return <section className="study-precheck" aria-label="学习预检">
    <h4>整套送到学习软件预检</h4>
    <p>同一次上传题卷与评分标准，接收端决定配对与识别任务。每批最多 20 份、合计 90 MiB；预检不等于已入库。</p>
    <div className="precheck-files">{offered.map(id => <label key={id}>
      <input type="checkbox" aria-label={'预检选择 ' + id} checked={selected.includes(id)} disabled={sending || !!receipt || uncertain}
        onChange={e => {invalidate(); setSelected(v => e.target.checked ? [...v, id] : v.filter(x => x !== id))}}/>{id}
    </label>)}</div>
    <div className="precheck-target"><label>本机服务端口
      <input aria-label="学习软件端口" type="text" inputMode="numeric" placeholder="输入学习软件显示的端口" value={port}
        disabled={sending || !!receipt || uncertain} onChange={e => {invalidate(); setPort(e.target.value)}}/>
    </label><button className="secondary" disabled={!valid || checking || sending || !!receipt || uncertain} onClick={() => void check()}>
      {checking ? <RefreshCw size={15}/> : <Check size={15}/>}核对服务
    </button><span className="quiet">{verified ? '此端口身份已核对（文件仍需接收端预检）' : '未连接，不会自动扫描端口'}</span></div>
    <label className="precheck-consent"><input type="checkbox" aria-label="确认本次预检发送" checked={confirmed}
      disabled={!verified || sending || !!receipt || uncertain} onChange={e => setConfirmed(e.target.checked)}/>
      我确认将上面所选 {selected.length} 份 PDF 发送到 127.0.0.1:{port || '待指定'}，由学习软件保存并预检。
    </label>
    <button className="primary" disabled={!valid || !confirmed || !verified || sending || !!receipt || uncertain} onClick={() => void send()}>
      {sending ? '正在发送，请勿重复操作…' : uncertain ? '结果待核对，已锁定重发' : receipt ? '已收到预检回执' : '确认整套发送预检'}<ArrowUpRight size={16}/>
    </button>
    {error && <p className="precheck-error" role="alert">{error}</p>}
    {receipt && <StudyReceiptView receipt={receipt}/>}
  </section>;
}

export function StudyReceiptHistory({revision}: {revision: number}) {
  const [receipts, setReceipts] = useState<StudyReceipt[]>([]);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const requestId = useRef(0);
  async function refresh() {
    const id = ++requestId.current; setLoading(true); setError('');
    try {const r = await request<{receipts: StudyReceipt[]}>('/api/study/receipts');
      if (id === requestId.current) {if (!Array.isArray(r.receipts)) throw new Error('本地回执响应格式不正确'); setReceipts(r.receipts)};
    } catch (e) {if (id === requestId.current) setError((e as Error).message)}
    finally {if (id === requestId.current) setLoading(false)}
  }
  useEffect(() => {void refresh(); return () => {requestId.current++}}, [revision]);
  return <section className="receipt-history"><div className="section-heading"><h2>本地交接记录</h2>
    <button className="secondary" disabled={loading} onClick={() => void refresh()}><RefreshCw size={15}/>刷新本地记录</button></div>
    <p className="quiet">这里只读取 C-Paper 保存的回执。查询学习软件任务状态需要再次指定端口并手动操作。</p>
    {error && <p role="alert" className="precheck-error">{error}</p>}
    {receipts.map((r, i) => <ReceiptStatus key={r.receipt_id || i} receipt={r}/>)}
    {!loading && !error && !receipts.length && <p className="quiet">还没有保存的交接记录。</p>}
  </section>;
}
function ReceiptStatus({receipt}: {receipt: StudyReceipt}) {
  const [port, setPort] = useState(''), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [status, setStatus] = useState<unknown>(null);
  const revision = useRef(0);
  useEffect(() => () => {revision.current++}, []);
  async function query() {
    const id = ++revision.current; setBusy(true); setError(''); setStatus(null);
    try {const r = await request<unknown>(`/api/study/receipts/${encodeURIComponent(receipt.receipt_id!)}/status?port=${Number(port)}`);
      if (id === revision.current) setStatus(r);
    } catch (e) {if (id === revision.current) setError((e as Error).message)}
    finally {if (id === revision.current) setBusy(false)}
  }
  return <article className="history-receipt"><StudyReceiptView receipt={receipt}/>
    {receipt.receipt_id && <div className="precheck-target"><label>查询此任务的本机端口
      <input aria-label={'查询端口 ' + receipt.receipt_id} value={port} inputMode="numeric" placeholder="手动输入端口" onChange={e => {revision.current++; setBusy(false); setStatus(null); setError(''); setPort(e.target.value)}}/>
    </label><button className="secondary" disabled={!validPort(port) || busy} onClick={() => void query()}>核对服务并查询此任务</button></div>}
    {error && <p role="alert" className="precheck-error">{error}</p>}
    {status !== null && <div className="precheck-receipt" role="status"><strong>刚刚读取的任务实际状态</strong><pre>{JSON.stringify(status, null, 2)}</pre></div>}
  </article>;
}
