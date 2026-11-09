import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

function load(name) {
  const state = new Map(), storage = new Map(), calls = [], tasks = [], commands = [];
  let props = {}, rendering = false;
  function el() {
    const obj={items:[],handlers:{}};let proxy;
    proxy=new Proxy(obj,{get(o,k){
      if(k in o)return o[k];
      if(k==='child')return c=>{o.items.push(c);return proxy;};
      if(k==='children')return cs=>{o.items.push(...cs);return proxy;};
      if(k==='when')return (yes,f)=>yes?f(proxy):proxy;
      if(k==='id'||k==='tooltip')return v=>{o[k+'Value']=v;return proxy;};
      if(k==='on_click'||k==='on_mouse_down'||k==='on_mouse_move'||k==='on_mouse_up')return (...a)=>{o.handlers[k]=a.at(-1);return proxy;};
      return ()=>proxy;
    }});return proxy;
  }
  const cx={notify(){},stop_propagation(){calls.push(['stop']);},timer:{every(){}},spawn(f){const p=f(cx);tasks.push(p);return p;}};
  const api={View:class {},div:el,h_flex:el,v_flex:el,svg:el,image:el,
    InputState:{new(){assert.equal(rendering,false,'input state created in render');let value='';return {on(){},value:()=>value,set_value:v=>{value=v;},focus(){throw Error('focus stolen');}};}},
    Input:{new:el},theme:()=>({}),shared:k=>state.get(k),setShared:(k,v)=>state.set(k,v),
    props:()=>props,home:()=>'/scratch',terminals:()=>[{address:'t7',state:'live'}],
    agents:()=>[],panes:()=>[],spaces:()=>[],focused:()=>null,layout:()=>null,window:()=>null,
    setAgentOrder:a=>calls.push(['order',a]),
    run:(...a)=>calls.push(['run',...a]),openOverlay:s=>{calls.push(['open',s]);return 'window';},
    closeOverlay:s=>calls.push(['close',s]),notify:s=>calls.push(['toast',s]),
    nextCommand:()=>new Promise(r=>commands.push(r)),watchFolder:()=>new Promise(()=>{}),
    exec:async (...a)=>{calls.push(['exec',...a]);return {code:0,stdout:'{}'};},
    localStorage:{getItem:k=>storage.get(k)??null,removeItem:k=>storage.delete(k),setItem:(k,v)=>storage.set(k,v)},Date,Map,Set,JSON};
  let code=fs.readFileSync(new URL(`../config/herdr-app/mods/${name}/main.js`,import.meta.url),'utf8');
  code=code.replace(/^import .*;\n/gm,'').replace('export default class','globalThis.Mod = class');
  const context=vm.createContext(api);vm.runInContext(code,context);
  const mod=new context.Mod();
  return {mod,state,storage,calls,cx,api:context,setProps:p=>{props=p;},render(){rendering=true;try{return mod.render(cx);}finally{rendering=false;}},async flush(){await Promise.all(tasks.filter(p=>p));}};
}
const row=()=>({schema:2,project:'project-a',checkout:'/repo with spaces',at:Date.now()/1000,decisions:1,
  decision_texts:[{id:'b-1',title:'Choose',question:'What next?',project:'project-a'}],waiting_panes:[]});

test('new decisions open once, timestamps and waiters do not reopen; stale decisions ignored',async()=>{
  const h=load('vega');const r=row();let rows=[r];
  h.mod.seen=new Set();h.mod.watching=true;
  h.api.exec=async()=>({code:0,stdout:JSON.stringify({folder:'/badges',rows})});
  await h.mod.badges(h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='open').length,1);
  assert.equal(h.calls.filter(c=>c[0]==='toast').length,1);
  r.at++;r.waiting_panes=[{pane:'t7'}];await h.mod.badges(h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='open').length,1);
  const refreshes=()=>h.calls.filter(c=>c[0]==='run'&&c[1]==='vega-inbox.refresh').length;
  assert.equal(refreshes(),0,'a closed inbox has no refresh command');
  h.state.set('vega.inbox-open',true);r.decision_texts.push({...r.decision_texts[0],id:'b-2'});
  await h.mod.badges(h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='open').length,1,'existing input is preserved');
  assert.equal(h.calls.filter(c=>c[0]==='toast').length,2);
  assert.equal(refreshes(),1);
  rows=[{...row(),at:1}];await h.mod.badges(h.cx);
  assert.equal(h.state.get('vega.badges').length,0);
  assert.equal(refreshes(),2);
});

