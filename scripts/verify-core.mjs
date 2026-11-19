import assert from 'node:assert/strict';
import {spawn,execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {randomUUID} from 'node:crypto';
import {promises as fs} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const runFile=promisify(execFile);
class McpProcess {
  constructor(binary,dataDir,binding){this.child=spawn(binary,['serve','--data-dir',dataDir,...(binding?['--binding-file',binding]:[])],{stdio:['pipe','pipe','pipe'],windowsHide:true});this.pid=this.child.pid;this.counter=0;this.pending=new Map();this.buffer='';this.stderr='';this.exit=null;this.invalid=[];this.child.stdout.setEncoding('utf8');this.child.stderr.setEncoding('utf8');this.child.stderr.on('data',d=>this.stderr+=d);this.child.stdout.on('data',d=>{this.buffer+=d;let at;while((at=this.buffer.indexOf('\n'))>=0){const raw=this.buffer.slice(0,at);this.buffer=this.buffer.slice(at+1);try{const message=JSON.parse(raw);const pending=this.pending.get(message.id);if(pending){clearTimeout(pending.timer);this.pending.delete(message.id);message.error?pending.reject(Error(JSON.stringify(message.error))):pending.resolve(message.result);}}catch(e){this.invalid.push(e.message);}}});this.child.on('error',e=>{for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(e);}this.pending.clear();});this.child.on('exit',(code,signal)=>{this.exit={code,signal};for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(Error('MCP child exited '+code));}this.pending.clear();});}
  request(method,params){const id=++this.counter;return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{this.pending.delete(id);reject(Error('MCP deadline '+method));},15000);this.pending.set(id,{resolve,reject,timer});this.child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');});}
  async start(){await this.request('initialize',{protocolVersion:'2025-11-25',capabilities:{},clientInfo:{name:'retenixa-cross-process',version:'0.1.0'}});this.child.stdin.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})+'\n');const tools=await this.request('tools/list',{});assert.equal(tools.tools.length,13);assert.ok(tools.tools.some(tool=>tool.name==='task_release'));return this;}
  async call(name,args){return this.request('tools/call',{name,arguments:args});}
  async close(){if(this.exit)return this.exit;const done=new Promise(resolve=>this.child.once('exit',(code,signal)=>resolve({code,signal})));this.child.stdin.end();const timer=setTimeout(()=>this.child.kill(),3000);const exit=await done;clearTimeout(timer);return exit;}
}
export async function verifyCore(projectRoot) {
  const root=path.join(projectRoot,'target/mcp-verification',randomUUID());const workspace=path.join(root,'workspace');await fs.mkdir(workspace,{recursive:true});await runFile('git',['-C',workspace,'init','-b','synthetic']);const dataDir=path.join(root,'data');const binary=path.join(projectRoot,'target/debug/retenixa.exe');const processes=[];let report;
  try {
    const a=new McpProcess(binary,dataDir);const b=new McpProcess(binary,dataDir);processes.push(a,b);await Promise.all([a.start(),b.start()]);assert.notEqual(a.pid,b.pid);
    const contexts=await Promise.all([a,b].map(p=>p.call('context_open',{workspace_path:workspace,client_label:'cross-process',mode:'fresh',request_id:randomUUID()})));for(const x of contexts)assert.equal(x.isError,false,JSON.stringify(x));const [ca,cb]=contexts.map(x=>x.structuredContent);assert.equal(ca.project_id,cb.project_id);assert.notEqual(ca.workflow_id,cb.workflow_id);
    const created=await a.call('task_create',{context_id:ca.context_id,request_id:randomUUID(),goal:'跨进程原子认领',scope:'合成验收',assignment:'unassigned',parent_task_id:null});assert.equal(created.isError,false);const task=created.structuredContent.result.task;
    const claims=await Promise.all([[a,ca],[b,cb]].map(([p,c])=>p.call('task_claim',{context_id:c.context_id,request_id:randomUUID(),task_id:task.task_id,expected_revision:1,expected_epoch:1})));assert.equal(claims.filter(x=>!x.isError).length,1);assert.equal(claims.filter(x=>x.isError).length,1);
    const winner=claims[0].isError?1:0;const oldContext=[ca,cb][winner];const newContext=[ca,cb][1-winner];const oldProcess=[a,b][winner];const newProcess=[a,b][1-winner];
    const transferFile=path.join(root,'transfer.json');await fs.writeFile(transferFile,JSON.stringify({project_id:ca.project_id,task_id:task.task_id,target_workflow_id:newContext.workflow_id,expected_revision:2,expected_epoch:2,request_id:randomUUID(),reason:'隔离跨进程管理转交'}));
    const transfer=JSON.parse((await runFile(binary,['transfer','--request-file',transferFile,'--confirm-transfer','--data-dir',dataDir],{windowsHide:true,timeout:20000})).stdout);
    function checkpoint(context){return {context_id:context.context_id,request_id:randomUUID(),task_id:task.task_id,expected_revision:transfer.result.revision,ownership_epoch:transfer.result.ownership_epoch,target_status:'in_progress',knowledge_refs:[],evidence_refs:[],snapshot:{schema_version:1,goal:'跨进程原子认领',scope:'合成验收',stage:'续写',completion_conditions:['拒绝旧所有者'],workspace:{workspace_id:context.workspace_id,observed_at:new Date().toISOString(),branch:{state:'unknown',reason:'服务采集'},head:{state:'unknown',reason:'空仓库'},base_commit:{state:'unknown',reason:'空仓库'},related_files:[],consistency:'unknown'},progress:[],attempts:[],verifications:[],unresolved_operations:[],blockers:[],next_steps:['恢复核对'],no_references_reason:'无固定知识'}};}
    const rejected=await oldProcess.call('checkpoint_save',checkpoint(oldContext));assert.equal(rejected.isError,true);assert.equal(rejected.structuredContent.error.code,'OWNERSHIP_CHANGED');const saved=await newProcess.call('checkpoint_save',checkpoint(newContext));assert.equal(saved.isError,false,JSON.stringify(saved));
    const assertOwnership = (reply, current) => {
      assert.equal(reply.isError,true);
      const error=reply.structuredContent.error;
      assert.equal(error.code,'OWNERSHIP_CHANGED');
      assert.equal(error.task_id,current.task_id);
      assert.equal(error.current_revision,current.revision);
      assert.equal(error.current_epoch,current.ownership_epoch);
      assert.equal(error.owner_workflow_id,current.owner_workflow_id);
    };
    assertOwnership(rejected,transfer.result);
    const createOwned = async (p,c,goal) => {
      const reply=await p.call('task_create',{context_id:c.context_id,request_id:randomUUID(),goal,scope:'隔离交接',assignment:'current_workflow',parent_task_id:null});
      assert.equal(reply.isError,false,JSON.stringify(reply));
      return reply.structuredContent.result.task;
    };
    const ownershipArgs = (c,t) => ({context_id:c.context_id,request_id:randomUUID(),task_id:t.task_id,expected_revision:t.revision,expected_epoch:t.ownership_epoch});
    const releaseCandidate=await createOwned(oldProcess,oldContext,'主动释放');
    const releaseRequest=ownershipArgs(oldContext,releaseCandidate);
    const released=await oldProcess.call('task_release',releaseRequest);
    assert.equal(released.isError,false,JSON.stringify(released));
    const releasedTask=released.structuredContent.result.task;
    assert.equal(releasedTask.owner_workflow_id,null);
    assert.equal(releasedTask.revision,2);
    assert.equal(releasedTask.ownership_epoch,2);
    const staleClaim=await newProcess.call('task_claim',ownershipArgs(newContext,releaseCandidate));
    assertOwnership(staleClaim,releasedTask);
    const claimedReleased=await newProcess.call('task_claim',ownershipArgs(newContext,releasedTask));
    assert.equal(claimedReleased.isError,false,JSON.stringify(claimedReleased));
    assert.equal((await oldProcess.call('task_release',releaseRequest)).structuredContent.replayed,true);
    const exitCandidate=await createOwned(oldProcess,oldContext,'退出后接管');
    const liveClaim=await newProcess.call('task_claim',ownershipArgs(newContext,exitCandidate));
    assert.equal(liveClaim.structuredContent.error.code,'CLAIM_DENIED');
    assert.equal((await oldProcess.close()).code,0);
    const afterExit=await newProcess.call('task_claim',ownershipArgs(newContext,exitCandidate));
    assert.equal(afterExit.isError,false,JSON.stringify(afterExit));
    assert.equal(afterExit.structuredContent.result.task.owner_workflow_id,newContext.workflow_id);
    assert.equal(afterExit.structuredContent.result.task.ownership_epoch,2);
    const bindingFile=path.join(root,'host-state','binding.json');
    await runFile(binary,['host-init','--data-dir',dataDir,'--binding-file',bindingFile,'--host','dsh','--host-session',randomUUID()]);
    const bound=new McpProcess(binary,dataDir,bindingFile);processes.push(bound);await bound.start();
    const openBound=async p=>{
      const result=await p.call('context_open',{workspace_path:workspace,client_label:'dsh',mode:'continue',request_id:randomUUID()});
      assert.equal(result.isError,false,JSON.stringify(result));
      return result.structuredContent;
    };
    const boundContext=await openBound(bound);
    const protectedTask=await createOwned(bound,boundContext,'DSH 停机仍保留归属');
    assert.equal((await bound.close()).code,0);
    assert.equal((await newProcess.call('task_claim',ownershipArgs(newContext,protectedTask))).structuredContent.error.code,'CLAIM_DENIED');
    const binding=JSON.parse(await fs.readFile(bindingFile,'utf8'));
    await fs.writeFile(bindingFile,JSON.stringify({...binding,expected_generation:1,attachment_request_id:randomUUID()}));
    const restarted=new McpProcess(binary,dataDir,bindingFile);processes.push(restarted);await restarted.start();
    const restartedContext=await openBound(restarted);
    assert.equal(restartedContext.workflow_id,boundContext.workflow_id);
    assert.equal(restartedContext.writer_generation,2);
    assert.equal((await restarted.call('task_release',ownershipArgs(restartedContext,protectedTask))).isError,false);
    const backup=path.join(root,'backup.sqlite3');const backed=JSON.parse((await runFile(binary,['backup','--output',backup,'--data-dir',dataDir],{windowsHide:true,timeout:20000})).stdout);assert.equal(backed.consistent,true);
    let blocked=false;try {await runFile(binary,['restore','--input',backup,'--data-dir',dataDir],{windowsHide:true,timeout:20000});}catch(error){blocked=error.stderr.includes('StorageBusy');}assert.equal(blocked,true);
    report={status:'passed',processes:processes.map(p=>p.pid),successfulClaims:1,rejectedClaims:1,oldOwnerRejected:true,releaseAndReclaim:true,ordinaryTakeoverAfterExit:true,persistentWorkflowProtected:true,backupWhileServing:true,restoreBlockedWhileServing:true,checkpointId:saved.structuredContent.result.checkpoint_id,root};
    for(const p of processes){assert.deepEqual(p.invalid,[]);assert.equal(p.stderr.includes('panicked'),false);}
  }finally {for(const p of processes){const exit=await p.close();assert.equal(exit.code,0);}}
  const restored=JSON.parse((await runFile(binary,['restore','--input',path.join(root,'backup.sqlite3'),'--data-dir',dataDir],{windowsHide:true,timeout:20000})).stdout);assert.equal(restored.integrity,'ok');report.restoreAfterAllClientsExit=true;await fs.writeFile(path.join(root,'report.json'),JSON.stringify(report,null,2));return report;
}
if(process.argv[1]&&path.resolve(process.argv[1])===path.resolve(fileURLToPath(import.meta.url))){console.log(await verifyCore(path.resolve(process.argv[2]??'.')));}
