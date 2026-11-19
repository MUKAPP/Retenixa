import {spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import path from 'node:path';
const root=path.resolve(process.argv[2]??'.');
const child=spawn(path.join(root,'target/debug/retenixa.exe'),['serve','--data-dir',path.join(root,'target/raw-probe',randomUUID())],{stdio:['pipe','pipe','pipe'],windowsHide:true});
console.log('spawn',child.pid);child.stdout.setEncoding('utf8');child.stderr.setEncoding('utf8');
let buffer='';let stderr='';let next=1;const pending=new Map();
child.stderr.on('data',d=>{stderr+=d.toString();process.stderr.write(d);});
child.stdout.on('data',d=>{buffer+=d.toString();let i;while((i=buffer.indexOf('\n'))>=0){const raw=buffer.slice(0,i);buffer=buffer.slice(i+1);try{const message=JSON.parse(raw);console.log('response',message.id,message.error??Object.keys(message.result??{}));const p=pending.get(message.id);if(p){clearTimeout(p.timer);pending.delete(message.id);p.resolve(message);}}catch(e){console.error('invalid stdout',e.message);}}});
child.on('error',e=>console.error('spawn error',e.message));
child.on('exit',(code,signal)=>{console.log('exit',code,signal);for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('child exit '+code));}pending.clear();});
function request(method,params){const id=next++;return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{pending.delete(id);reject(Error('timeout '+method));},10000);pending.set(id,{resolve,reject,timer});child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');});}
try {const initialized=await request('initialize',{protocolVersion:'2025-11-25',capabilities:{},clientInfo:{name:'retenixa-probe',version:'0.1.0'}});if(initialized.error)throw Error(JSON.stringify(initialized.error));child.stdin.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})+'\n');const listed=await request('tools/list',{});console.log('tools',listed.result?.tools?.map(t=>t.name));child.stdin.end();}
catch(error){console.error(error.message);child.kill();process.exitCode=1;}
finally {setTimeout(()=>{if(child.exitCode===null){console.error('shutdown boundary; stopping owned probe child');child.kill();}},3000).unref();}