test('answer and dismiss use argv and the recorded project cwd; inputs never autofocus',async()=>{
  const h=load('vega-inbox');const r=row(),d=r.decision_texts[0];
  h.state.set('vega.badges',[r]);h.mod.init({},h.cx);h.render();
  h.mod.act(r,d,false,h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='exec').length,0,'empty answer rejected');
  const answer='yes "quoted" $(not-a-shell)\nsecond line';h.mod.inputs.get('project-a/b-1').set_value(answer);
  h.mod.act(r,d,false,h.cx);await new Promise(r=>setImmediate(r));
  const call=h.calls.find(c=>c[0]==='exec');
  assert.equal(call[1],'vega');assert.equal(JSON.stringify(call[2]),JSON.stringify(['uac','answer','b-1',answer]));
  assert.equal(call[3].cwd,'/repo with spaces');
  h.mod.act(r,d,true,h.cx);await new Promise(r=>setImmediate(r));
  const dismiss=h.calls.filter(c=>c[0]==='exec')[1];assert.equal(dismiss[1],'bd');
  assert.equal(JSON.stringify(dismiss[2]),JSON.stringify(['close','b-1','--reason',answer]));
  h.api.exec=async()=>({code:1,stdout:'failure'});h.mod.act(r,d,false,h.cx);await new Promise(r=>setImmediate(r));
  assert.equal(h.mod.results.get('project-a/b-1'),'failure');
});

test('each agent icon stops row propagation and routes its own action',()=>{
  const h=load('vega-sidebar');
  h.mod.vegaTerminals=new Map([['t7',{labels:{'vega.mail':'2','vega.decisions':'1','vega.waiting':'1','vega.kind':'C','vega.uac':'UAC','vega.git':'dirty'}}]]);
  const icons=h.mod.vegaIcons('t7',{}).items;
  assert.equal(icons.length,4,'uac and kind sit before the name, as in the TUI row');
  for(const icon of icons)icon.handlers.on_click({},h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='stop').length,4);
  h.mod.vegaTerminals.set('t8',{labels:{'vega.git':'\u2800'}});
  assert.equal(h.mod.vegaIcons('t8',{}).items.length,0,'a blank token draws nothing');
  assert(h.calls.some(c=>c[0]==='open'&&c[1].mod==='vega-inbox'));
  assert(h.calls.some(c=>c[0]==='open'&&c[1].props?.mail==='t7'));
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='agent.focus'));
});

// Exercise the Python transport against a real subprocess fake, including hc's
// base64 wrapper. The fake never contacts a core or runs a decision command.
import os from 'node:os';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
function transportFixture() {
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'vega-badges-'));
  const log=path.join(dir,'calls.jsonl');
  fs.writeFileSync(path.join(dir,'hc'),`#!/usr/bin/env python3
import sys,json,base64,os
args=sys.argv[1:]
with open(os.environ['TEST_HC_LOG'],'a') as f: f.write(json.dumps(args)+'\\n')
if args[:2]==['machine','ls']:
 if os.environ.get('TEST_MACHINE_SLOW'): __import__('time').sleep(1)
 print(json.dumps({'machines':[{'name':n,'state':'connected'} for n in ['ganymede','broken']]+[{'name':'offline','state':'disconnected'}]}))
else:
 name=args[args.index('--on')+1]
 if name=='broken': print('not JSON'); sys.exit(0)
 program=args[args.index('--')+1]
 if program=='python3':
  row=json.loads(os.environ['TEST_BADGE_ROW'])
  row['waiting_panes']=[{'pane':'hc:t7','name':'Remote','attention':'asking'},{'pane':'w1:p1'},{'pane':'other/t8'}]
  body=json.dumps({'rows':[row]})
  code=0
 else:
  body='answer received: '+args[-1]
  code=int(os.environ.get('TEST_REMOTE_CODE','0'))
 print(json.dumps({'stdout':base64.b64encode(body.encode()).decode(),'stderr':base64.b64encode('remote diagnostic'.encode()).decode(),'code':code}))
`,{mode:0o755});
  const env={...process.env,PATH:dir+':'+process.env.PATH,TEST_HC_LOG:log,TEST_BADGE_ROW:JSON.stringify(row()),XDG_STATE_HOME:path.join(dir,'state')};
  return {dir,env,log,run(script,args=[],extra={}){return JSON.parse(execFileSync('python3',['-c',script,...args],{env:{...env,...extra},encoding:'utf8'}));},close(){fs.rmSync(dir,{recursive:true,force:true});}};
}
test('remote badges unwrap hc JSON, qualify only local hc waiters, isolate an unavailable machine',()=>{
  const h=load('vega'),f=transportFixture();
  try {
    const result=f.run(vm.runInContext('collectBadges',h.api),[vm.runInContext('readBadges',h.api),'[]']);
    assert.equal(result.rows.length,1);
    assert.equal(result.rows[0].machine,'ganymede');
    assert.equal(result.rows[0].decision_texts[0].question,'What next?');
    assert.equal(result.rows[0].checkout,'/repo with spaces');
    assert.deepEqual(result.rows[0].waiting_panes,[{pane:'hc:ganymede/t7',name:'Remote',attention:'asking'}]);
    assert(result.errors.some(e=>e.source==='broken' && e.message==='hc broken badges unavailable'));
    assert(!fs.readFileSync(f.log,'utf8').includes('"--on", "offline"'));
  } finally { f.close(); }
});
test('remote action retains Unicode argv, remote cwd and wrapped failure status',()=>{
  const h=load('vega-inbox'),f=transportFixture();
  try {
    const answer='Grüße "yes" $(never-run)\nnext';
    const args=['ganymede','/remote repo','vega','uac','answer','b-1',answer];
    const result=f.run(vm.runInContext('remoteAction',h.api),args,{TEST_REMOTE_CODE:'9'});
    assert.equal(result.code,9);assert.equal(result.stdout,'answer received: '+answer);assert.equal(result.stderr,'remote diagnostic');
    const call=JSON.parse(fs.readFileSync(f.log,'utf8').trim());
    assert.deepEqual(call,['exec','--json','--on','ganymede','--cwd','/remote repo','--timeout','30','--','vega','uac','answer','b-1',answer]);
  } finally { f.close(); }
});
test('same decision on two machines has separate inputs and notifications; remote actions never execute locally',async()=>{
  const status=load('vega'),h=load('vega-inbox');const a=row(),b={...row(),machine:'ganymede'};
  status.mod.seen=new Set();status.mod.watching=true;
  status.api.exec=async()=>({code:0,stdout:JSON.stringify({rows:[a,b],folder:'/badges'})});
  await status.mod.badges(status.cx);
  assert.equal(status.mod.seen.size,2);
  h.state.set('vega.badges',[a,b]);h.mod.init({},h.cx);h.render();
  assert.equal(h.mod.inputs.size,2);
  h.mod.inputs.get('ganymede/project-a/b-1').set_value('remote answer');
  h.api.exec=async(...args)=>{h.calls.push(['exec',...args]);return {code:0,stdout:JSON.stringify({code:7,stdout:'remote failure',stderr:''})};};
  h.mod.act(b,b.decision_texts[0],false,h.cx);await new Promise(r=>setImmediate(r));
  const call=h.calls.find(c=>c[0]==='exec');
  assert.equal(call[1],'python3');
  assert.deepEqual(Array.from(call[2].slice(2)),['ganymede','/repo with spaces','vega','uac','answer','b-1','remote answer']);
  assert.equal(h.mod.results.get('ganymede/project-a/b-1'),'remote failure');
});

