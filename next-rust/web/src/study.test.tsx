// @vitest-environment jsdom
import {afterEach,it,expect,vi} from 'vitest';
import {render,screen,fireEvent,cleanup,waitFor} from '@testing-library/react';
import {StudyPrecheck,StudyReceiptHistory} from './StudyPrecheck';
const response=(data:unknown,status=200)=>new Response(JSON.stringify(data),{status,headers:{'content-type':'application/json'}});
const files=['9709_s23_qp_12.pdf','9709_s23_ms_12.pdf'];
const receipt={sent_for_precheck:true,published:false,receipt_id:'r1',receipt_path:'/test/receipt.json',port:4317,operation_state:'complete',imports:files.map((filename,i)=>({id:String(i),filename,status:'awaiting_agent'})),recognition_job:{id:'job',status:'awaiting_agent'}};
async function verify(){fireEvent.change(screen.getByLabelText('学习软件端口'),{target:{value:'4317'}});fireEvent.click(screen.getByRole('button',{name:'核对服务'}));await screen.findByText('此端口身份已核对（文件仍需接收端预检）')}
afterEach(()=>{cleanup();vi.unstubAllGlobals()});
it('does not connect by default and sends both selected files only after explicit consent',async()=>{const fetcher=vi.fn(async(url:RequestInfo|URL,init?:RequestInit)=>response(String(url).includes('/status')?{verified:true}:receipt));vi.stubGlobal('fetch',fetcher);render(<StudyPrecheck fileIds={files}/>);expect(fetcher).not.toHaveBeenCalled();await verify();expect(fetcher).toHaveBeenCalledTimes(1);expect((screen.getByRole('button',{name:'确认整套发送预检'}) as HTMLButtonElement).disabled).toBe(true);fireEvent.click(screen.getByLabelText('确认本次预检发送'));fireEvent.click(screen.getByRole('button',{name:'确认整套发送预检'}));await screen.findByText('9709_s23_qp_12.pdf：等待识别 Agent');expect(fetcher).toHaveBeenCalledTimes(2);expect(JSON.parse(String(fetcher.mock.calls[1][1]?.body))).toEqual({port:4317,confirmed:true,ids:[...files].sort()});expect(screen.getByText('接收端回执 · 不作为正式入库证明')).toBeTruthy()});
it('changing port or selected files clears verification and consent',async()=>{vi.stubGlobal('fetch',vi.fn(async()=>response({verified:true})));render(<StudyPrecheck fileIds={files}/>);await verify();fireEvent.click(screen.getByLabelText('确认本次预检发送'));fireEvent.click(screen.getByLabelText('预检选择 '+files[1]));expect((screen.getByLabelText('确认本次预检发送') as HTMLInputElement).checked).toBe(false);expect((screen.getByRole('button',{name:'确认整套发送预检'}) as HTMLButtonElement).disabled).toBe(true);await verify();fireEvent.click(screen.getByLabelText('确认本次预检发送'));fireEvent.change(screen.getByLabelText('学习软件端口'),{target:{value:'4320'}});expect((screen.getByLabelText('确认本次预检发送') as HTMLInputElement).checked).toBe(false)});
it('ignores verification that returns after the target port changes',async()=>{let finish!:(r:Response)=>void;vi.stubGlobal('fetch',vi.fn(async()=>await new Promise<Response>(r=>{finish=r})));render(<StudyPrecheck fileIds={files}/>);fireEvent.change(screen.getByLabelText('学习软件端口'),{target:{value:'4317'}});fireEvent.click(screen.getByRole('button',{name:'核对服务'}));fireEvent.change(screen.getByLabelText('学习软件端口'),{target:{value:'4320'}});finish(response({verified:true}));await waitFor(()=>expect(screen.getByText('未连接，不会自动扫描端口')).toBeTruthy());expect((screen.getByLabelText('确认本次预检发送') as HTMLInputElement).disabled).toBe(true)});
it('locks an ambiguous upload rather than offering blind retries',async()=>{const fetcher=vi.fn(async(url:RequestInfo|URL)=>String(url).includes('/status')?response({verified:true}):response({error:'接收端响应丢失，结果不确定'},502));vi.stubGlobal('fetch',fetcher);render(<StudyPrecheck fileIds={files}/>);await verify();fireEvent.click(screen.getByLabelText('确认本次预检发送'));const send=screen.getByRole('button',{name:'确认整套发送预检'});fireEvent.click(send);fireEvent.click(send);await screen.findByRole('alert');expect(fetcher.mock.calls.filter(c=>String(c[0]).includes('/import-batch'))).toHaveLength(1);expect((screen.getByRole('button',{name:'结果待核对，已锁定重发'}) as HTMLButtonElement).disabled).toBe(true)});
it('loads only local receipts until an explicit status query is requested',async()=>{const fetcher=vi.fn(async(_url:RequestInfo|URL)=>response({receipts:[receipt]}));vi.stubGlobal('fetch',fetcher);render(<StudyReceiptHistory revision={0}/>);await screen.findByText('9709_s23_qp_12.pdf：等待识别 Agent');expect(fetcher).toHaveBeenCalledTimes(1);expect(String(fetcher.mock.calls[0]?.[0])).toBe('/api/study/receipts');expect((screen.getByRole('button',{name:'核对服务并查询此任务'}) as HTMLButtonElement).disabled).toBe(true)});
it('does not attach a late upload receipt to changed file props',async()=>{
 let finish!:(r:Response)=>void;vi.stubGlobal('fetch',vi.fn(async(url:RequestInfo|URL)=>String(url).includes('/status')?response({verified:true}):await new Promise<Response>(r=>{finish=r})));
 const {rerender}=render(<StudyPrecheck fileIds={files}/>);await verify();fireEvent.click(screen.getByLabelText('确认本次预检发送'));fireEvent.click(screen.getByRole('button',{name:'确认整套发送预检'}));rerender(<StudyPrecheck fileIds={['different.pdf']}/>);finish(response(receipt));await screen.findByText(/上一批发送已返回回执/);expect(screen.queryByText('9709_s23_qp_12.pdf：等待识别 Agent')).toBe(null);
});
it('surfaces a receiver precheck failure even when a receipt was saved',async()=>{
 vi.stubGlobal('fetch',vi.fn(async(url:RequestInfo|URL)=>String(url).includes('/status')?response({verified:true}):response({...receipt,precheck_failed:true,imports:[{id:'x',filename:files[0],status:'failed',message:'PDF异常'}]})));
 render(<StudyPrecheck fileIds={files}/>);await verify();fireEvent.click(screen.getByLabelText('确认本次预检发送'));fireEvent.click(screen.getByRole('button',{name:'确认整套发送预检'}));await screen.findByText('接收端预检存在失败项，请逐件核对，未认定整套成功。');expect(screen.getByText(/预检失败 · PDF异常/)).toBeTruthy();
});

