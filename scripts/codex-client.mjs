import {spawn} from 'node:child_process';
import path from 'node:path';
import {promises as fs} from 'node:fs';
export class CodexClient {
 constructor({codex,home,workspace,retenixa,dataDir}){
  const env={...Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('DSH_')&&!k.startsWith('CODEX_')&&!/KEY|PASSWORD|SECRET|TOKEN/i.test(k))),CODEX_HOME:home};
  const overrides={ 'mcp_servers.retenixa.command':retenixa,'mcp_servers.retenixa.args':['serve','--data-dir',dataDir],'mcp_servers.retenixa.startup_timeout_sec':30,'model_provider':'local-verification','model_providers.local-verification.name':'isolated-no-model','model_providers.local-verification.base_url':'http://127.0.0.1:9/v1','model_providers.local-verification.wire_api':'responses','model_providers.local-verification.requires_openai_auth':false,'model':'verification-only-no-model','check_for_update_on_startup':false};
  const args=[];for(const [key,value]of Object.entries(overrides))args.push('-c',key+'='+JSON.stringify(value));args.push('app-server','--listen','stdio://');
  this.child=spawn(codex,args,{env,cwd:workspace,stdio:['pipe','pipe','pipe'],windowsHide:true});this.stderr='';this.notifications=[];this.calls=[];this.pending=new Map();this.counter=0;this.buffer=Buffer.alloc(0);this.diagnostics=[];this.exited=null;
  this.child.stderr.setEncoding('utf8');this.child.stderr.on('data',d=>this.stderr+=d);
  this.child.stdout.on('data',d=>{this.buffer=Buffer.concat([this.buffer,d]);let at;while((at=this.buffer.indexOf(10))>=0){const raw=this.buffer.subarray(0,at);this.buffer=this.buffer.subarray(at+1);const line=raw.toString('utf8').trim();if(!line)continue;if(!line.startsWith('{')){this.diagnostics.push({rawBase64:raw.toString('base64'),reason:'Codex native non-JSON stdout; encoding not assumed'});continue;}let message;try{message=JSON.parse(line);}catch(error){this.protocolError=error.message;continue;}
    if(message.id!==undefined&&this.pending.has(message.id)){const p=this.pending.get(message.id);clearTimeout(p.timer);this.pending.delete(message.id);message.error?p.reject(Error(JSON.stringify(message.error))):p.resolve(message.result);}
    else if(message.id!==undefined){this.child.stdin.write(JSON.stringify({id:message.id,error:{code:-32601,message:'Verification never approves interactive requests'}})+'\n');}
    else this.notifications.push(message);
  }});
  this.child.on('error',e=>{for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(e);}this.pending.clear();});this.child.on('exit',(code,signal)=>{this.exited={code,signal};for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(Error('Codex exit '+code+' '+this.stderr.slice(-2000)));}this.pending.clear();});
 }
 request(method,params){const id=++this.counter;this.calls.push(method);return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{this.pending.delete(id);reject(Error('Codex request deadline: '+method));},45000);this.pending.set(id,{resolve,reject,timer});this.child.stdin.write(JSON.stringify({id,method,params})+'\n');});}
 async start(workspace){this.initialized=await this.request('initialize',{clientInfo:{name:'retenixa-handoff-verification',version:'0.1.0'},capabilities:{experimentalApi:true}});this.child.stdin.write(JSON.stringify({method:'initialized',params:{}})+'\n');this.thread=await this.request('thread/start',{cwd:workspace,ephemeral:true,approvalPolicy:'never',sandbox:'danger-full-access',skipGitRepoCheck:true});this.threadId=this.thread.thread.id;return this;}
 async status(){return this.request('mcpServerStatus/list',{limit:100});}
 async call(tool,args){return this.request('mcpServer/tool/call',{threadId:this.threadId,server:'retenixa',tool,arguments:args});}
 async close(){if(this.exited)return this.exited;const done=new Promise(resolve=>this.child.once('exit',(code,signal)=>resolve({code,signal})));this.child.stdin.end();const timer=setTimeout(()=>this.child.kill(),5000);try{return await done;}finally{clearTimeout(timer);}}
}
export async function probeCodex(config){await fs.mkdir(config.home,{recursive:true});const c=new CodexClient(config);try{await c.start(config.workspace);const status=await c.status();const opened=await c.call('context_open',{workspace_path:config.workspace,client_label:'codex',mode:'fresh',request_id:crypto.randomUUID()});return {initialized:c.initialized,threadId:c.threadId,status,opened,methods:c.calls,stderr:c.stderr};}finally{await c.close();}}