test('project group totals match source machine as well as project and expire',()=>{
  const h=load('vega-sidebar');
  const local={...row(),decisions:2},remote={...row(),machine:'ganymede',decisions:5};
  // Remote first catches a regression to project-only matching for the local group.
  h.state.set('vega.badges',[remote,local]);
  assert.equal(h.mod.vegaTotals('t7','project-a').decisions,2);
  assert.equal(h.mod.vegaTotals('ganymede/t7','project-a').decisions,5);
  assert.equal(h.mod.vegaTotals('other/t7','project-a'),undefined);
  assert.equal(h.mod.vegaTotals('ganymede/t7','another-project'),undefined);
  remote.at=1;
  assert.equal(h.mod.vegaTotals('ganymede/t7','project-a'),undefined);
});


function elements(node) {
  if (!node || typeof node !== 'object') return [];
  return [node,...node.items.flatMap(elements)];
}
function texts(node) { return elements(node).flatMap(n=>n.items.filter(i=>typeof i==='string')); }

test('UAC projects hold their agents across machines in dragged order; other spaces follow',()=>{
  const h=load('vega-sidebar');
  const agent=(address,space)=>({address,space_name:'x',space,tab:'tb',pane:address,status:'idle',program:{key:'claude'},seq:1});
  h.api.spaces=()=>[
    {id:'sp1',name:'lanista',machine:null,branch:null,active:false,tabs:[]},
    {id:'ganymede/sp2',name:'lanista',machine:'ganymede',branch:'feat/x',active:false,tabs:[]},
    {id:'sp3',name:'scratch',machine:null,branch:null,active:false,tabs:[]},
    {id:'sp4',name:'vega',machine:null,branch:null,active:false,tabs:[]},
  ];
  h.api.agents=()=>[agent('t5','sp1'),agent('ganymede/t2','ganymede/sp2'),agent('t7','sp4')];
  h.api.terminals=()=>[
    {address:'ganymede/t2',state:'live',labels:{'herdr.space':'sp2','vega.host':'♃₃','vega.role':'uac','vega.uac':'u▲c','vega.git':'✓','vega.project-id':'github.com/team/lanista','vega.project':'github.com/team/lanista'}},
    {address:'ganymede/t3',state:'live',parent:'ganymede/t2',labels:{'herdr.space':'sp2','vega.project':'lanista','vega.label':'⚙ feat/x'}},
    {address:'t5',state:'live',labels:{'herdr.space':'sp1','vega.host':'♆₈','vega.project':'lanista'}},
    {address:'t6',state:'live',labels:{'herdr.space':'sp3','vega.host':'♆₈'}},
    {address:'t7',state:'live',labels:{'herdr.space':'sp4','vega.host':'♆₈','vega.role':'uac','vega.project':'github.com/team/vega'}},
  ];
  h.state.set('vega.badges',[
    {...row(),project:'github.com/team/lanista',notes:9,decisions:0,waiting:0},
    {...row(),machine:'ganymede',project:'github.com/team/lanista',notes:2,decisions:0,waiting:0},
  ]);
  h.mod.init({},h.cx);let tree=h.render(), nodes=elements(tree), text=texts(tree);
  let ids=nodes.map(n=>n.idValue).filter(i=>typeof i==='string');
  const at=i=>ids.indexOf(i);
  assert.equal(ids.filter(i=>i.startsWith('uac-')).length,2,'one group per UAC project');
  assert(at('uac-lanista')<at('ag-ganymede/t2')&&at('ag-ganymede/t2')<at('ag-t5')&&at('ag-t5')<at('uac-vega'),'UAC first, then the project\'s agents on other machines');
  assert(at('sub-ganymede/t3')>at('ag-ganymede/t2'),'sentinel is a branch of its UAC');
  assert(text.includes('feat/x')&&!text.some(s=>s.includes('⚙')),'sentinel name without the cog');
  assert(text.includes('♃₃')&&text.includes('♆₈'),'machine symbol on every row');
  assert(text.includes('Spaces')&&at('space-sp3')>at('uac-vega'),'a space without a UAC follows under Spaces');
  assert(!ids.includes('space-sp1'),'no folder rows inside a UAC project');
  assert(text.includes('✉ 2'));assert(!text.includes('✉ 9'),'totals come from the UAC\'s machine');
  // Drag vega above lanista; the order is kept.
  const press=(id,ev)=>elements(h.render()).find(n=>n.idValue===id).handlers[ev]({},h.cx);
  press('uac-vega','on_mouse_down');press('uac-lanista','on_mouse_move');press('uac-lanista','on_mouse_up');
  ids=elements(h.render()).map(n=>n.idValue).filter(i=>typeof i==='string');
  assert(ids.indexOf('uac-vega')<ids.indexOf('uac-lanista'),'dragged project moves up');
  assert.deepEqual(JSON.parse(h.storage.get('project-order')),['vega','lanista']);
  const click=elements(h.render()).find(n=>n.idValue==='space-sp3');click.handlers.on_click({},h.cx);
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='space.select'&&c[2].id==='sp3'),'a space row selects that space');
});

