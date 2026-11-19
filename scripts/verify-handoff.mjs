import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {promises as fs} from 'node:fs';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import path from 'node:path';
import {verifyDsh} from './verify-dsh.mjs';
const runFile=promisify(execFile);
export async function verifyHandoff({dshRoot,projectRoot,codex}){
 const driver=new URL('./codex-client.mjs',import.meta.url);const {CodexClient}=await import(driver.href+'?v='+(await fs.stat(driver)).mtimeMs);
 const version=(await runFile(codex,['--version'],{windowsHide:true})).stdout.trim();
 return verifyDsh({dshRoot,projectRoot,handoff:true,afterSave:async source=>{
  const home=path.join(source.root,'codex-home');await fs.mkdir(home,{recursive:true});const client=new CodexClient({codex,home,workspace:source.workspace,retenixa:source.binary,dataDir:source.dataDir});
  try{
   await client.start(source.workspace);const inventory=await client.status();const server=inventory.data.find(s=>s.name==='retenixa');assert.ok(server.tools.memory_search);assert.ok(server.tools.context_resume);
   const opened=await client.call('context_open',{workspace_path:source.workspace,client_label:'codex-handoff',mode:'fresh',request_id:randomUUID()});assert.equal(opened.isError,false,JSON.stringify(opened));const context=opened.structuredContent;assert.equal(context.project_id,source.context.project_id);assert.notEqual(context.workflow_id,source.context.workflow_id);
   const read=await client.call('context_resume',{context_id:context.context_id,task_id:source.task.task_id,checkpoint_id:source.saved.checkpoint_id,cursor:null,byte_budget:1048576});assert.equal(read.isError,false,JSON.stringify(read));const recovery=read.structuredContent;
   assert.equal(recovery.saved_context.fixed_knowledge[0].claim,'交接认证策略v1：refreshAccessToken失败时先核对原请求，不重复执行未知发布。');assert.equal(recovery.saved_context.fixed_knowledge[0].revision,2);assert.ok(recovery.current_delta.some(d=>d.kind==='changed'&&d.subject_id===source.knowledge.knowledge_id));assert.equal(recovery.saved_context.snapshot.unresolved_operations[0].operation_id,'external-unknown');assert.equal(recovery.current_task.owner_workflow_id,source.context.workflow_id);
   const failures=await client.call('memory_search',{context_id:context.context_id,query:'refreshAccessToken',kind:'attempts',scope:{task_id:source.task.task_id,workspace_id:null,path_prefix:null,conditions:['offline'],include_inactive:false},cursor:null,limit:20,byte_budget:65536});assert.equal(failures.isError,false,JSON.stringify(failures));assert.equal(failures.structuredContent.results[0].attempt.reason.value,'验证环境离线');
   const snapshot=JSON.parse(JSON.stringify(recovery.saved_context.snapshot));snapshot.workspace.workspace_id=context.workspace_id;snapshot.stage='Codex受控接续';snapshot.next_steps=['先确认原未知发布，再按失败条件评估下一步'];
   const newSave=()=>({context_id:context.context_id,request_id:randomUUID(),task_id:source.task.task_id,expected_revision:recovery.current_task.revision,ownership_epoch:recovery.current_task.ownership_epoch,snapshot,knowledge_refs:recovery.saved_context.knowledge_refs,evidence_refs:recovery.saved_context.evidence_refs,target_status:'in_progress'});
   const denied=await client.call('checkpoint_save',newSave());assert.equal(denied.isError,true);assert.equal(denied.structuredContent.error.code,'OWNERSHIP_CHANGED');
   const file=path.join(source.root,'approved-transfer.json');await fs.writeFile(file,JSON.stringify({project_id:context.project_id,task_id:source.task.task_id,target_workflow_id:context.workflow_id,expected_revision:recovery.current_task.revision,expected_epoch:recovery.current_task.ownership_epoch,request_id:randomUUID(),reason:'用户授权的隔离第二客户端交接验收'}));
   const transfer=JSON.parse((await runFile(source.binary,['transfer','--data-dir',source.dataDir,'--request-file',file,'--confirm-transfer'],{windowsHide:true,timeout:20000})).stdout);
   const old=await source.invokeDsh('checkpoint_save',{task_id:source.task.task_id,expected_revision:transfer.result.revision,ownership_epoch:transfer.result.ownership_epoch,snapshot:source.restored.saved_context.snapshot,knowledge_refs:source.restored.saved_context.knowledge_refs,evidence_refs:[],target_status:'in_progress',related_paths:[]});assert.equal(old.error,true);
   const save=newSave();save.expected_revision=transfer.result.revision;save.ownership_epoch=transfer.result.ownership_epoch;const committed=await client.call('checkpoint_save',save);assert.equal(committed.isError,false,JSON.stringify(committed));assert.equal(committed.structuredContent.result.task.owner_workflow_id,context.workflow_id);
   const final=await client.call('context_resume',{context_id:context.context_id,task_id:source.task.task_id,checkpoint_id:null,cursor:null,byte_budget:1048576});assert.equal(final.isError,false);assert.equal(final.structuredContent.saved_context.snapshot.unresolved_operations[0].operation_id,'external-unknown');assert.equal(final.structuredContent.saved_context.fixed_knowledge[0].revision,2);
   assert.ok(!client.calls.includes('turn/start'));assert.equal(client.protocolError,undefined);const report={status:'passed',secondClient:version,threadId:client.threadId,projectId:context.project_id,sourceWorkflow:source.context.workflow_id,targetWorkflow:context.workflow_id,oldPinnedRevision:2,currentKnowledgeRevision:3,failedAttemptFound:true,deniedBeforeTransfer:true,oldOwnerRejectedAfterTransfer:true,approvedTransfer:true,unknownOperationPreserved:true,modelTurns:0,userProfileModified:false,nonProtocolStdout:client.diagnostics,methods:client.calls};await fs.writeFile(path.join(source.root,'handoff-report.json'),JSON.stringify(report,null,2));return report;
  }finally{await client.close();}
 }});
}
