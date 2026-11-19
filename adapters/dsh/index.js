import { createRequire } from 'node:module';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { randomUUID, createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
const runFile = promisify(execFile);
const adapterVersion = createRequire(import.meta.url)('./package.json').version;
const copy = value => JSON.parse(JSON.stringify(value));
const hash = value => createHash('sha256').update(value).digest('hex');
const writes = new Set(['task_create','task_claim','task_release','checkpoint_save','knowledge_write']);
const definitiveRejections = new Set(['INVALID_INPUT','NOT_FOUND','VERSION_CONFLICT','OWNERSHIP_CHANGED','CLAIM_DENIED','REQUEST_ID_REUSED','REFERENCE_UNAVAILABLE','PROJECT_SCOPE_MISMATCH']);
const noContext = new Set(['contract_get','system_doctor']);
const prefix = 'mcp__retenixa__';
export const name = 'retenixa-dsh-adapter';
export const inject = ['agents','tools','storageDomain','sessionProjections','systemPrompt'];
export async function apply(ctx, config) { await install(ctx, config, createRequire(import.meta.url)); }
function requireConfig(config) {
  for (const field of ['command','dataDir']) {
    if (typeof config[field] !== 'string' || !path.isAbsolute(config[field])) throw new Error('Retenixa '+field+'必须为绝对路径');
  }
}
async function writeBinding(file, record) {
  const secret = JSON.parse(await fs.readFile(file,'utf8'));
  secret.expected_generation=record.expectedGeneration;
  secret.attachment_request_id=record.attachmentRequestId;
  const temporary=file+'.'+randomUUID()+'.tmp';
  const handle=await fs.open(temporary,'wx',0o600);
  try { await handle.writeFile(JSON.stringify(secret));await handle.sync(); } finally {await handle.close();}
  await fs.rename(temporary,file);
}
function cleanedEnv(extra={}) {
  return {...Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('DSH_')&&!/KEY|PASSWORD|SECRET|TOKEN/i.test(k))),...extra};
}
async function projectKeys(workspace, dataDir) {
  const cwd=await fs.realpath(workspace);
  const env=cleanedEnv();for(const key of ['GIT_DIR','GIT_COMMON_DIR','GIT_WORK_TREE','GIT_INDEX_FILE'])delete env[key];
  let location,mainRoot;
  try {const result=await runFile('git',['-C',cwd,'rev-parse','--path-format=absolute','--git-common-dir'],{env,windowsHide:true});const common=await fs.realpath(result.stdout.trim());location='git:'+common;if(path.basename(common)==='.git')mainRoot=await fs.realpath(path.dirname(common));}
  catch(error) {if(error.code==='ENOENT')throw error;location='path:'+cwd;}
  const key=identity=>hash(JSON.stringify([path.resolve(dataDir),identity]));
  return {key:key(location),legacyKey:mainRoot?key('path:'+mainRoot):null,mainRoot};
}
function publicInput(tool) {
  const schema=copy(tool.inputSchema);
  const remove=(node,fields)=>{
    if(node.properties)for(const field of fields)delete node.properties[field];
    if(node.required)node.required=node.required.filter(x=>!fields.includes(x));
    for(const mode of ['oneOf','anyOf','allOf'])for(const child of node[mode]??[])remove(child,fields);
  };
  remove(schema,['context_id',...(writes.has(tool.name)?['request_id']:[])]);
  if(tool.name==='context_open')return {type:'object',description:'不接受参数。项目目录与附着身份由宿主提供。',properties:{},additionalProperties:false};
  if(!noContext.has(tool.name))schema.description=(schema.description?schema.description+' ':'')+'context_id 由宿主提供，不得传入。'+(writes.has(tool.name)?'本次写入的 request_id 由宿主生成，不得传入。':'');
  if(tool.name==='checkpoint_save') {
    const snapshot=schema.$defs?.CheckpointSnapshot;
    if(!snapshot)throw new Error('Retenixa checkpoint schema 缺少快照定义');
    remove(snapshot,['workspace']);
    schema.properties.related_paths={type:'array',description:'相对当前会话工作目录。宿主根据这些路径填写 snapshot.workspace，请勿自行传入该字段。',maxItems:1000,items:{type:'string',minLength:1,maxLength:4096}};
    schema.properties.base_commit={type:['string','null'],pattern:'^([a-fA-F0-9]{40}|[a-fA-F0-9]{64})$'};
  }
  return schema;
}
function checkPublicInput(name,args) {
  if(name==='context_open'&&Object.keys(args).length)throw new Error('Retenixa context_open 不接受参数；项目目录和附着由宿主提供');
  if(Object.hasOwn(args,'context_id')||(writes.has(name)&&Object.hasOwn(args,'request_id')))throw new Error('Retenixa context_id 与本次写入的 request_id 由宿主提供，不得传入');
  if(name==='checkpoint_save'&&args.snapshot&&Object.hasOwn(args.snapshot,'workspace'))throw new Error('Retenixa snapshot.workspace 由宿主填写；请传 related_paths');
}
function prepare(name,args,context) {
  const payload=copy(args);
  if(name==='checkpoint_save') {
    const paths=payload.related_paths??[];delete payload.related_paths;
    const base=payload.base_commit;delete payload.base_commit;
    payload.snapshot.workspace={workspace_id:context.workspace_id,observed_at:new Date().toISOString(),branch:{state:'unknown',reason:'由服务另行采集'},head:{state:'unknown',reason:'由服务另行采集'},base_commit:base?{state:'known',value:base}:{state:'unknown',reason:'尚未明确任务基准提交'},related_files:paths.map(relative=>({path:relative,sha256:{state:'unknown',reason:'由服务另行采集'},observed_at:new Date().toISOString()})),consistency:'unknown'};
  }
  return payload;
}
export async function install(ctx,config,requireHost=createRequire(import.meta.url)) {
  requireConfig(config);
  const {z}=requireHost('zod');
  const {defineDomain,domainTable}=requireHost('@deepseek-ai/dsh-storage-domain');
  const {Client}=requireHost('@modelcontextprotocol/client');
  const {StdioClientTransport}=requireHost('@modelcontextprotocol/client/stdio');
  const {createMcpToolDefinition}=requireHost('@deepseek-ai/dsh-mcp-client');
  const requestFields={bindingKey:z.string(),eventKey:z.string(),requestId:z.string(),operation:z.string(),argsHash:z.string()};
  const payloadSchema=z.record(z.string(),z.unknown());
  const legacyRequestSchema=z.object({...requestFields,payload:payloadSchema,status:z.enum(['prepared','committed','result_purged','rejected']),result:z.unknown().nullable()});
  const requestSchema=z.discriminatedUnion('status',[
    z.object({...requestFields,status:z.literal('prepared'),payload:payloadSchema,result:z.null()}),
    z.object({...requestFields,status:z.enum(['committed','result_purged']),payload:z.null(),result:z.null()}),
    z.object({...requestFields,status:z.literal('rejected'),payload:z.null(),result:payloadSchema}),
  ]);
  const callSchema=z.object({requestId:z.string(),operation:z.string(),argsHash:z.string()});
  const bindingSchema=z.object({file:z.string(),workspacePath:z.string().optional(),expectedGeneration:z.number().int().nonnegative(),attachmentRequestId:z.string(),pending:z.array(z.string()),callIndex:z.record(z.string(),callSchema).default({}),requests:z.record(z.string(),legacyRequestSchema).optional(),lastContext:payloadSchema.nullable()});
  const domain=await ctx.storageDomain.open(defineDomain({name:'retenixa',version:2,compatibleVersions:[1],layout:'single',tables:{bindings:domainTable(bindingSchema)}}));
  const bindings=domain.table('bindings');
  const compactRequest=record=>({...record,payload:record.status==='prepared'?record.payload:null,result:record.status==='rejected'?record.result:null});
  const callPointer=record=>({requestId:record.requestId,operation:record.operation,argsHash:record.argsHash});
  let requestDomain,requests,projection;
  try {
    requestDomain=await ctx.storageDomain.open(defineDomain({name:'retenixa_requests',version:1,layout:'per-record',tables:{requests:domainTable(requestSchema)}}));
    requests=requestDomain.table('requests');
    // 先逐条持久化原请求，再移除旧绑定中的正文；中断后可按相同键继续搬迁。
    for(const [key,binding] of bindings.entries()) {
      if(!binding.requests)continue;
      const callIndex={...binding.callIndex};
      for(const [event,record] of Object.entries(binding.requests)) {
        const requestKey=hash(event),existing=requests.get(requestKey);
        if(existing&&(existing.requestId!==record.requestId||existing.bindingKey!==record.bindingKey||existing.operation!==record.operation||existing.argsHash!==record.argsHash))throw new Error('Retenixa 旧请求与逐记录存储的身份不一致；停止迁移');
        if(!existing)await requests.put(requestKey,compactRequest(record));
        callIndex[requestKey]=callPointer(existing??record);
      }
      const next={...binding,callIndex};delete next.requests;
      await bindings.put(key,next);
    }
  projection=await ctx.sessionProjections.register({key:'retenixa-call-index',stateVersion:1,stateSchema:z.object({calls:z.array(z.object({seq:z.number().int(),id:z.string(),name:z.string(),done:z.boolean()}))}),init:()=>({calls:[]}),apply(state,event){
    if(event.type==='tool/call'&&event.data.name.startsWith(prefix))return {calls:[...state.calls,{seq:event.seq,id:event.data.callId,name:event.data.name,done:false}]};
    if(event.type==='tool/result'&&event.sourceEventSeqs)return {calls:state.calls.map(c=>event.sourceEventSeqs.includes(c.seq)?{...c,done:true}:c)};
    return state;
  }});
  } catch(error) {await requestDomain?.close();await domain.close();throw error;}
  const requestKeys=new Map(),preparedByBinding=new Map();
  function indexRequest(key,record) {
    requestKeys.set(record.requestId,{key,bindingKey:record.bindingKey});
    let pending=preparedByBinding.get(record.bindingKey);
    if(record.status==='prepared') {
      if(!pending){pending=new Set();preparedByBinding.set(record.bindingKey,pending);}
      pending.add(record.requestId);
    } else pending?.delete(record.requestId);
  }
  for(const [key,binding] of bindings.entries())for(const [requestKey,pointer] of Object.entries(binding.callIndex))requestKeys.set(pointer.requestId,{key:requestKey,bindingKey:key});
  for(const [key,record] of requests.entries())indexRequest(key,record);
  const pendingIds=key=>[...new Set([...(bindings.get(key)?.pending??[]),...(preparedByBinding.get(key)??[])])];
  async function saveRequest(record) {
    const next=compactRequest(record),key=hash(record.eventKey);
    await requests.put(key,next);
    indexRequest(key,next);
  }
  const instances=new Set();const installed=new WeakMap();const activeBindings=new Set();
  async function bindingKeyFor(workspace, sessionId) {
    const keys=await projectKeys(workspace,config.dataDir);
    const sessionKey=key=>hash(JSON.stringify([key,sessionId]));
    const primary=sessionKey(keys.key),legacy=keys.legacyKey?sessionKey(keys.legacyKey):null;
    // 同一目录初始化 Git 后沿用已附着的能力及待确认请求；已有 Git 工作流仍可在 worktree 中续接。
    if(legacy&&bindings.get(legacy)&&(workspace===keys.mainRoot||!bindings.get(primary)?.lastContext))return legacy;
    return primary;
  }
  async function attachAgent(agent) {
    if(installed.has(agent))return installed.get(agent);
    const currentWorkspace=()=>config.workspacePath??ctx.workingDirectory?.get(agent.session)??agent.session.header.cwd;
    let workspacePath=currentWorkspace();
    if(typeof workspacePath!=='string'||!path.isAbsolute(workspacePath))throw new Error('Retenixa 需要当前会话的绝对项目目录');
    workspacePath=await fs.realpath(workspacePath);
    let bindingKey=await bindingKeyFor(workspacePath,agent.session.id);
    if(activeBindings.has(bindingKey))throw new Error('Retenixa 同一宿主绑定已有存活适配器；先协调关闭旧 Agent');
    activeBindings.add(bindingKey);
    let failedCleanup;
    try {
    let state=domain.table('bindings').get(bindingKey);
    if(pendingIds(bindingKey).length&&state?.workspacePath)workspacePath=state.workspacePath;
    else if(state){state={...state,workspacePath};await domain.table('bindings').put(bindingKey,state);}
    let file=state?.file??path.join(config.dataDir,'host-bindings','dsh',bindingKey+'.json');
    await runFile(config.command,['host-init','--binding-file',file,'--host','dsh','--host-session',agent.session.id,'--data-dir',config.dataDir],{env:cleanedEnv(),windowsHide:true});
    if(!state){const secret=JSON.parse(await fs.readFile(file,'utf8'));state={file,workspacePath,expectedGeneration:secret.expected_generation,attachmentRequestId:secret.attachment_request_id,pending:[],callIndex:{},lastContext:null};await domain.table('bindings').put(bindingKey,state);}
    let client,transport,tools=new Map(),context=null,closed=false,stale=false,clientReady=false;
    let serial=Promise.resolve();
    const queue=action=>{const next=serial.then(async()=>{await selectWorkspace();return action();});serial=next.catch(()=>{});return next;};
    async function closeTransport(){clientReady=false;try{await client?.close();}finally{await transport?.close();client=null;transport=null;}}
    async function connect(){
      try {
        await writeBinding(file,state);
        client=new Client({name:'retenixa-dsh-adapter',version:adapterVersion},{capabilities:{},versionNegotiation:{mode:'auto'}});
        transport=new StdioClientTransport({command:config.command,args:['serve','--data-dir',config.dataDir,'--binding-file',file],env:cleanedEnv(),cwd:workspacePath,stderr:'pipe'});
        transport.stderr?.on('data',()=>{});client.onerror=()=>{};
        await client.connect(transport);
        tools=new Map((await client.listTools(undefined,{cacheMode:'refresh'})).tools.map(t=>[t.name,t]));
        clientReady=true;
      } catch(error) {await closeTransport();throw error;}
    }
    async function selectWorkspace(){
      const nextWorkspace=await fs.realpath(currentWorkspace());
      if(nextWorkspace===workspacePath){if(!clientReady&&!closed&&!stale)await connect();return;}
      if(closed||stale)throw new Error('Retenixa 适配器已关闭或附着过期；不要自动争抢');
      if(pendingIds(bindingKey).length)throw new Error('Retenixa 原目录 '+workspacePath+' 有结果未知的请求；请先返回原目录通过 pending_requests/retry_request 核查，再切换项目');
      const nextKey=await bindingKeyFor(nextWorkspace,agent.session.id);
      if(nextKey!==bindingKey&&activeBindings.has(nextKey))throw new Error('Retenixa 目标项目已有存活适配器');
      let nextState=domain.table('bindings').get(nextKey);
      const nextFile=nextState?.file??path.join(config.dataDir,'host-bindings','dsh',nextKey+'.json');
      await runFile(config.command,['host-init','--binding-file',nextFile,'--host','dsh','--host-session',agent.session.id,'--data-dir',config.dataDir],{env:cleanedEnv(),windowsHide:true});
      if(!nextState){const secret=JSON.parse(await fs.readFile(nextFile,'utf8'));nextState={file:nextFile,workspacePath:nextWorkspace,expectedGeneration:secret.expected_generation,attachmentRequestId:secret.attachment_request_id,pending:[],callIndex:{},lastContext:null};await domain.table('bindings').put(nextKey,nextState);}
      if(pendingIds(nextKey).length&&nextState.workspacePath&&nextState.workspacePath!==nextWorkspace)throw new Error('Retenixa 目标项目在 '+nextState.workspacePath+' 有结果未知的请求；请先返回该目录核查');
      nextState={...nextState,workspacePath:nextWorkspace};
      await domain.table('bindings').put(nextKey,nextState);
      await closeTransport();
      activeBindings.delete(bindingKey);activeBindings.add(nextKey);
      bindingKey=nextKey;workspacePath=nextWorkspace;file=nextFile;state=nextState;context=null;
      await connect();
    }
    async function raw(operation,payload,signal){if(closed||stale)throw new Error('Retenixa 适配器已关闭或附着过期；不要自动争抢');const tool=tools.get(operation);if(!tool)throw new Error('Retenixa 服务未实现 '+operation);const result=await client.callTool({name:operation,arguments:payload},{toolDefinition:tool,signal,timeout:30000});if(result.isError){if(result.structuredContent?.error?.code==='STALE_ATTACHMENT')stale=true;return result;}return result;}
    async function saveState(next){await domain.table('bindings').put(bindingKey,next);state=next;}
    async function ensureContext(signal){if(context)return context;
      let response=await raw('context_open',{workspace_path:workspacePath,client_label:'dsh',mode:'continue',request_id:state.attachmentRequestId},signal);
      if(response.isError)throw new Error('Retenixa 附着失败：'+response.structuredContent?.error?.code+'；'+response.structuredContent?.error?.message);
      let opened=response.structuredContent;
      await saveState({...state,expectedGeneration:opened.writer_generation,attachmentRequestId:randomUUID(),lastContext:opened});
      if(opened.requires_reattach){await closeTransport();await connect();response=await raw('context_open',{workspace_path:workspacePath,client_label:'dsh',mode:'continue',request_id:state.attachmentRequestId},signal);if(response.isError)throw new Error('Retenixa 重新附着失败：'+response.structuredContent?.error?.code+'；'+response.structuredContent?.error?.message);opened=response.structuredContent;if(opened.requires_reattach)throw new Error('Retenixa 附着协调冲突，停止自动附着');await saveState({...state,expectedGeneration:opened.writer_generation,attachmentRequestId:randomUUID(),lastContext:opened});}
      context=opened;return context;
    }
    async function mark(record,result){
      // 确定的业务拒绝可以结案；传输、存储及过期附着错误仍保留待确认状态。
      const status=result.isError?(definitiveRejections.has(result.structuredContent?.error?.code)?'rejected':null):(['committed','result_purged'].includes(result.structuredContent?.status)?result.structuredContent.status:null);
      if(status){if(record.status!==status)await saveRequest({...record,status,result:status==='rejected'?copy(result):null});if(state.pending.includes(record.requestId))await saveState({...state,pending:state.pending.filter(id=>id!==record.requestId)});}
      return result;
    }
    async function submit(record,signal){
      const key=hash(record.eventKey);
      if(!state.callIndex[key]||!state.pending.includes(record.requestId))await saveState({...state,pending:pendingIds(bindingKey),callIndex:{...state.callIndex,[key]:callPointer(record)}});
      const current=await ensureContext(signal);
      const response=await raw(record.operation,{...copy(record.payload),context_id:current.context_id,request_id:record.requestId},signal);
      return mark(record,response);
    }
    async function confirm(record,signal){
      if(record.status==='rejected')return mark(record,copy(record.result));
      const current=await ensureContext(signal);
      const receipt=await raw('request_get',{context_id:current.context_id,request_id:record.requestId},signal);
      if(receipt.isError)return receipt;
      if(receipt.structuredContent.status!=='not_found')return mark(record,receipt);
      if(record.status!=='prepared')throw new Error('Retenixa 已确认的原请求回执缺失；不会重新执行，请核对恢复的数据');
      return submit(record,signal);
    }
    function eventKey(exec){
      if(!exec.parent){const calls=projection.stateOf(agent.session).calls.filter(c=>!c.done&&c.id===exec.callId&&c.name===exec.name);if(calls.length===1)return bindingKey+':'+calls[0].seq;}
      // 嵌套调用或缺少唯一持久事件时，每次调用独立建档，未知结果通过原 request_id 恢复。
      return bindingKey+':'+randomUUID();
    }
    async function invoke(operation,args,exec){if(exec.agent!==agent)throw new Error('Retenixa 工具不属于当前 Agent');checkPublicInput(operation,args);if(operation==='context_open'){const current=await ensureContext(exec.signal);const index=await raw('task_list',{context_id:current.context_id,status:null,workspace_id:null,parent_task_id:null,cursor:null,limit:20},exec.signal);return {content:[{type:'text',text:JSON.stringify({...current,task_index:index.structuredContent,pending_requests:pendingIds(bindingKey),resume_hint:'对需要接续的任务显式调用 context_resume；不要重执行未知外部操作'})}],structuredContent:current};}
      if(noContext.has(operation))return raw(operation,args,exec.signal);
      const current=await ensureContext(exec.signal);
      if(!writes.has(operation))return raw(operation,{...args,context_id:current.context_id},exec.signal);
      const event=eventKey(exec),key=hash(event),argsHash=hash(JSON.stringify(args));
      const pointer=state.callIndex[key];let record=requests.get(key);
      if(pointer&&!record)throw new Error('Retenixa 原宿主调用的请求记录缺失；不会创建新请求，请恢复宿主持久数据');
      if(record){if(record.bindingKey!==bindingKey||record.operation!==operation||record.argsHash!==argsHash||(pointer&&pointer.requestId!==record.requestId))throw new Error('Retenixa 宿主调用标识已用于不同载荷，未发送');if(record.status!=='prepared')return confirm(record,exec.signal);}
      else{record={bindingKey,eventKey:event,requestId:randomUUID(),operation,payload:prepare(operation,args,current),argsHash,status:'prepared',result:null};await saveRequest(record);}
      return submit(record,exec.signal);
    }
    const registrations=[];
    const instance={get bindingKey(){return bindingKey;},get context(){return context;},async close(){if(closed)return;closed=true;await serial;for(const dispose of registrations.reverse())await dispose();await closeTransport();instances.delete(instance);activeBindings.delete(bindingKey);}};
    installed.set(agent,instance);instances.add(instance);failedCleanup=()=>instance.close();agent.ctx.effect(()=>()=>instance.close());
    await connect();
    for(const tool of tools.values()){const definition=createMcpToolDefinition(agent.ctx,{name:prefix+tool.name,rawName:tool.name,description:tool.description,inputSchema:publicInput(tool),outputSchema:tool.outputSchema,call:(args,exec)=>queue(()=>invoke(tool.name,args,exec))});registrations.push(agent.ctx.tools.register(definition));}
    const helper=(rawName,inputSchema,readOnly,fn)=>{const definition=createMcpToolDefinition(agent.ctx,{name:prefix+rawName,rawName,description:readOnly?'列出本绑定结果未知的原始请求；这些 ID 不是新操作授权':'按宿主持久原始载荷确认/重投结果未知请求，不接受新的载荷或生成新请求 ID',inputSchema,outputSchema:{type:'object'},call:(args,exec)=>queue(async()=>{if(exec.agent!==agent)throw new Error('Retenixa Agent 不匹配');return fn(args,exec);})});registrations.push(agent.ctx.tools.register(definition));};
    helper('pending_requests',{type:'object',properties:{},additionalProperties:false},true,async()=>{const result={pending:pendingIds(bindingKey).map(id=>{const indexed=requestKeys.get(id),r=indexed?requests.get(indexed.key):undefined;return {request_id:id,operation:r?.operation,status:r?.status??'unknown'};})};return {content:[{type:'text',text:JSON.stringify(result)}],structuredContent:result};});
    helper('retry_request',{type:'object',properties:{request_id:{type:'string',minLength:1,maxLength:128}},required:['request_id'],additionalProperties:false},false,async(args,exec)=>{const indexed=requestKeys.get(args.request_id);if(!indexed||indexed.bindingKey!==bindingKey)throw new Error('Retenixa 原请求不属于当前宿主绑定');const record=requests.get(indexed.key);if(!record)throw new Error('Retenixa 原请求记录缺失；不会重新执行，请恢复宿主持久数据');return confirm(record,exec.signal);});
    registrations.push(agent.ctx.systemPrompt.section({name:'retenixa-auto-project',order:9200,text:'Retenixa 绑定当前工作目录。纯只读问答或无需接续的小改动，不建任务、不存检查点。跟踪任务 → context_open 核对、按需 context_resume；阶段完成或结案 → 保存检查点。未知写入 → 核查原请求；未知外部操作 → 核查原宿主，勿重跑。归属或附着冲突 → 停止写入，协调交接。'}));
    return instance;
    } catch(error) {if(failedCleanup)await failedCleanup();activeBindings.delete(bindingKey);installed.delete(agent);throw error;}
  }
  ctx.effect(()=>async()=>{for(const instance of [...instances])await instance.close();await projection.dispose();await requestDomain.close();await domain.close();});
  ctx.on('agent/created',async({agent})=>{await attachAgent(agent);});
  for(const agent of ctx.agents.list())await attachAgent(agent);
  return {attachAgent,domain,requestDomain,projection,instances};
}