test('mail archive and notification JSON render readable cards and empty/error states',async()=>{
  for(const notes of [false,true]) {
    const h=load('vega-sidebar');h.setProps({mail:'ganymede/t7',notes,overlay:'mail-window'});
    const message={from:'t2@io',host:'io',time:'2026-11-07T10:00:00Z',read:false,unread:true,title:'Notice',body:'Line one\nLine two',text:'Line one\nLine two'};
    h.api.exec=async(...a)=>{h.calls.push(['exec',...a]);return {code:0,stdout:notes?JSON.stringify(message)+'\n':JSON.stringify({total:1,messages:[message]})};};
    h.mod.init({},h.cx);await h.flush();
    const content=texts(h.render());assert(content.includes(message.text));assert(content.some(s=>s.includes('Unread')));
    const call=h.calls.find(c=>c[0]==='exec');assert(call[2].includes('--json'));assert.equal(call[3].on,'ganymede');
    assert(!content.some(s=>s.startsWith('{')));
    h.mod.mail=[];assert(texts(h.render()).includes('No mail.'));
    h.api.exec=async()=>({code:1,stderr:'Broker unavailable'});h.mod.init({},h.cx);await h.flush();
    assert(texts(h.render()).some(s=>s.includes('Broker unavailable')));
  }
});

test('a finished intermediate parent cannot hide a live child below a listed grandparent',()=>{
  const h=load('vega-sidebar');
  const agent=address=>({address,space_name:'vega',space:'s',tab:'tb',pane:address,status:'idle',program:{},seq:1,pinned:false});
  h.api.agents=()=>[agent('t1'),agent('t3')];
  h.api.terminals=()=>[
    {address:'t1',state:'live',parent:null,labels:{}},
    {address:'t2',state:'finished',parent:'t1',labels:{}},
    {address:'t3',state:'live',parent:'t2',labels:{}},
  ];
  h.mod.init({},h.cx);const tree=h.render();
  const order=h.calls.find(c=>c[0]==='order')[1];
  assert.equal(JSON.stringify(order),JSON.stringify(['t1','t3']));
  assert(elements(tree).some(n=>n.idValue==='ag-t3'), 'live child has a visible row');
  assert.equal(elements(tree).filter(n=>n.idValue==='sub-t3').length,0,'child has its own top-level row');
  h.api.terminals=()=>[{address:'t3',state:'live',parent:'missing',labels:{}}];
  h.api.agents=()=>[agent('t3')];h.render();
  assert.equal(h.calls.filter(c=>c[0]==='order').at(-1)[1][0],'t3','missing parents also keep a live root visible');
});

