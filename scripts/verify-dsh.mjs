import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
const runFile=promisify(execFile);
export async function verifyDsh({dshRoot,projectRoot,handoff=false,afterSave}) {
  const adapterUrl=new URL('../adapters/dsh/index.js',import.meta.url);const {install,inject}=await import(adapterUrl.href+'?v='+(await fs.stat(adapterUrl)).mtimeMs);
  const requireHost=createRequire(path.join(dshRoot,'node_modules/@deepseek-ai/dsh-agent/package.json'));
  const root=path.join(projectRoot,'target','dsh-verification',randomUUID());
  const workspace=path.join(root,'workspace');await fs.mkdir(workspace,{recursive:true});
  const progress=async stage=>{await fs.writeFile(path.join(root,'progress.json'),JSON.stringify({stage,at:new Date().toISOString(),root}));console.log('DSH verification:',stage);};
  await progress('workspace');await runFile('git',['-C',workspace,'init','-b','synthetic']);
  const binary=path.join(projectRoot,'target/debug/retenixa.exe');await fs.stat(binary);
  const sessionId='retenixa-'+randomUUID();
  const dataDir=path.join(root,'data');
  const config={command:binary,dataDir,workspacePath:workspace};
  let dropResponse=true;let callCounter=0;
  async function host(fault=false) {
    const {Context}=requireHost('@deepseek-ai/cordis');const ctx=new Context();const fibers=[];let controller;
    async function mount(name,settings={}){await progress('mount:'+name);const m=requireHost('@deepseek-ai/'+name);const f=ctx.plugin(m.default??m,settings);fibers.push(f);let timer;try{await Promise.race([f,new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('宿主插件依赖未就绪: '+name)),10000);})]);}finally{clearTimeout(timer);}}
    try {
      for(const name of ['dsh-session','dsh-session-projection','dsh-agent','dsh-llm','dsh-system-prompt','dsh-tools','dsh-tool-executor-native'])await mount(name);
      await mount('dsh-session-persistence-jsonl',{root:path.join(root,'sessions'),compression:'none'});
      await mount('dsh-storage');await mount('dsh-storage-json',{root:path.join(root,'storages')});await mount('dsh-storage-domain',{backend:'json'});
      await mount('dsh-agent-loop',{agents:[]});
      const scopedRequire=name=>{
        const actual=requireHost(name);
        if(name!=='@modelcontextprotocol/client'||!fault)return actual;
        return {...actual,Client:class extends actual.Client {async callTool(params,options){const result=await super.callTool(params,options);if(dropResponse&&params.name==='task_create'&&!result.isError){dropResponse=false;throw new Error('Synthetic response loss after actual database commit');}return result;}}};
      };
      const f=ctx.plugin({name:'retenixa-verification-adapter',inject,async apply(scope){controller=await install(scope,config,scopedRequire);}});fibers.push(f);await f;
      return {ctx,controller,async close(handle){await progress('dispose-host');if(handle)await handle.dispose();for(const f of fibers.reverse())await f.dispose();await progress('host-closed');}};
    }catch(error){for(const f of fibers.reverse())await f.dispose();throw error;}
  }
  async function invoke(h,agent,raw,args={}) {
    await progress('invoke:'+raw);const turn=++callCounter;agent.session.append('turn/start',{turn});agent.session.append('step/start',{turn,step:1});agent.session.append('assistant/message',{turn,step:1,stream:[],message:requireHost('@deepseek-ai/dsh-llm').createAssistantMessage({source:{provider:'retenixa-synthetic',model:'verification-only-no-llm'},content:[{type:'tool-call',id:'call-'+turn,name:'mcp__retenixa__'+raw,arguments:JSON.stringify(args)}]})},{surfaceOp:'append'});const abort=new AbortController();const timer=setTimeout(()=>abort.abort(),12000);
    try {const outcome=await h.ctx.tools.execute({calls:[{id:'call-'+turn,name:'mcp__retenixa__'+raw,arguments:JSON.stringify(args)}],signal:abort.signal,agent,log:{turn,step:1}});const result=outcome.results[0].result;return {error:result.isError===true,data:result.value?.structuredContent,rendered:result.content};}
    finally {clearTimeout(timer);agent.session.append('step/end',{turn,step:1});agent.session.append('turn/end',{turn,reason:'completed'});await h.ctx.sessions.flush(agent.session);}
  }
  let h1=await host(true);let firstHandle;let original;let lostId;
  try {
    await progress('create-agent');firstHandle=await h1.ctx.agents.create({sessionId,agentOptions:{}});const agent=firstHandle.agent;
    const opened=await invoke(h1,agent,'context_open');assert.equal(opened.error,false,JSON.stringify(opened));original=opened.data;assert.equal(original.writer_generation,1);
    const definition=agent.ctx.tools.get('mcp__retenixa__task_create',agent);if(definition?.parameters){assert.equal('context_id' in definition.parameters.properties,false);assert.equal('request_id' in definition.parameters.properties,false);}
    const lost=await invoke(h1,agent,'task_create',{goal:'隔离宿主持久续接',scope:'合成验收',assignment:'current_workflow',parent_task_id:null});assert.equal(lost.error,true);
    const pending=await invoke(h1,agent,'pending_requests');assert.equal(pending.data.pending.length,1);lostId=pending.data.pending[0].request_id;
  }finally{await h1.close(firstHandle);}
  h1=null;
  const h2=await host(false);let resumed;
  try {
    await progress('resume-agent');resumed=await h2.ctx.agents.resume({resumeSessionId:sessionId,agentOptions:{}});const agent=resumed.agent;
    const opened=await invoke(h2,agent,'context_open');assert.equal(opened.error,false,JSON.stringify(opened));assert.equal(opened.data.workflow_id,original.workflow_id);assert.equal(opened.data.writer_generation,2);
    const pending=await invoke(h2,agent,'pending_requests');assert.equal(pending.data.pending[0].request_id,lostId);
    const confirmed=await invoke(h2,agent,'retry_request',{request_id:lostId});assert.equal(confirmed.error,false,JSON.stringify(confirmed));assert.equal(confirmed.data.status,'committed');
    const list=await invoke(h2,agent,'task_list',{status:null,workspace_id:null,parent_task_id:null,cursor:null,limit:20});assert.equal(list.data.tasks.length,1);const task=list.data.tasks[0];
    const snapshot={schema_version:1,goal:'隔离宿主持久续接',scope:'合成验收',stage:'阶段保存',completion_conditions:['恢复同workflow且确认原提交'],progress:[],attempts:[],verifications:[],unresolved_operations:[{host:'synthetic',operation_id:'external-unknown',description:'外部结果未知',state:'unknown',possible_side_effects:['可能已发布'],verification_method:'用原宿主查结果'}],blockers:[],next_steps:['先核查未知外部操作'],no_references_reason:'合成任务无固定知识'};
    let handoffKnowledge=null;let handoffRefs=[];
    if(handoff){
      const proposed=await invoke(h2,agent,'knowledge_write',{op:'propose',expected:'absent',key:'handoff-auth',scope:{workspace_id:null,path_prefix:null,conditions:[]},claim:'交接认证策略v1：refreshAccessToken失败时先核对原请求，不重复执行未知发布。',evidence_refs:[],new_evidence:[]});assert.equal(proposed.error,false,JSON.stringify(proposed));
      const accepted=await invoke(h2,agent,'knowledge_write',{op:'accept',knowledge_id:proposed.data.result.knowledge.knowledge_id,expected_revision:1,acceptance_basis:{rationale:'合成验收明确接受',evidence_refs:[],agent_report:'由当前Agent提供的可归因现场报告'}});assert.equal(accepted.error,false,JSON.stringify(accepted));handoffKnowledge=accepted.data.result.knowledge;
      handoffRefs=[{knowledge_id:handoffKnowledge.knowledge_id,revision:2,purpose:'本阶段明确采用的认证处理依据',content_sha256:null,selection:'caller_explicit'}];
      snapshot.attempts=[{goal:'恢复认证连接',conditions:['offline','windows'],method:'refreshAccessToken',state:'failed',result:'端口关闭仍无法连接',reason:{state:'known',value:'验证环境离线'},verification_refs:[]}];
    }
    const saved=await invoke(h2,agent,'checkpoint_save',{task_id:task.task_id,expected_revision:task.revision,ownership_epoch:task.ownership_epoch,snapshot,knowledge_refs:handoffRefs,evidence_refs:[],target_status:'in_progress',related_paths:[]});assert.equal(saved.error,false,JSON.stringify(saved));
    if(handoff){const changed=await invoke(h2,agent,'knowledge_write',{op:'revise',knowledge_id:handoffKnowledge.knowledge_id,expected_revision:2,claim:'交接认证策略v2：网络恢复后重新评估，原策略需复核。',evidence_refs:[],new_evidence:[],reason:'保存以后环境发生变化'});assert.equal(changed.error,false,JSON.stringify(changed));}
    const restored=await invoke(h2,agent,'context_resume',{task_id:task.task_id,checkpoint_id:null,cursor:null,byte_budget:1048576});assert.equal(restored.error,false,JSON.stringify(restored));assert.equal(restored.data.saved_context.snapshot.unresolved_operations[0].operation_id,'external-unknown');
    const pendingAfter=await invoke(h2,agent,'pending_requests');assert.equal(pendingAfter.data.pending.length,0);
    const crossClient=afterSave?await afterSave({root,workspace,dataDir,binary,task,saved:saved.data.result,restored:restored.data,context:opened.data,knowledge:handoffKnowledge,invokeDsh:(name,args)=>invoke(h2,agent,name,args)}):null;
    const report={crossClient,status:'passed',dshVersion:requireHost('@deepseek-ai/dsh-agent/package.json').version,sessionId,workflowId:original.workflow_id,firstGeneration:1,resumedGeneration:2,committedOriginalRequest:lostId,taskCount:1,unknownOperationPreserved:true,externalModelCalls:0,userProfileModified:false,root};await fs.writeFile(path.join(root,'report.json'),JSON.stringify(report,null,2));return report;
  }finally {await h2.close(resumed);}
}