async function showPriorTaskStatus(next:()=>Promise<Response>){
 let statusCalls=0;
 vi.stubGlobal('fetch',vi.fn(async(url:RequestInfo|URL)=>{
  if(!String(url).includes('/status?'))return response({receipts:[receipt]});
  statusCalls++;
  return statusCalls===1?response({verified:true,port:4317,receipt_id:'r1',recognition_job:{id:'job',status:'awaiting_agent',test_marker:'previous-success'}}):next();
 }));
 render(<StudyReceiptHistory revision={0}/>);
 const target=await screen.findByLabelText('查询端口 r1');
 fireEvent.change(target,{target:{value:'4317'}});
 fireEvent.click(screen.getByRole('button',{name:'核对服务并查询此任务'}));
 await screen.findByText('刚刚读取的任务实际状态');
 await waitFor(()=>expect((screen.getByRole('button',{name:'核对服务并查询此任务'}) as HTMLButtonElement).disabled).toBe(false));
}
it.each([
 [502,'学习服务暂不可用'],
 [409,'学习软件服务身份不匹配'],
 [409,'任务来源文件 SHA256 不匹配'],
])('ReceiptStatus removes stale success after refresh error %s %s',async(status,error)=>{
 await showPriorTaskStatus(async()=>response({error},status));
 fireEvent.click(screen.getByRole('button',{name:'核对服务并查询此任务'}));
 await screen.findByText(error);
 expect(screen.queryByText('刚刚读取的任务实际状态')).toBe(null);
 expect(screen.queryByText(/previous-success/)).toBe(null);
});
it('ReceiptStatus clears the prior success immediately while a refresh is pending',async()=>{
 let finish!:(r:Response)=>void;
 await showPriorTaskStatus(async()=>await new Promise<Response>(r=>{finish=r}));
 fireEvent.click(screen.getByRole('button',{name:'核对服务并查询此任务'}));
 expect((screen.getByRole('button',{name:'核对服务并查询此任务'}) as HTMLButtonElement).disabled).toBe(true);
 expect(screen.queryByText('刚刚读取的任务实际状态')).toBe(null);
 expect(screen.queryByText(/previous-success/)).toBe(null);
 finish(response({verified:true,port:4317,receipt_id:'r1',recognition_job:{id:'job',status:'processing',test_marker:'fresh-success'}}));
 await screen.findByText(/fresh-success/);
 expect(screen.getByText('刚刚读取的任务实际状态')).toBeTruthy();
});