test('inbox closes after the last card disappears, respecting project scope and live waiting panes',()=>{
  const h=load('vega-inbox');h.setProps({overlay:'inbox-window',project:'project-a'});
  const r=row();h.state.set('vega.badges',[r]);h.mod.init({},h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='close').length,0);
  r.decision_texts=[];r.waiting_panes=[{pane:'hc:t7',name:'Worker'}];h.mod.prepare(h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='close').length,0);
  h.api.terminals=()=>[];h.mod.prepare(h.cx);
  assert(h.calls.some(c=>c[0]==='close'&&c[1]==='inbox-window'));assert.equal(h.state.get('vega.inbox-open'),false);
  h.calls.length=0;h.state.set('vega.badges',[{...row(),project:'other'}]);h.mod.prepare(h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='close').length,1,'another project cannot keep a filtered inbox open');
});

test('successful Answer and Dismiss close the last card immediately; errors keep it open',async()=>{
  for(const dismiss of [false,true]) {
    const h=load('vega-inbox');const r=row();h.setProps({overlay:'answer-window'});
    h.state.set('vega.badges',[r]);h.mod.init({},h.cx);
    h.mod.inputs.get('project-a/b-1').set_value('Proceed');
    h.api.exec=async()=>({code:1,stderr:'Failed'});
    h.mod.act(r,r.decision_texts[0],dismiss,h.cx);await new Promise(r=>setImmediate(r));
    assert.equal(h.calls.filter(c=>c[0]==='close').length,0);
    h.api.exec=async()=>({code:0,stdout:'Done'});
    h.mod.act(r,r.decision_texts[0],dismiss,h.cx);await new Promise(r=>setImmediate(r));
    assert(h.calls.some(c=>c[0]==='close'&&c[1]==='answer-window'));
    assert.equal(h.mod.rows()[0].decision_texts.length,0,'stale badge cannot keep an answered card visible');
  }
});

test('empty initial mod check has null props and no overlay to close',()=>{
  const h=load('vega-inbox');h.setProps(null);h.mod.init({},h.cx);h.render();
  assert.equal(h.calls.filter(c=>c[0]==='close').length,0);
});

test('pinned listed child stays once under its live parent and once in navigation',()=>{
  const h=load('vega-sidebar');
  const agent=(address,pinned)=>({address,pinned,space:'s',space_name:'vega',tab:'tb',pane:address,status:'idle',program:{},seq:1});
  h.api.agents=()=>[agent('t1',false),agent('t2',true)];
  h.api.terminals=()=>[{address:'t1',state:'live',parent:null,labels:{}},{address:'t2',state:'live',parent:'t1',labels:{'herdr.pinned':'true'}}];
  h.mod.init({},h.cx);const nodes=elements(h.render());
  assert.equal(nodes.filter(n=>n.idValue==='sub-t2').length,1);
  assert.equal(nodes.filter(n=>n.idValue==='ag-t2').length,0);
  assert.equal(nodes.filter(n=>n.idValue==='pinned').length,0);
  assert.equal(JSON.stringify(h.calls.find(c=>c[0]==='order')[1]),JSON.stringify(['t1','t2']));
});

test('remote completion and failures stay machine-scoped and close only the last remaining card',async()=>{
  for(const dismiss of [false,true]) {
    const h=load('vega-inbox'),local=row(),remote={...row(),machine:'ganymede'};
    h.setProps({overlay:'remote-window'});h.state.set('vega.badges',[local,remote]);h.mod.init({},h.cx);
    h.mod.inputs.get('ganymede/project-a/b-1').set_value('Remote answer');
    for(const result of [{code:3,stderr:'Transport failed'},{code:0,stdout:JSON.stringify({code:9,stdout:'Inner failed',stderr:''})}]) {
      h.api.exec=async(...a)=>{h.calls.push(['exec',...a]);return result;};
      h.mod.act(remote,remote.decision_texts[0],dismiss,h.cx);await new Promise(r=>setImmediate(r));
      assert.equal(h.mod.completed.size,0);assert.equal(h.mod.rows()[1].decision_texts.length,1);
      assert.equal(h.calls.filter(c=>c[0]==='close').length,0);
    }
    h.api.exec=async(...a)=>{h.calls.push(['exec',...a]);return {code:0,stdout:JSON.stringify({code:0,stdout:'Sent',stderr:''})};};
    h.mod.act(remote,remote.decision_texts[0],dismiss,h.cx);await new Promise(r=>setImmediate(r));
    assert(h.calls.filter(c=>c[0]==='exec').every(c=>c[1]==='python3'),'remote actions never run bd/vega locally');
    assert(h.mod.completed.has('ganymede/project-a/b-1'));
    assert.equal(h.mod.rows()[1].decision_texts.length,0);
    assert.equal(h.mod.rows()[0].decision_texts.length,1,'local card with the same ID remains');
    assert.equal(h.calls.filter(c=>c[0]==='close').length,0);
    h.mod.inputs.get('project-a/b-1').set_value('Local answer');h.api.exec=async()=>({code:0,stdout:'Sent'});
    h.mod.act(local,local.decision_texts[0],dismiss,h.cx);await new Promise(r=>setImmediate(r));
    assert.equal(h.calls.filter(c=>c[0]==='close'&&c[1]==='remote-window').length,1);
  }
});

