import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

function load(name) {
  const state = new Map(), calls = [], tasks = [], commands = [];
  let props = {}, rendering = false;
  function el() {
    const obj={items:[],handlers:{}};let proxy;
    proxy=new Proxy(obj,{get(o,k){
      if(k in o)return o[k];
      if(k==='child')return c=>{o.items.push(c);return proxy;};
      if(k==='children')return cs=>{o.items.push(...cs);return proxy;};
      if(k==='when')return (yes,f)=>yes?f(proxy):proxy;
      if(k==='on_click'||k==='on_mouse_down')return (...a)=>{o.handlers[k]=a.at(-1);return proxy;};
      return ()=>proxy;
    }});return proxy;
  }
  const cx={notify(){},stop_propagation(){calls.push(['stop']);},timer:{every(){}},spawn(f){const p=f(cx);tasks.push(p);return p;}};
  const api={View:class {},div:el,h_flex:el,v_flex:el,svg:el,image:el,
    InputState:{new(){assert.equal(rendering,false,'input state created in render');let value='';return {on(){},value:()=>value,set_value:v=>{value=v;},focus(){throw Error('focus stolen');}};}},
    Input:{new:el},theme:()=>({}),shared:k=>state.get(k),setShared:(k,v)=>state.set(k,v),
    props:()=>props,home:()=>'/scratch',terminals:()=>[{address:'t7',state:'live'}],
    run:(...a)=>calls.push(['run',...a]),openOverlay:s=>{calls.push(['open',s]);return 'window';},
    closeOverlay:s=>calls.push(['close',s]),notify:s=>calls.push(['toast',s]),
    nextCommand:()=>new Promise(r=>commands.push(r)),watchFolder:()=>new Promise(()=>{}),
    exec:async (...a)=>{calls.push(['exec',...a]);return {code:0,stdout:'{}'};},
    localStorage:{getItem:()=>null,removeItem(){},setItem(){}},Date,Map,Set,JSON};
  let code=fs.readFileSync(new URL(`../config/herdr-app/mods/${name}/main.js`,import.meta.url),'utf8');
  code=code.replace(/^import .*;\n/gm,'').replace('export default class','globalThis.Mod = class');
  const context=vm.createContext(api);vm.runInContext(code,context);
  const mod=new context.Mod();
  return {mod,state,calls,cx,api:context,setProps:p=>{props=p;},render(){rendering=true;try{return mod.render(cx);}finally{rendering=false;}},async flush(){await Promise.all(tasks.filter(p=>p));}};
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
  const h=load('vega-agents');
  h.mod.vegaTerminals=new Map([['t7',{labels:{'vega.mail':'2','vega.decisions':'1','vega.waiting':'1','vega.kind':'C','vega.uac':'UAC','vega.git':'dirty'}}]]);
  const icons=h.mod.vegaIcons('t7',{}).items;
  assert.equal(icons.length,6);
  for(const icon of icons)icon.handlers.on_click({},h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='stop').length,6);
  assert(h.calls.some(c=>c[0]==='open'&&c[1].mod==='vega-inbox'));
  assert(h.calls.some(c=>c[0]==='open'&&c[1].props?.mail==='t7'));
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='agent.focus'));
});