test('only the qualified remote waiting terminal keeps an otherwise empty inbox open',()=>{
  const h=load('vega-inbox');const remote={...row(),machine:'ganymede',decision_texts:[],waiting_panes:[{pane:'hc:ganymede/t7',name:'Remote'}]};
  h.setProps({overlay:'waiting-window'});h.state.set('vega.badges',[remote]);
  h.api.terminals=()=>[{address:'ganymede/t7',state:'live'}];h.mod.init({},h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='close').length,0);
  assert.equal(h.mod.waiting(h.mod.rows())[0].machine,'ganymede');
  h.api.terminals=()=>[{address:'t7',state:'live'}];h.mod.prepare(h.cx);
  assert(h.calls.some(c=>c[0]==='close'&&c[1]==='waiting-window'),'same bare ID on this machine is unrelated');
});

test('worktree placeholder shells stay out of the list and the tree',()=>{
  const h=load('vega-sidebar');
  const agent=address=>({address,space_name:'vega',space:'s',tab:'tb',pane:address,status:'done',program:{},seq:1,pinned:false});
  h.api.agents=()=>[agent('t1'),agent('t3c')];
  h.api.terminals=()=>[
    {address:'t1',state:'live',parent:null,labels:{}},
    {address:'t3c',state:'live',parent:'t1',labels:{'vega.worktree-owner':'t3d'}},
    {address:'t3d',state:'live',parent:'t1',labels:{}},
  ];
  h.mod.init({},h.cx);const nodes=elements(h.render());
  assert(!nodes.some(n=>n.idValue==='ag-t3c'||n.idValue==='sub-t3c'),'placeholder has no row');
  assert(nodes.some(n=>n.idValue==='sub-t3d'),'its sentinel still shows');
});

test('vega mod draws the shipped status line without the palette hint',()=>{
  const h=load('vega');
  h.api.spaces=()=>[{id:'sp1',name:'vega',machine:'ganymede',branch:'main',active:true,tabs:[]}];
  h.api.agents=()=>[{address:'t1',status:'blocked'},{address:'t2',status:'idle'}];
  let text=texts(h.render());
  assert(text.includes('ganymede')&&text.includes('main'),'machine and branch of the shown space');
  assert(text.includes('2 agents')&&text.includes('· 1 needs you'));
  assert(!text.some(s=>/commands|vegad/.test(s)),'no palette hint, no vegad line');
  h.api.spaces=()=>[];h.api.agents=()=>[];
  text=texts(h.render());
  assert(text.includes('This computer')&&text.includes('0 agents'),'falls back to this computer');
  assert(!text.some(s=>s.startsWith('vega:')),'no error, no error text');
  h.mod.error='hc machines: timeout';
  assert(texts(h.render()).includes('vega: hc machines: timeout'),'a badge read error shows in the status line');
});

test('theme.toml symbol colours override defaults and key host colours by label',async()=>{
  const h=load('vega-sidebar');
  const toml='[symbols]\nset = "nerd"\n[symbols.colors]\nnotes = "#e0a020"\n[symbols.hosts]\nproteus = "♆₈"\nganymede = "♃₃"\n[symbols.host_colors]\nproteus = "#3d73cc"\n[other]\nnotes = "#000000"\n';
  h.api.exec=async(cmd,args)=>{h.calls.push(['exec',cmd,args]);return {code:0,stdout:toml};};
  h.mod.init({},h.cx);await h.flush();
  assert(h.calls.some(c=>c[0]==='exec'&&c[2][0]==='/scratch/.config/vega/theme.toml'),'reads the vega theme file');
  const c=h.mod.colors();
  assert.equal(c.notes,'#e0a020','an override wins');
  assert.equal(c.codex,'#0e8f6f','a default stays');
  assert.equal(c.hosts['♆₈'],'#3d73cc','host colour keyed by its label');
  assert.equal(c.hosts['♃₃'],undefined,'a host without a colour falls back to the theme');
  // A remote space row shows its machine's label, taken from that machine's terminals.
  h.api.spaces=()=>[{id:'ganymede/sp9',name:'tmp',machine:'ganymede',branch:null,active:false,tabs:[]}];
  h.api.terminals=()=>[{address:'ganymede/t9',state:'live',labels:{'vega.host':'♃₃'}},{address:'t1',state:'live',labels:{'vega.host':'♆₈'}}];
  const text=texts(h.render());
  assert(text.includes('♃₃'),'remote space row names its machine by label');
});

for (const source of ['machines','ganymede']) test(`badge failures retain last good state and debounce ${source}`,async()=>{
  const h=load('vega');h.mod.seen=new Set();h.mod.watching=true;
  const r={...row(),machine:'ganymede'};
  let data={rows:[r],machines:[{name:'ganymede',state:'connected'}]};
  h.api.exec=async()=>({code:0,stdout:JSON.stringify(data)});
  await h.mod.badges(h.cx);
  data={rows:source==='machines'?[r]:[],errors:[{source,message:'hc read slow (12 s)'}]};
  await h.mod.badges(h.cx);
  assert.equal(h.mod.error,null);
  assert.equal(h.state.get('vega.badges').length,1);
  await h.mod.badges(h.cx);assert.equal(h.mod.error,null);
  await h.mod.badges(h.cx);assert.equal(h.mod.error,'hc read slow (12 s)');
  data={rows:[]};await h.mod.badges(h.cx);
  assert.equal(h.mod.error,null);assert.equal(h.state.get('vega.badges').length,0);
  data={rows:[],errors:[{source,message:'hc read unavailable'}]};
  await h.mod.badges(h.cx);assert.equal(h.mod.error,null,'success resets the streak');
});

test('whole reader failures keep badges and report a short error only after three failures',async()=>{
  const h=load('vega');h.mod.seen=new Set();h.mod.watching=true;
  h.state.set('vega.badges',[row()]);
  h.api.exec=async(bin)=>bin==='python3'?{code:1,stderr:'raw exception'}:{code:0,stdout:'{"running":true}'};
  await h.mod.refresh(h.cx);assert.equal(h.mod.error,null);
  assert.equal(h.state.get('vega.badges').length,1);
  await h.mod.refresh(h.cx);await h.mod.refresh(h.cx);
  assert.equal(h.mod.error,'badge reader unavailable');
});

test('relay starts only in the real default app and state homes',()=>{
  const h=load('vega'),dir=fs.mkdtempSync(path.join(os.tmpdir(),'vega-start-'));
  try {
    const bin=path.join(dir,'vega-post'),log=path.join(dir,'started');
    fs.writeFileSync(bin,'#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_START_LOG"\n',{mode:0o755});
    const env={...process.env,HOME:dir,TEST_START_LOG:log};
    for(const key of ['HERDR_APP_HOME','XDG_CONFIG_HOME','XDG_STATE_HOME']) delete env[key];
    const run=extra=>execFileSync('python3',['-c',vm.runInContext('startRelay',h.api),bin,'relay','start'],{env:{...env,...extra}});
    for(const extra of [
      {HERDR_APP_HOME:path.join(dir,'scratch-app')},
      {XDG_STATE_HOME:path.join(dir,'scratch-state')},
      {XDG_CONFIG_HOME:path.join(dir,'scratch-config')},
      {HERDR_APP_HOME:path.join(dir,'scratch-app'),XDG_STATE_HOME:path.join(dir,'scratch-state')},
    ]) run(extra);
    assert(!fs.existsSync(log),'scratch environments never launch the relay');
    run({});
    run({HERDR_APP_HOME:path.join(dir,'.config/herdr-app'),XDG_STATE_HOME:path.join(dir,'.local/state')});
    assert.deepEqual(fs.readFileSync(log,'utf8').trim().split('\n'),['relay start','relay start']);
  } finally {fs.rmSync(dir,{recursive:true,force:true});}
});

test('timed-out machine list uses the last inventory and still reads connected remotes',()=>{
  const h=load('vega'),f=transportFixture();
  try {
    // Shorten the transport deadline for the fake slow command; no real hc/core is used.
    const script=vm.runInContext('collectBadges',h.api).replace('timeout=12','timeout=0.1');
    const result=f.run(script,[vm.runInContext('readBadges',h.api),JSON.stringify([{name:'ganymede',state:'connected'}])],{TEST_MACHINE_SLOW:'1'});
    assert.deepEqual(result.errors,[{source:'machines',message:'hc machine ls slow (0.1 s)'}]);
    assert.equal(result.machines[0].name,'ganymede');
    assert.equal(result.rows[0].machine,'ganymede');
  } finally {f.close();}
});

// The remote starter runs through a fake hc whose exec arguments are checked against the
// vendored hc schema; its argv really executes, so the shell snippet and $HOME are exercised.
test('remote vegad start goes through hc exec, only in real homes, and runs relay start there',()=>{
  const h=load('vega'),dir=fs.mkdtempSync(path.join(os.tmpdir(),'vega-remote-'));
  try {
    const schema=JSON.parse(fs.readFileSync(new URL('../crates/vega-herdr/schema/hc-0.1.18.openrpc.json',import.meta.url),'utf8'));
    const names=schema.methods.find(m=>m.name==='machine.exec').params.map(p=>p.name);
    fs.mkdirSync(path.join(dir,'.local/bin'),{recursive:true});
    fs.writeFileSync(path.join(dir,'.local/bin/vega-post'),'#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_START_LOG"\n',{mode:0o755});
    fs.writeFileSync(path.join(dir,'hc'),`#!/usr/bin/env python3
import sys,json,base64,subprocess
a=sys.argv[1:]; assert a[0]=='exec'
flags={'--json':'json','--on':'on','--timeout':'timeout'}
params=json.loads(${JSON.stringify(JSON.stringify(names))})
i=1
while a[i]!='--':
 assert a[i] in flags and flags[a[i]] in ['json']+params, a[i]
 i+=2 if a[i]!='--json' else 1
p=subprocess.run(a[i+1:],capture_output=True)
print(json.dumps({'stdout':base64.b64encode(p.stdout).decode(),'stderr':base64.b64encode(p.stderr).decode(),'code':p.returncode}))
`,{mode:0o755});
    const log=path.join(dir,'started');
    const env={...process.env,HOME:dir,PATH:dir+':'+process.env.PATH,TEST_START_LOG:log};
    for(const key of ['HERDR_APP_HOME','XDG_CONFIG_HOME','XDG_STATE_HOME']) delete env[key];
    const args=['hc','exec','--json','--on','ganymede','--timeout','20','--','sh','-c',vm.runInContext('remoteStart',h.api)];
    const run=extra=>JSON.parse(execFileSync('python3',['-c',vm.runInContext('startRelay',h.api),...args],{env:{...env,...extra},encoding:'utf8'}));
    assert.deepEqual(run({XDG_STATE_HOME:path.join(dir,'scratch')}),{skipped:true});
    assert(!fs.existsSync(log),'a scratch app never starts a remote vegad');
    const outer=run({}),inner=JSON.parse(outer.stdout);
    assert.equal(outer.code,0);assert.equal(inner.code,0);
    assert.equal(fs.readFileSync(log,'utf8').trim(),'relay start');
  } finally {fs.rmSync(dir,{recursive:true,force:true});}
});

test('vega mod starts vegad on connected machines now, then slowly; failures show after three',async()=>{
  const h=load('vega');let now=1e12,fail=false;
  h.api.Date={now:()=>now};
  const starts=()=>h.calls.filter(c=>c[0]==='exec'&&c[2].includes('--on')).map(c=>c[2][c[2].indexOf('--on')+1]);
  h.api.exec=async(...a)=>{h.calls.push(['exec',...a]);
    return {code:0,stdout:JSON.stringify(fail?{code:0,stdout:JSON.stringify({code:1})}:{code:0,stdout:JSON.stringify({code:0})})};};
  h.mod.started=new Map();h.mod.machines=[{name:'ganymede',state:'connected'},{name:'off',state:'disconnected'}];
  await h.mod.startRemotes(h.cx);
  assert.deepEqual(starts(),['ganymede'],'connected machines only');
  const call=h.calls.find(c=>c[0]==='exec');
  assert.deepEqual(Array.from(call[2].slice(2)),['hc','exec','--json','--on','ganymede','--timeout','20','--','sh','-c','exec "$HOME/.local/bin/vega-post" relay start']);
  await h.mod.startRemotes(h.cx);assert.equal(starts().length,1,'not every refresh');
  now+=300001;await h.mod.startRemotes(h.cx);assert.equal(starts().length,2,'slow period');
  h.mod.machines=[{name:'ganymede',state:'disconnected'}];await h.mod.startRemotes(h.cx);
  h.mod.machines=[{name:'ganymede',state:'connected'}];await h.mod.startRemotes(h.cx);
  assert.equal(starts().length,3,'a machine turning connected starts at once');
  fail=true;now+=300001;
  for(let i=0;i<3;i++){await h.mod.startRemotes(h.cx);assert.equal(h.mod.startError,i<2?null:'hc ganymede vegad start failed');now+=60001;}
  assert(texts(h.render()).includes('vega: hc ganymede vegad start failed'));
  fail=false;await h.mod.startRemotes(h.cx);assert.equal(h.mod.startError,null);
});
