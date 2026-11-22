import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

function load(name, {allowFocus=false,crlf=false}={}) {
  const state = new Map(), storage = new Map(), calls = [], tasks = [], commands = [];
  const files = new Map(), fsCalls = [];
  const fakeFs = {
    mkdir: async p => { fsCalls.push(['mkdir', p]); },
    readFile: async (p, enc) => { if (!files.has(p)) throw new Error('ENOENT'); return files.get(p); },
    writeFile: async (p, text) => { fsCalls.push(['write', p]); files.set(p, text); },
  };
  let props = {}, rendering = false, background = false;
  function el() {
    const obj={items:[],handlers:{}};let proxy;
    proxy=new Proxy(obj,{get(o,k){
      if(k in o)return o[k];
      if(k==='child')return c=>{o.items.push(c);return proxy;};
      if(k==='children')return cs=>{o.items.push(...cs);return proxy;};
      if(k==='when')return (yes,f)=>yes?f(proxy):proxy;
      if(k==='id'||k==='tooltip'||k==='text_color'||k==='bg'||k==='border_color'||k==='spin')return v=>{o[k+'Value']=v;return proxy;};
      if(k==='on_click'||k==='on_mouse_down'||k==='on_mouse_move'||k==='on_mouse_up'||k==='on_key_down'||k==='on_scroll_wheel')return (...a)=>{o.handlers[k]=a.at(-1);if(k==='on_mouse_down'){o.mouseButton=a[0];o.handlers[`${k}:${a[0]}`]=a.at(-1);}return proxy;};
      return ()=>proxy;
    }});return proxy;
  }
  const cx={notify(){},open_url:url=>calls.push(['open-url',url]),write_to_clipboard:text=>calls.push(['clipboard',text]),stop_propagation(){calls.push(['stop']);},timer:{every(){}},sleep(){background=true;return new Promise(()=>{});},spawn(f){background=false;const p=f(cx);if(!background)tasks.push(p);return p;}};
  const api={View:class {},div:el,h_flex:el,v_flex:el,svg:source=>Object.assign(el(),{svgSource:source}),image:el,
    InputState:{new(options={}){assert.equal(rendering,false,'input state created in render');let value=options.value??'';return {handlers:{},on(event,f){this.handlers[event]=f;},value:()=>value,set_value:v=>{value=v;},focus(){assert(allowFocus,'focus stolen');calls.push(['focus']);}};}},
    Input:{new:el},ScrollHandle:{new:()=>({scroll_to_item:i=>calls.push(['scroll',i])})},
    Scrollbar:{vertical:el},v_virtual_list:(id,count,height,key,render)=>el().id(id).children(render({start:0,end:count})),
    settings:()=>({zoom:1}),theme:()=>({}),shared:k=>state.get(k),setShared:(k,v)=>state.set(k,v),
    props:()=>props,home:()=>'/scratch',terminals:()=>[{address:'t7',state:'live'}],
    agents:()=>[],panes:()=>[],spaces:()=>[],focused:()=>null,layout:()=>null,window:()=>null,
    setAgentOrder:a=>calls.push(['order',a]),
    run:(...a)=>calls.push(['run',...a]),openOverlay:s=>{calls.push(['open',s]);return 'window';},
    closeOverlay:s=>calls.push(['close',s]),notify:s=>calls.push(['toast',s]),
    nextCommand:()=>{background=true;return new Promise(r=>commands.push(r));},watchFolder:()=>new Promise(()=>{}),
    exec:async (...a)=>{calls.push(['exec',...a]);return {code:0,stdout:'{}'};},
    checking:()=>false, appHome:()=>'/scratch/app-home', fs: fakeFs,
    localStorage:{getItem:k=>storage.get(k)??null,removeItem:k=>storage.delete(k),setItem:(k,v)=>storage.set(k,v)},Date,Map,Set,JSON};
  let code=fs.readFileSync(new URL(`../config/herdr-app/mods/${name}/main.js`,import.meta.url),'utf8');
  if (crlf) code=code.replace(/\r?\n/g,'\r\n');
  code=code.replace(/^import .*;\r?\n/gm,'').replace('export default class','globalThis.Mod = class');
  const context=vm.createContext(api);
  if(['vega-window-tabs','vega-services'].includes(name)) for(const file of name==='vega-services'?['model.js']:['navigation.js','tabs.js','diff.js']) {
    const helper=fs.readFileSync(new URL(`../config/herdr-app/mods/${name}/${file}`,import.meta.url),'utf8')
      .replace(/^import .*;\r?\n/gm,'').replaceAll('export function','function').replaceAll('export const','const');
    vm.runInContext(helper,context);
  }
  vm.runInContext(code,context);
  const mod=new context.Mod();
  return {mod,state,storage,calls,cx,api:context,commands,files,fsCalls,newCopy:()=>new context.Mod(),setProps:p=>{props=p;},render(){rendering=true;try{return mod.render(cx);}finally{rendering=false;}},async flush(){await Promise.all(tasks.filter(p=>p));}};
}
const row=()=>({schema:2,project:'project-a',checkout:'/repo with spaces',at:Date.now()/1000,decisions:1,
  decision_texts:[{id:'b-1',title:'Choose',question:'What next?',project:'project-a'}],waiting_panes:[]});

test('sidebar preference timer redraws only for changes, errors, and recovery', async () => {
  const h = load('vega-sidebar'), timers = new Map();
  h.cx.timer.every = (ms, tick) => timers.set(ms, tick);
  let redraws = 0;
  h.cx.notify = () => redraws++;
  let response = {code: 0, stdout: '{"projects":{},"views":{}}'};
  h.api.exec = async () => response;
  h.mod.init({}, h.cx);
  await h.flush();
  redraws = 0;
  const poll = () => timers.get(5000)(h.cx);
  for (let i = 0; i < 3; i++) await poll();
  assert.equal(redraws, 0);
  response = {code: 0, stdout: '{"projects":{"alpha":{"pinned":true}},"views":{}}'};
  await poll();
  const lastGood = h.mod.preferences;
  assert.equal(redraws, 1);
  response = {code: 1, stderr: 'offline'};
  await poll();
  await poll();
  assert.equal(redraws, 2);
  assert.equal(h.mod.preferences, lastGood);
  response = {code: 0, stdout: JSON.stringify(lastGood)};
  await poll();
  await poll();
  assert.equal(redraws, 3);
  assert.equal(h.mod.configError, null);
});

test('sidebar age timer redraws once when a label changes or a badge expires', () => {
  const h = load('vega-sidebar'), timers = new Map();
  h.cx.timer.every = (ms, tick) => timers.set(ms, tick);
  let redraws = 0, now = 1_800_000_000_000;
  h.cx.notify = () => redraws++;
  h.api.Date = class extends Date { static now() { return now; } };
  h.api.agents = () => [{address: 't7', status: 'idle'}];
  h.mod.init({}, h.cx);
  h.state.set('vega.badges', [{...row(), at: now / 1000 - 45}]);
  h.render();
  assert(texts(h.render()).includes('<1m'));
  const tick = () => timers.get(30000)(h.cx);
  tick();
  assert.equal(redraws, 0);
  now += 30_000;
  tick();
  tick();
  assert.equal(redraws, 1, 'an expired badge invalidates once');
  now += 30_000;
  tick();
  tick();
  assert.equal(redraws, 2, 'the age boundary invalidates once');
  assert(texts(h.render()).includes('1m'));
});

test('status polls skip identical render state and keychain heartbeat timestamps', async () => {
  const h = load('vega');
  let redraws = 0;
  h.cx.notify = () => redraws++;
  h.api.exec = async () => ({code: 0, stdout: '{"running":true,"rows":[]}'});
  // Direct calls exercise the same poll body without launching the daemon or watcher task.
  h.mod.watching = true;
  for (let i = 0; i < 3; i++) await h.mod.refresh(h.cx);
  assert.equal(redraws, 0);
  const now = Date.now() / 1000;
  const keychain = {host: 'puck', state: 'unlocked', since: now - 10, at: now,
    agent: {state: 'ready', keys: 2, at: now}};
  h.mod.applyKeychains([keychain], now, h.cx);
  const published = h.state.get('vega.keychains');
  h.mod.applyKeychains([{...keychain, at: now + 1, agent: {...keychain.agent, at: now + 1}}], now + 1, h.cx);
  assert.equal(redraws, 1);
  assert.equal(h.state.get('vega.keychains'), published);
  h.mod.applyKeychains([keychain], now - 181, h.cx);
  assert.equal(redraws, 2);
  assert.equal(h.mod.keychains[0].state, 'unknown');
});

test('decision inbox timer redraws when content changes and closes on expiration', () => {
  const h = load('vega-inbox');
  let redraws = 0, tick;
  h.cx.notify = () => redraws++;
  h.cx.timer.every = (_ms, callback) => { tick = () => callback(h.cx); };
  const badge = row();
  h.state.set('vega.badges', [badge]);
  h.setProps({overlay: 'inbox'});
  h.mod.init({}, h.cx);
  badge.at += 1;
  tick();
  assert.equal(redraws, 0);
  badge.decision_texts[0].title = 'New title';
  tick();
  tick();
  assert.equal(redraws, 1);
  badge.at -= 120;
  tick();
  assert(h.calls.some(([call, id]) => call === 'close' && id === 'inbox'));
});

test('unspaced remote core agents join project rows without a native agent entry',()=>{
  const h=load('vega-sidebar'),coord='github.com/durandom/rally-ng-app',member='github.com/durandom/rally-ng-maps';
  const uac={address:'ganymede/t1j',state:'live',status:'idle',labels:{'vega.role':'uac','vega.project-id':coord,'herdr.space':'s1','vega.group-members':JSON.stringify([member])}};
  const child={address:'io/t36',state:'live',status:'done',parent:uac.address,name:'guide',labels:{'vega.role':'sentinel','vega.harness':'claude','vega.mode':'headed','vega.project-id':member}};
  h.api.terminals=()=>[uac,child];
  h.api.agents=()=>[{...uac,pane:uac.address,space:'ganymede/s1'}];
  h.mod.init({},h.cx);
  let nodes=elements(h.render());
  assert(nodes.some(n=>n.idValue==='ag-io/t36'));
  assert(nodes.some(n=>n.idValue===`project-block-${member}`));
  assert(!nodes.some(n=>n.idValue==='term-io/t36'));
  const cached=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,cached);
  // Core status participates in the existing cache and Active filtering.
  h.state.set('vega.hide-inactive',true);
  child.status='idle';nodes=elements(h.render());
  assert(!nodes.some(n=>n.idValue==='ag-io/t36'));
  child.status='working';h.render();assert.notEqual(h.mod.modelCache,cached);
  // Native adoption supplies the richer row, never a duplicate.
  h.api.agents=()=>[{...uac,pane:uac.address,space:'ganymede/s1'},{...child,pane:child.address}];
  nodes=elements(h.render());assert.equal(nodes.filter(n=>n.idValue==='ag-io/t36').length,1);
  h.api.agents=()=>[{...uac,pane:uac.address,space:'ganymede/s1'}];
  // Without a group claim the child remains reachable through its parent tree.
  delete uac.labels['vega.group-members'];
  nodes=elements(h.render());assert(nodes.some(n=>n.idValue==='sub-io/t36'));
  // Parent/project inheritance also works without the child's own project label.
  delete child.labels['vega.project-id'];
  nodes=elements(h.render());assert(nodes.some(n=>n.idValue==='sub-io/t36'));
  assert.equal(h.mod.modelCache.membership.projectOfAgent(child),coord);
  // If its parent disappears, its own project still supplies a top-level row.
  child.labels['vega.project-id']=member;uac.state='exited';h.api.agents=()=>[];
  nodes=elements(h.render());assert(nodes.some(n=>n.idValue==='ag-io/t36'));
  // Saved views retain authority over these synthetic rows.
  h.mod.preferences={projects:{},views:{hidden:{labels:['other'],machines:[]}}};
  h.state.set('vega.view','hidden');nodes=elements(h.render());
  assert(!nodes.some(n=>['ag-io/t36','sub-io/t36'].includes(n.idValue)));
  h.state.set('vega.view','all');child.state='exited';
  nodes=elements(h.render());assert(!nodes.some(n=>n.idValue==='ag-io/t36'));
});

test('rally group nests io sentinels under one ganymede UAC with member actions and badges',()=>{
  const h=load('vega-sidebar'),coord='github.com/durandom/rally-ng-app',member='github.com/durandom/rally-ng-maps';
  const uac={address:'ganymede/t1j',state:'live',status:'idle',labels:{'vega.role':'uac','vega.project-id':coord,'vega.project':'rally-ng-app','vega.git':'dirty','vega.group-members':JSON.stringify([coord,member])}};
  const child={address:'io/t2',state:'live',status:'working',parent:uac.address,labels:{'vega.role':'sentinel','vega.project-id':member}};
  h.api.terminals=()=>[uac,child];
  h.api.agents=()=>[uac,child].map(a=>({...a,pane:a.address,program:{key:'codex'}}));
  h.mod.init({},h.cx);
  h.state.set('vega.badges',[
    {...row(),project:coord,machine:'ganymede',decisions:9},
    {...row(),project:member,machine:'ganymede',decisions:2},
    {...row(),project:member,machine:'io',decisions:7},
  ]);
  const nodes=elements(h.render()),header=nodes.find(n=>n.idValue===`uac-${member}`);
  assert(header);assert(texts(header).includes('rally-ng-maps'));
  assert(!texts(header).includes('no UAC'));assert(!texts(header).includes('dirty'));
  const block=nodes.find(n=>n.idValue===`project-block-${coord}`);
  assert(elements(block).includes(header),'member header is inside the coordination block');
  assert.equal(nodes.filter(n=>n.idValue==='ag-ganymede/t1j').length,1);
  assert(!nodes.some(n=>typeof n.idValue==='string'&&n.idValue.startsWith('group-uac-')));
  const context=nodes.find(n=>n.idValue==='ag-ganymede/t1j');
  assert(context);assert(nodes.some(n=>n.idValue==='ag-io/t2'));
  const badge=nodes.find(n=>n.idValue===`total-uac-${member}`);
  assert(texts(badge).includes('2'));
  badge.handlers.on_click({},h.cx);
  let overlay=h.calls.filter(c=>c[0]==='open').at(-1)[1];
  assert.equal(overlay.props.attentionProject,member);assert.equal(overlay.props.uac,uac.address);
  header.handlers['on_mouse_down:right']({},h.cx);
  overlay=h.calls.filter(c=>c[0]==='open').at(-1)[1];
  assert.equal(overlay.props.projectConfig,member);assert.equal(overlay.props.uac,uac.address);
  context.handlers.on_click();assert(h.calls.some(c=>c[0]==='run'&&c[1]==='pane.focus'&&c[2].pane===uac.address));
  context.handlers['on_mouse_down:right']({},h.cx);
  overlay=h.calls.filter(c=>c[0]==='open').at(-1)[1];
  h.setProps({...overlay.props,overlay:'menu'});
  elements(h.render()).find(n=>n.idValue==='agent-project').handlers.on_click({},h.cx);
  overlay=h.calls.filter(c=>c[0]==='open').at(-1)[1];
  assert.equal(overlay.props.projectConfig,coord);assert.equal(overlay.props.uac,uac.address);
  h.setProps(overlay.props);
  assert(texts(h.mod.uacTab(h.mod.colors())).includes(uac.address));
  h.setProps({});h.state.set('vega.hide-inactive',true);
  assert(elements(h.render()).some(n=>n.idValue==='ag-ganymede/t1j'),'active member keeps its one idle group UAC context');
  h.mod.preferences={projects:{},views:{Remote:{machines:['ganymede'],labels:[]}}};
  h.state.set('vega.view','Remote');
  assert(elements(h.render()).some(n=>n.idValue==='ag-ganymede/t1j'),'machine view uses the group UAC host');
  h.mod.preferences.views.Remote.machines=['io'];
  assert(elements(h.render()).some(n=>n.idValue==='ag-ganymede/t1j'),'member host matches the entire group');
  h.api.focused=()=>({terminal:uac.address});h.api.theme=()=>({selected:'#selected'});
  assert.equal(elements(h.render()).filter(n=>n.bgValue==='#selected').length,1,'only one highlighted UAC row');
  h.api.focused=()=>({terminal:child.address});
  const selected=elements(h.render()).filter(n=>n.bgValue==='#selected');
  assert.equal(selected.length,1);assert.equal(selected[0].idValue,'ag-io/t2');
  h.api.agents=()=>[child];
  assert.equal(elements(h.render()).filter(n=>n.idValue==='ag-ganymede/t1j').length,1,'missing UAC agent report still supplies one context row');
  h.mod.folded.add(`uac-${member}`);
  assert(!elements(h.render()).some(n=>n.idValue==='ag-io/t2'));
  assert(elements(h.render()).some(n=>n.idValue==='ag-ganymede/t1j'));
  h.mod.folded.clear();
  h.state.set('vega.view','all');uac.labels['vega.group-members']='';
  assert.equal(h.mod.projectGroups([uac,child]).uacs.has(member),false);
});

test('puck 27-member group hides empty members and keeps folds, order, views and pins at block scope',()=>{
  const h=load('vega-sidebar'),coord='github.com/team/epic-creator';
  const members=Array.from({length:27},(_,i)=>`github.com/team/member-${i}`);
  const uac={address:'puck/t19',state:'live',status:'idle',labels:{'vega.role':'uac','vega.project-id':coord,'vega.harness':'pi','vega.group-members':JSON.stringify(members)}};
  const agent=(address,project,status='working')=>({address,state:'live',status,labels:{'vega.project-id':project},program:{key:'codex'}});
  const first=agent('io/t2',members[0]),second=agent('puck/t3',members[1],'idle');
  const own={...agent('t4',members[2]),labels:{'vega.project-id':members[2],'vega.role':'uac'}};
  const shell={...agent('puck/t5',members[3]),foreground:{name:'zsh'}};
  const dead={...agent('puck/t6',members[4]),state:'exited'};
  const all=[uac,first,second,own,shell,dead];
  h.api.terminals=()=>all;h.api.agents=()=>[second,own,first,uac];h.mod.init({},h.cx);
  h.api.theme=()=>({selected:'#selected'});h.api.focused=()=>({terminal:uac.address});
  const nodes=()=>elements(h.render()),ids=()=>nodes().map(n=>n.idValue);
  let tree=h.render(),block=elements(tree).find(n=>n.idValue===`project-block-${coord}`);
  assert.equal(elements(block).filter(n=>typeof n.idValue==='string'&&n.idValue.startsWith('uac-')).length,3,'coord plus two members with agents');
  assert(elements(block).some(n=>n.idValue==='ag-io/t2'));
  assert(!elements(block).some(n=>n.idValue===`uac-${members[2]}`),'own UAC stays outside');
  for(const member of members.slice(3))assert(!ids().includes(`uac-${member}`),'no agents: '+member);
  assert.equal(nodes().filter(n=>n.bgValue==='#selected').length,1);
  assert.equal(ids().filter(id=>id==='ag-puck/t19').length,1);
  assert.deepEqual(Array.from(h.calls.filter(c=>c[0]==='order').at(-1)[1]),[uac.address,first.address,second.address,own.address],'navigation follows block/member render order');
  h.mod.move([coord,members[2]],members[2],coord);
  assert.deepEqual(JSON.parse(h.storage.get('project-order')),[members[2],coord],'drag persists block IDs only');
  h.mod.folded.add(`uac-${coord}`);assert(!ids().includes('ag-io/t2'));assert(!ids().includes('ag-puck/t19'));
  assert(ids().includes('ag-t4'));h.mod.folded.clear();
  h.mod.preferences={projects:{[members[26]]:{labels:['work']}},views:{Work:{labels:['work'],machines:[]},Strict:{labels:['excluded'],machines:[],respect_pins:false}}};
  h.state.set('vega.view','Work');assert(ids().includes('ag-io/t2'),'even an empty member label admits whole block');
  assert(!ids().includes('ag-t4'),'own UAC no longer shares group views');
  h.state.set('vega.view','Strict');first.labels['herdr.pinned']='true';
  assert(!ids().includes(`uac-${coord}`),'strict view excludes pinned agent');
  h.mod.preferences.views.Strict.respect_pins=true;
  assert(ids().includes('ag-puck/t19'));assert(ids().includes('ag-io/t2'));
  assert(!ids().includes('ag-puck/t3'),'individual pin does not admit siblings');
  delete first.labels['herdr.pinned'];uac.labels['herdr.pinned']='true';
  assert.equal(ids().filter(id=>id==='ag-puck/t19').length,1,'pinning UAC admits only one UAC row');
  assert(!ids().includes('ag-io/t2'),'pinning UAC does not admit member agents');
  delete uac.labels['herdr.pinned'];first.labels['herdr.pinned']='true';
  h.mod.preferences.projects[coord]={pinned:true};
  assert(ids().includes('ag-puck/t3'),'project pin admits whole group');
  h.state.set('vega.hide-inactive',true);h.api.focused=()=>null;
  assert(ids().includes('ag-puck/t19'));assert(ids().includes('ag-io/t2'));
  assert(!ids().includes(`uac-${members[1]}`),'Active hides idle unpinned members');
  h.api.layout=()=>({id:'sidebar',size:{px:76}});
  assert.equal(ids().filter(id=>id==='ag-puck/t19').length,1,'compact rail has no UAC duplicates');
  h.api.layout=()=>null;h.state.set('vega.hide-inactive',false);
  delete h.mod.preferences.projects[coord].pinned;
  h.mod.preferences.projects[members[26]].pinned=true;
  assert(ids().includes('ag-puck/t3'),'legacy member project pin admits group');
  h.state.set('vega.view','all');uac.labels['vega.group-members']='';
  block=elements(h.render()).find(n=>n.idValue===`project-block-${coord}`);
  assert(!elements(block).some(n=>n.idValue==='ag-io/t2'),'removed membership restores independent projects');
  assert(ids().includes(`uac-${members[0]}`));
  assert.equal(ids().filter(id=>id==='ag-puck/t19').length,1);
});

test('group membership ignores malformed, dead and foreign-role labels; own UAC wins',()=>{
  const h=load('vega-sidebar'),member='github.com/team/maps',coord='github.com/team/app';
  const group={address:'ganymede/t1',state:'live',labels:{'vega.role':'uac','vega.project-id':coord,'vega.group-members':JSON.stringify([member])}};
  const resolve=terms=>h.mod.projectGroups(terms).uacs.get(member)?.address;
  assert.equal(resolve([group]),group.address);
  const own={address:'t2',state:'live',labels:{'vega.role':'uac','vega.project-id':member}};
  assert.equal(resolve([group,own]),own.address);assert.equal(resolve([own,group]),own.address);
  assert.equal(resolve([{...group,state:'exited'}]),undefined);
  assert.equal(resolve([{...group,labels:{...group.labels,'vega.project-id':''}}]),undefined,'a group needs a coordination project');
  for(const role of ['sentinel','uber'])assert.equal(resolve([{...group,labels:{...group.labels,'vega.role':role}}]),undefined);
  assert.equal(resolve([{...group,labels:{...group.labels,'vega.window':'status'}}]),undefined);
  for(const value of ['{broken','null','{}','"maps"','[null,42,{},""]']) {
    assert.equal(resolve([{...group,labels:{...group.labels,'vega.group-members':value}}]),undefined);
  }
  assert.equal(resolve([{...group,address:'t1'}]),'t1');
  assert(h.mod.projectGroups([group]).projects.has(member));
});

test('stock tabs and pane forks render static themed working dots and keep actions',()=>{
  const nodes=root=>[root,...(root?.items??[]).filter(x=>x&&typeof x==='object').flatMap(nodes)];
  for(const name of ['vega-tabs','vega-pane']) {
    const h=load(name);
    h.api.theme=()=>({working:'#ffaa00',blocked:'#ff0000',done:'#00ff00'});
    h.api.panes=()=>[{pane:'p1',tab:'tb1',cwd:'/scratch',status:'working',program:{name:'Agent',agent:true}}];
    h.api.spaces=()=>[{id:'s1',active:true,tabs:[{id:'tb1',name:'Work',active:true,status:'working'}]}];
    h.setProps({pane:'p1'});
    h.mod.init();
    const tree=h.render(),all=nodes(tree);
    assert(all.some(n=>n.bgValue==='#ffaa00'),'working dot uses theme.working');
    assert(!all.some(n=>Object.hasOwn(n,'spinValue')),'no status animation');
    if(name==='vega-tabs') {
      all.find(n=>n.idValue==='close-tb1').handlers.on_click({},h.cx);
      assert(h.calls.some(c=>c[0]==='run'&&c[1]==='tab.close'&&c[2].id==='tb1'));
    } else {
      tree.handlers.on_click();
      assert(h.calls.some(c=>c[0]==='run'&&c[1]==='pane.focus'&&c[2].pane==='p1'));
      h.mod.hovered=true;
      nodes(h.render()).find(n=>n.idValue==='ph-r-p1').handlers.on_click({},h.cx);
      assert(h.calls.some(c=>c[0]==='run'&&c[1]==='pane.split'&&c[2].pane==='p1'));
    }
    const source=fs.readFileSync(new URL(`../config/herdr-app/mods/${name}/main.js`,import.meta.url),'utf8');
    assert(!/\.(spin|frames|loop)\(/.test(source));
  }
});

test('tabs follow the leading agent live title and preserve custom names and generic fallbacks',()=>{
  const h=load('vega-tabs');
  const program={name:'Claude Code',key:'claude',agent:true};
  let tab={id:'tb1',name:'1',named:false,active:true};
  let lead={tab:'tb1',terminal:'t1',title:'✳ Fix login',program};
  h.api.spaces=()=>[{id:'s1',active:true,tabs:[tab]}];
  // An agent takes precedence even with a shell first in the split.
  h.api.panes=()=>[{tab:'tb1',program:{name:'zsh',agent:false}},lead];
  h.mod.init();
  const title=()=>texts(elements(h.render()).find(n=>n.idValue==='tab-tb1'))[0];
  assert.equal(title(),'Fix login');
  lead.title='[ ! ] [Review] “Unicode 修正”';
  assert.equal(title(),'[Review] “Unicode 修正”');
  tab={...tab,name:'My task',named:true};
  assert.equal(title(),'My task');
  tab.name='Claude Code';
  assert.equal(title(),'[Review] “Unicode 修正”','program-named tabs follow live titles');
  tab={...tab,name:'1',named:false};
  for(const generic of ['',null,'t1','Claude Code','CLAUDE','✳ ~/repo','/repo','C:\\repo','user@host']) {
    lead.title=generic;assert.equal(title(),'Claude Code',String(generic));
  }
  lead={...lead,program:{name:'zsh',agent:false},title:'Do not use a shell title'};
  assert.equal(title(),'zsh');
  h.api.panes=()=>[];
  assert.equal(title(),'Terminal');
  tab={...tab,name:'tb42',named:true};
  assert.equal(title(),'tb42','explicit names resembling old automatic IDs stay');
});

test('tabs recover invalid saved panels and retain the Services choice per space',()=>{
  for(const saved of ['{bad','null','[]','"text"','42','false',null]) {
    const h=load('vega-tabs');
    if(saved!==null) h.storage.set('panels',saved);
    h.mod.init();
    assert.equal(JSON.stringify(h.mod.panels),'{}');
    h.render();
  }
  const h=load('vega-tabs');
  h.storage.set('panels','{"s1":"services"}');
  h.api.spaces=()=>[{id:'s1',active:true,tabs:[]}];
  h.api.layout=()=>({children:[{id:'panel',hidden:true},{id:'panel-body',active:'files'},{id:'services'}]});
  h.state.set('workspace.panels',[]);
  h.mod.init();
  const tree=h.render();
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='layout.select'&&c[2].child==='services'));
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='layout.show'&&c[2].id==='panel'));
  assert(elements(tree).some(n=>n.idValue==='panel-services'));
  assert.equal(JSON.parse(h.storage.get('panels')).s1,'services','pending restore is not overwritten');
});

test('tabs wheel notches switch once and smooth swipes accumulate with pacing and resets',()=>{
  const h=load('vega-tabs');let now=1000;
  h.api.Date={now:()=>now};h.mod.init();
  const wheel=h.render().handlers.on_scroll_wheel;
  const commands=()=>h.calls.filter(c=>c[0]==='run').map(c=>c[1]);
  const smooth=(y,ms=0,phase)=>{now+=ms;wheel({delta:{x:0,y},touch_phase:phase});};
  wheel({delta:{y:120},delta_lines:{x:0,y:3}});
  wheel({delta:{y:-120},delta_lines:{x:0,y:-3}});
  wheel({delta:{y:120},delta_lines:{x:1,y:0}});
  assert.deepEqual(commands(),['tab.prev','tab.next']);
  h.calls.length=0;
  smooth(9);smooth(10,10);assert.deepEqual(commands(),[]);
  smooth(1,10);assert.deepEqual(commands(),['tab.prev']);
  smooth(60,50);assert.equal(commands().length,1,'rapid events are paced');
  smooth(1,50);assert.equal(commands().length,2);
  smooth(58,100);assert.equal(commands().length,2,'long swipe needs another 60px');
  smooth(1);assert.equal(commands().length,3);
  smooth(1,10);
  smooth(-20,100);assert.equal(commands().at(-1),'tab.next','turn resets the gesture');
  smooth(19,300);assert.equal(commands().length,4,'pause starts at the short-swipe threshold');
  smooth(1);assert.equal(commands().length,5);
  smooth(19,100,'started');assert.equal(commands().length,5,'explicit start resets the threshold');
  smooth(1);assert.equal(commands().length,6);
  smooth(0,300);assert.equal(commands().length,6,'horizontal-only motion does nothing');
});

function navigationFixture() {
  const h=load('vega-sidebar');
  let focus=null, liveAgents=['t1','puck/t1','t3'], liveSpaces=['s1','puck/s1','s3'];
  let livePanes=[{pane:'p1',tab:'tab1'},{pane:'p2',tab:'tab1'},
    {pane:'mod-pane',tab:'tab1'},{pane:'puck/p1',tab:'puck/tab1'}];
  const snapshots={t1:{terminal:'t1',space:'s1',tab:'tab1',pane:'p1'},
    'puck/t1':{terminal:'puck/t1',space:'puck/s1',tab:'puck/tab1',pane:'puck/p1'},
    t3:{terminal:'t3',space:'s3',tab:'tab3',pane:'p3'}};
  h.api.focused=()=>focus;
  h.api.agents=()=>liveAgents.map(address=>({address}));
  h.api.spaces=()=>liveSpaces.map(id=>({id}));
  h.api.panes=()=>livePanes;
  h.api.run=(command,args)=>{
    h.calls.push(['run',command,args]);
    if(command==='agent.focus') focus={...snapshots[args.terminal]};
    if(command==='space.select') focus={...Object.values(snapshots).find(f=>f.space===args.id)};
    if(command==='pane.focus') focus={...focus,pane:args.pane,terminal:args.pane==='mod-pane'?null:'t1'};
  };
  const sample=vm.runInContext('sampleFocus',h.api),last=vm.runInContext('lastFocus',h.api);
  return {...h,sample,last,focus(f){focus=typeof f==='string'?{...snapshots[f]}:f;sample();},
    current:()=>focus,gone(){liveAgents=liveAgents.filter(x=>x!=='puck/t1');liveSpaces=liveSpaces.filter(x=>x!=='puck/s1');livePanes=livePanes.filter(x=>x.pane!=='p2');}};
}

test('navigation MRU updates once per focus, toggles and skips gone agents/spaces',()=>{
  for(const kind of ['agent','space']) {
    const h=navigationFixture();
    h.last(`last-${kind}`);assert.equal(h.calls.length,0,'empty history is a no-op');
    h.focus('t1');h.sample();h.last(`last-${kind}`);assert.equal(h.calls.length,0,'one entry is a no-op');
    h.focus('puck/t1');h.last(`last-${kind}`);assert.equal(h.current().terminal,'t1');
    h.last(`last-${kind}`);assert.equal(h.current().terminal,'puck/t1');
    h.focus('t3');h.gone();h.last(`last-${kind}`);assert.equal(h.current().terminal,'t1');
    h.last(`last-${kind}`);assert.equal(h.current().terminal,'t3');
    h.focus({...h.current(),terminal:'shell'});
    assert(!vm.runInContext('navigation.agents',h.api).includes('shell'));
  }
});

test('last pane remembers each tab, includes mod panes, skips closed panes and toggles',()=>{
  const h=navigationFixture();h.focus('t1');h.focus({...h.current(),pane:'p2'});
  h.focus('puck/t1');h.last('last-pane');assert.equal(h.calls.length,0,'no cross-tab jump');
  h.focus('t1');h.last('last-pane');assert.equal(h.current().pane,'p2');
  h.last('last-pane');assert.equal(h.current().pane,'p1');
  h.focus({...h.current(),pane:'mod-pane',terminal:null});h.gone();
  h.last('last-pane');assert.equal(h.current().pane,'p1');
  h.last('last-pane');assert.equal(h.current().pane,'mod-pane');
});

test('sidebar copies share history and handle each broadcast key only once',async()=>{
  const h=navigationFixture(),other=h.newCopy();
  h.mod.startNavigation(h.cx);other.startNavigation(h.cx);
  h.focus('t1');h.focus('puck/t1');
  for(const resolve of h.commands.splice(0))resolve('last-agent');
  await new Promise(r=>setImmediate(r));
  assert.equal(h.calls.length,1);assert.equal(h.current().terminal,'t1');
  for(const resolve of h.commands.splice(0))resolve('last-agent');
  await new Promise(r=>setImmediate(r));
  assert.equal(h.calls.length,2);assert.equal(h.current().terminal,'puck/t1');
  // If the first copy is hidden/removed, only the other receives the key.
  const listeners=h.commands.splice(0);listeners[1]('last-agent');
  await new Promise(r=>setImmediate(r));
  assert.equal(h.calls.length,3);assert.equal(h.current().terminal,'t1');
});

test('last navigation keys use mod command argument syntax',()=>{
  const keys=JSON.parse(fs.readFileSync(new URL('../config/herdr-app/keys.json',import.meta.url),'utf8')).global;
  for(const [key,command] of [['ctrl-s l','last-agent'],['cmd-;','last-agent'],['ctrl-s shift-l','last-space'],['ctrl-s ;','last-pane']])
    assert.equal(keys[key],`vega-sidebar.${command}`);
});

test('background sampling catches focus without renders and shares a cheap polling budget',async()=>{
  const h=navigationFixture();let now=1000,terminal='t1',reads=0,focusReads=0;
  const sleeps=[];
  h.api.Date={now:()=>now};
  h.api.focused=()=>{focusReads++;return {terminal,space:'s1',tab:'tab1',pane:terminal};};
  h.api.agents=()=>{reads++;return [{address:'t1'},{address:'puck/t1'}];};
  const cx={...h.cx,sleep(ms){assert.equal(ms,300);return new Promise(r=>sleeps.push(r));}};
  cx.spawn=f=>f(cx);
  h.mod.startNavigation(cx);h.newCopy().startNavigation(cx);
  assert.equal(focusReads,1,'copies share one poll');assert.equal(reads,1);
  now+=300;
  for(const resume of sleeps.splice(0))resume();
  await new Promise(r=>setImmediate(r));
  assert.equal(focusReads,2);assert.equal(reads,1,'unchanged focus does not copy inventory');
  now+=300;terminal='puck/t1';
  for(const resume of sleeps.splice(0))resume();
  await new Promise(r=>setImmediate(r));
  assert.equal(reads,2);
  assert.deepEqual(Array.from(vm.runInContext('navigation.agents',h.api)),['puck/t1','t1']);
});

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

test('waiting child owns the hand and header badge opens a popover without focusing or folding',()=>{
  const h=load('vega-sidebar');
  const project='project-a';
  const uac={address:'t1',state:'live',space:'s1',labels:{'vega.role':'uac','vega.project-id':project,'vega.waiting':'✋1'}};
  const child={address:'ganymede/t2',state:'live',space:'s1',parent:'t1',status:'idle',turn:3,
    labels:{'vega.project-id':project,'herdr.seen':'3'}};
  h.api.terminals=()=>[uac,child];
  h.api.agents=()=>[{address:'t1',space:'s1',status:'idle'}];
  h.state.set('vega.badges',[{...row(),notes:0,decisions:0,decision_texts:[],waiting:1,
    waiting_panes:[{pane:'hc:ganymede/t2',name:'Worker',attention:'asking',title:'Need a choice'}]}]);
  h.state.set('vega.hide-inactive',true);
  h.mod.init({},h.cx);
  const tree=h.render(),nodes=elements(tree);
  assert(nodes.some(n=>n.idValue==='vega-ganymede/t2-waiting'),'waiting idle child survives Active');
  assert(!nodes.some(n=>n.idValue==='vega-t1-waiting'),'no aggregate hand on UAC');
  const badge=nodes.find(n=>n.idValue==='total-uac-project-a');
  assert.deepEqual(texts(badge),['1']);
  h.calls.length=0;badge.handlers.on_mouse_down({},h.cx);badge.handlers.on_click({},h.cx);
  const popup=h.calls.find(c=>c[0]==='open')[1];
  assert.equal(popup.kind,'popup');assert.equal(popup.props.attentionProject,project);assert.equal(popup.props.uac,'t1');
  assert(!h.calls.some(c=>c[0]==='run'),'header opens detail without focusing a pane');
  assert.equal(h.mod.folded.size,0);
});

test('attention popover routes remote Open and Done, decisions and project-scoped notes through fakes',async()=>{
  const h=load('vega-sidebar');const now=Date.now();
  const r={...row(),machine:'ganymede',notes:1,waiting:2,checkout:'/remote repo',
    decision_texts:[{id:'d1',title:'Choose model',created:Math.floor(now/1000)-60}],
    waiting_panes:[{pane:'hc:io/t2',name:'Worker',attention:'asking',title:'Question?'},
      {pane:'hc:ganymede/t3',name:'Dialog',attention:'input',title:'Permission'}]};
  const notes=[{host:'proteus',id:now-1,project:'project-a',unread:true,title:'A note'},
    {host:'proteus',id:now,project:'other',unread:true,title:'Unrelated'}];
  h.api.terminals=()=>[{address:'ganymede/t1',state:'live',labels:{'vega.role':'uac','vega.project-id':'project-a'}}];
  h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,[...args]]);
    return bin==='python3'?{code:0,stdout:JSON.stringify({code:0,stdout:'dismissed',stderr:''})}
      :{code:0,stdout:args.includes('clear')?'':notes.map(n=>JSON.stringify(n)).join('\n')};};
  h.state.set('vega.badges',[r]);h.setProps({attentionProject:'project-a',uac:'ganymede/t1',overlay:'popup'});
  h.mod.init({},h.cx);await h.flush();
  const click=id=>elements(h.render()).find(n=>n.idValue===id).handlers.on_click({},h.cx);
  assert(texts(h.render()).some(s=>s.includes('Worker · waiting')));
  assert(texts(h.render()).some(s=>s.includes('d1 · decision · 1m ago')));
  assert(!texts(h.render()).includes('Unrelated'));
  assert(!elements(h.render()).some(n=>n.idValue==='attention-wait-hc:ganymede/t3-done'),'live dialog has no dismiss');
  click('attention-wait-hc:io/t2-open');
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='pane.focus'&&c[2].terminal==='io/t2'));
  click('attention-wait-hc:io/t2-done');await h.flush();
  const dismiss=h.calls.find(c=>c[0]==='exec'&&c[2].includes('dismiss'));
  assert.deepEqual(dismiss[2].slice(2),['ganymede','/remote repo','vega','mux','dismiss','hc:io/t2']);
  assert(!elements(h.render()).some(n=>n.idValue==='attention-wait-hc:io/t2'));
  click('attention-decision-d1-done');
  assert(h.calls.some(c=>c[0]==='open'&&c[1].props?.tab==='decisions'));
  click(`attention-note-proteus-${now-1}-done`);await h.flush();
  assert(h.calls.some(c=>c[0]==='exec'&&JSON.stringify(c[2])===JSON.stringify(['post','notes','clear','--upto',`proteus=${now-1}`])));
  click('attention-notes');assert(h.calls.some(c=>c[0]==='open'&&c[1].props?.tab==='notes'));
});

test('unchanged waiting reconciliation never publishes or redraws, including empty and expired state',()=>{
  const h=load('vega-sidebar');let writes=0, redraws=0;
  h.api.setShared=(key,value)=>{writes++;h.state.set(key,value);};
  h.cx.notify=()=>redraws++;
  const reconcile=()=>h.mod.reconcileWaiting(h.cx);
  reconcile();reconcile();assert.equal(writes,0);assert.equal(redraws,0);
  const r={...row(),waiting_panes:[{pane:'hc:t7',attention:'input'}]};
  h.state.set('vega.badges',[r]);reconcile();
  assert.equal(writes,1);assert.equal(redraws,1);
  const previous=h.state.get('vega.waiting-state');
  reconcile();h.newCopy().reconcileWaiting(h.cx);
  assert.equal(writes,1);assert.equal(redraws,1);
  r.at-=61;reconcile();
  assert.equal(writes,2);assert.equal(redraws,2);
  assert.equal(Object.keys(previous).length,1,'published episodes are not mutated during reconciliation');
  reconcile();assert.equal(writes,2);assert.equal(redraws,2);
});

test('attention clock only redraws for displayed age changes and stale badge expiry',async()=>{
  const h=load('vega-sidebar');let now=1000000, redraws=0;const timers=new Map();
  h.api.Date={now:()=>now};h.cx.notify=()=>redraws++;
  h.cx.timer.every=(ms,f)=>timers.set(ms,f);
  h.setProps({attentionProject:'project-a',uac:'t1'});
  h.state.set('vega.badges',[{...row(),at:now/1000,decision_texts:[{id:'d1',created:now/1000-59}]}]);
  h.mod.init({},h.cx);await h.flush();h.render();redraws=0;
  const tick=()=>timers.get(1000)(h.cx);
  tick();assert.equal(redraws,0);
  now+=1000;tick();assert.equal(redraws,1,'minute boundary changes age');
  h.render();tick();assert.equal(redraws,1);
  now+=60000;tick();assert.equal(redraws,2,'expired badges disappear');
  h.render();now+=1000;tick();assert.equal(redraws,2,'empty popover stays quiet');
});

test('waiting auto-clears only after a viewed turn advances, once across copies; a new episode can wait again',async()=>{
  for(const remote of [false,true]) {
    const h=load('vega-sidebar');const address=remote?'ganymede/t2':'t2';
    const term={address,state:'live',turn:4,labels:{'herdr.seen':'3'}};
    const w={pane:`hc:${address}`,name:'Worker',attention:'silent',title:''};
    const r={...row(),machine:remote?'ganymede':'',waiting:1,waiting_panes:[w]};
    let focus=null;
    h.api.terminals=()=>[term];h.api.focused=()=>focus;
    h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,[...args]]);return {code:0,stdout:bin==='python3'?JSON.stringify({code:0,stdout:'ok',stderr:''}):'ok'};};
    h.state.set('vega.badges',[r]);h.mod.init({},h.cx);await h.flush();h.calls.length=0;
    const reconcile=()=>h.mod.reconcileWaiting(h.cx);
    reconcile();term.turn=5;reconcile();await h.flush();
    assert(!h.calls.some(c=>c[0]==='exec'),'advancing unseen turns leaves the hand');
    term.labels['herdr.seen']='5';focus={terminal:address};reconcile();await h.flush();
    assert(!h.calls.some(c=>c[0]==='exec'),'focus alone leaves the hand');
    focus=null;term.turn=6;reconcile();
    const copy=h.newCopy();copy.dismissBusy=new Set();copy.dismissResults=new Map();copy.reconcileWaiting(h.cx);
    await h.flush();
    assert.equal(h.calls.filter(c=>c[0]==='exec').length,1,'one dismiss across copies');
    assert.equal(h.mod.vegaTotals(remote?'ganymede/t1':'t1','project-a').waiting,0);
    r.waiting_panes=[];reconcile();r.waiting_panes=[w];reconcile();
    assert.equal(h.mod.vegaTotals(remote?'ganymede/t1':'t1','project-a').waiting,1,'new briefing restores waiting');
  }
});

test('waiting sees focus between polls and keeps failed dismiss visible for explicit retry',async()=>{
  const h=load('vega-sidebar');const term={address:'t2',state:'live',turn:4,labels:{'herdr.seen':'3'}};
  const w={pane:'hc:t2',attention:'asking',title:'Choice'},r={...row(),waiting:1,waiting_panes:[w]};
  h.api.terminals=()=>[term];h.state.set('vega.badges',[r]);h.mod.init({},h.cx);await h.flush();h.calls.length=0;
  let code=1;h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,[...args]]);return {code,stdout:'failed'};};
  h.mod.reconcileWaiting(h.cx);
  term.labels['herdr.seen']='4';term.turn=5;w.title='New output';w.attention='silent';h.mod.reconcileWaiting(h.cx);await h.flush();
  assert.equal(h.mod.vegaTotals('t1','project-a').waiting,1);
  h.mod.reconcileWaiting(h.cx);await h.flush();
  assert.equal(h.calls.filter(c=>c[0]==='exec').length,1,'no repeated background retries');
  code=0;await h.mod.dismissWaiting(r,w,h.cx);
  assert.equal(h.mod.vegaTotals('t1','project-a').waiting,0);
  r.at=Date.now()/1000-61;assert.equal(h.mod.vegaIcons('t2',{}).items.length,0,'stale row has no hand');
});

test('auto-dismiss ignores stale waiters, live input dialogs and seen labels from before the waiting turn',async()=>{
  const h=load('vega-sidebar');const term={address:'t2',state:'live',turn:6,labels:{}};
  const w={pane:'hc:t2',attention:'asking'},r={...row(),waiting:1,waiting_panes:[w]};
  h.api.terminals=()=>[term];h.state.set('vega.badges',[r]);h.mod.init({},h.cx);await h.flush();h.calls.length=0;
  h.mod.reconcileWaiting(h.cx);
  term.labels['herdr.seen']='4';term.turn=7;h.mod.reconcileWaiting(h.cx);await h.flush();
  assert(!h.calls.some(c=>c[0]==='exec'),'old seen label cannot handle the new waiter');
  term.labels['herdr.seen']='7';h.mod.reconcileWaiting(h.cx);
  term.turn=8;w.attention='input';h.mod.reconcileWaiting(h.cx);await h.flush();
  assert(!h.calls.some(c=>c[0]==='exec'),'live dialog cannot auto-dismiss');
  w.attention='silent';r.at=Date.now()/1000-61;h.mod.reconcileWaiting(h.cx);await h.flush();
  assert(!h.calls.some(c=>c[0]==='exec'),'stale row cannot auto-dismiss');
});

test('each agent icon stops row propagation and routes its own action',()=>{
  const h=load('vega-sidebar');
  h.mod.vegaTerminals=new Map([['t7',{labels:{'vega.mail':'2','vega.decisions':'1','vega.waiting':'1','vega.kind':'C','vega.uac':'UAC','vega.git':'dirty'}}]]);
  const icons=h.mod.vegaIcons('t7',{}).items;
  assert.equal(icons.length,2,'uac and kind sit before the name; aggregate waiting and decisions stay off the UAC row');
  for(const icon of icons)icon.handlers.on_click({},h.cx);
  assert.equal(h.calls.filter(c=>c[0]==='stop').length,2);
  h.mod.vegaTerminals.set('t8',{labels:{'vega.git':'\u2800'}});
  assert.equal(h.mod.vegaIcons('t8',{}).items.length,0,'a blank token draws nothing');
  assert(!h.calls.some(c=>c[0]==='open'&&c[1].mod==='vega-inbox'),'agent rows no longer open the inbox');
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
 if os.environ.get('TEST_MACHINE_SLOW'): __import__('time').sleep(5)
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

test('space context menu renames local and qualified remote IDs on submit; Escape cancels',()=>{
  for (const id of ['sp-local','puck/sp-remote']) {
    const h=load('vega-sidebar');
    h.api.spaces=()=>[{id,name:'uac',machine:id.includes('/')?'puck':null,tabs:[]}];
    h.mod.init({},h.cx);
    const space=elements(h.render()).find(n=>n.idValue===`space-${id}`);
    assert.equal(space.mouseButton,'right');
    space.handlers.on_mouse_down({button:'right'},h.cx);
    assert(h.calls.some(c=>c[0]==='stop'));
    assert(!h.calls.some(c=>c[0]==='run'&&c[1]==='space.select'));
    const spec=h.calls.find(c=>c[0]==='open')[1];
    assert.equal(spec.kind,'popup');assert.equal(spec.props.spaceMenu.id,id);
    for (const cancel of [true,false]) {
      const popup=load('vega-sidebar',{allowFocus:true});
      popup.setProps({...spec.props,overlay:'rename-popup'});popup.mod.init({},popup.cx);
      assert(!popup.calls.some(c=>c[0]==='focus'));
      elements(popup.render()).find(n=>n.idValue==='rename-space').handlers.on_click({},popup.cx);
      assert.equal(popup.mod.renameInput.value(),'uac');
      popup.render();popup.render();
      assert.equal(popup.calls.filter(c=>c[0]==='focus').length,1,'only explicit Rename focuses');
      popup.mod.renameInput.set_value('   ');popup.mod.renameInput.handlers.submit({},popup.cx);
      assert(!popup.calls.some(c=>c[0]==='run'),'empty names do not submit');
      popup.mod.renameInput.set_value('fullsend');
      if (cancel) {
        popup.render().handlers.on_key_down({keystroke:'escape'},popup.cx);
        assert(!popup.calls.some(c=>c[0]==='run'),'Escape never renames');
      } else {
        // InputState emits submit for Enter.
        popup.mod.renameInput.handlers.submit({},popup.cx);
        const call=popup.calls.find(c=>c[0]==='run');
        assert.equal(call[1],'space.rename');assert.equal(call[2].id,id);assert.equal(call[2].name,'fullsend');
      }
      assert(popup.calls.some(c=>c[0]==='close'&&c[1]==='rename-popup'));
    }
  }
});

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
  assert(at('uac-github.com/team/lanista')<at('ag-ganymede/t2')&&at('ag-ganymede/t2')<at('ag-t5')&&at('ag-t5')<at('uac-github.com/team/vega'),'UAC first, then the project\'s agents on other machines');
  assert(at('sub-ganymede/t3')>at('ag-ganymede/t2'),'sentinel is a branch of its UAC');
  assert(text.includes('feat/x')&&!texts(nodes.find(n=>n.idValue==='sub-ganymede/t3')).some(s=>s.includes('⚙')),'sentinel name without the cog');
  assert(text.includes('♃₃')&&text.includes('♆₈'),'machine symbol on every row');
  assert(text.includes('Spaces')&&at('space-sp3')>at('uac-github.com/team/vega'),'a space without a UAC follows under Spaces');
  assert(!ids.includes('space-sp1'),'no folder rows inside a UAC project');
  const totals=nodes.filter(n=>typeof n.idValue==='string'&&n.idValue.startsWith('total-')).map(n=>n.tooltipValue);
  assert(totals.some(t=>t.includes('✉ 2 notes')));assert(!totals.some(t=>t.includes('✉ 9')),'totals come from the UAC\'s machine');
  // Drag vega above lanista; the order is kept.
  const press=(id,ev)=>elements(h.render()).find(n=>n.idValue===id).handlers[ev]({},h.cx);
  press('uac-github.com/team/vega','on_mouse_down');press('uac-github.com/team/lanista','on_mouse_move');press('uac-github.com/team/lanista','on_mouse_up');
  ids=elements(h.render()).map(n=>n.idValue).filter(i=>typeof i==='string');
  assert(ids.indexOf('uac-github.com/team/vega')<ids.indexOf('uac-github.com/team/lanista'),'dragged project moves up');
  assert.deepEqual(JSON.parse(h.storage.get('project-order')),['github.com/team/vega','github.com/team/lanista']);
  const click=elements(h.render()).find(n=>n.idValue==='space-sp3');click.handlers.on_click({},h.cx);
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='space.select'&&c[2].id==='sp3'),'a space row selects that space');
});

test('the Uber-UAC is one row at the top, outside every project group and without a "no UAC" marker',()=>{
  const h=load('vega-sidebar');
  const agent=(address,space)=>({address,space_name:'x',space,tab:'tb',pane:address,status:'idle',program:{key:'claude'},seq:1});
  h.api.spaces=()=>[
    {id:'sp1',name:'vega',machine:null,branch:null,active:false,tabs:[]},
    {id:'sp9',name:'uber',machine:null,branch:null,active:false,tabs:[]},
  ];
  h.api.agents=()=>[agent('t7','sp1'),agent('t9','sp9')];
  const uber={'herdr.space':'sp9','vega.host':'♆₈','vega.role':'uber','vega.engaged':'yes','vega.project-id':'github.com/durandom/vega-uber-uac','vega.project':'vega-uber-uac'};
  h.api.terminals=()=>[
    {address:'t7',state:'live',labels:{'herdr.space':'sp1','vega.host':'♆₈','vega.role':'uac','vega.project-id':'github.com/team/vega','vega.project':'vega'}},
    {address:'t9',state:'live',labels:uber},
  ];
  h.mod.init({},h.cx);
  let tree=h.render(),ids=elements(tree).map(n=>n.idValue).filter(i=>typeof i==='string'),text=texts(tree);
  assert(ids.indexOf('uber-t9')>=0&&ids.indexOf('uber-t9')<ids.indexOf('uac-github.com/team/vega'),'the Uber row leads the project list');
  assert(!ids.includes('ag-t9')&&!ids.includes('uac-github.com/durandom/vega-uber-uac'),'no agent row and no project group for the Uber');
  assert(!text.includes('vega-uber-uac')&&!text.includes('no UAC'),'no project name and no marker');
  assert(text.includes('Über')&&text.includes('engaged'));
  elements(tree).find(n=>n.idValue==='uber-t9').handlers.on_click({},h.cx);
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='pane.focus'&&c[2].pane==='t9'),'a click focuses the Uber pane');
  uber['vega.engaged']='';
  assert(!texts(h.render()).includes('engaged'),'idle Uber shows no engaged marker');
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

test('unprovisioned hosts do not produce a second badge failure in the status bar',async()=>{
  const h=load('vega');h.mod.seen=new Set();h.mod.watching=true;
  h.mod.started=new Map([['io-win',{notProvisioned:true}]]);
  h.api.exec=async()=>({code:0,stdout:JSON.stringify({rows:[],errors:[{source:'io-win',message:'hc io-win badges unavailable'}]})});
  for(let i=0;i<4;i++) await h.mod.badges(h.cx);
  assert.equal(h.mod.error,null);
  h.mod.started.set('io-win',{notProvisioned:false});
  for(let i=0;i<3;i++) await h.mod.badges(h.cx);
  assert.equal(h.mod.error,'hc io-win badges unavailable','provisioned reader failures still surface');
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
    const script=vm.runInContext('collectBadges',h.api).replace('timeout=12','timeout=1.5');
    const result=f.run(script,[vm.runInContext('readBadges',h.api),JSON.stringify([{name:'ganymede',state:'connected'}])],{TEST_MACHINE_SLOW:'1'});
    assert.deepEqual(result.errors,[{source:'machines',message:'hc machine ls slow (1.5 s)'}]);
    assert.equal(result.machines[0].name,'ganymede');
    assert.equal(result.rows[0].machine,'ganymede');
  } finally {f.close();}
});

// The remote starter runs through a fake hc whose exec arguments are checked against the
// vendored hc schema; its argv really executes, so the shell snippet and $HOME are exercised.
test('remote vegad start goes through hc exec, only in real homes, and runs relay start there',()=>{
  const h=load('vega'),dir=fs.mkdtempSync(path.join(os.tmpdir(),'vega-remote-'));
  try {
    const schema=JSON.parse(fs.readFileSync(new URL('../crates/vega-herdr/schema/hc-0.1.19.openrpc.json',import.meta.url),'utf8'));
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
    fs.unlinkSync(path.join(dir,'.local/bin/vega-post'));
    assert.equal(JSON.parse(run({}).stdout).code,77,'missing binary is explicit');
    fs.writeFileSync(path.join(dir,'.local/bin/vega-post'),'#!/bin/sh\nexit 1\n',{mode:0o755});
    assert.equal(JSON.parse(run({}).stdout).code,1,'present broken binary remains a failure');
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
  assert.deepEqual(Array.from(call[2].slice(2)),['hc','exec','--json','--on','ganymede','--timeout','20','--','sh','-c',vm.runInContext('remoteStart',h.api)]);
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

test('unprovisioned remote starters stay quiet, diagnose once and recover on slow recheck',async()=>{
  for (const result of [
    {error:{kind:'spawn_failed',message:'cannot start sh: program not found'}},
    {code:77,stdout:Buffer.from('vega-not-provisioned\n').toString('base64')},
  ]) {
    const h=load('vega'); let now=1e12, current=result; const logs=[];
    h.api.Date={now:()=>now}; h.api.console={log:s=>logs.push(s)};
    // Real hc shape: launch errors go to stderr with exit 1 and an empty stdout.
    h.api.exec=async(_bin,args)=>{ const reply = args.includes('vega-post.exe') && (current.error?.message === 'cannot start sh: program not found' || (current.code === 77 && current.stdout)) ? {error:{kind:'spawn_failed',message:'cannot start vega-post.exe: program not found'}} : current; return {code:0,stdout:JSON.stringify(reply.error
      ?{code:1,stdout:'',stderr:JSON.stringify(reply)}
      :{code:0,stdout:JSON.stringify(reply),stderr:''})}; };
    h.mod.machines=[{name:'io-win',state:'connected'}];
    for(let i=0;i<4;i++) {
      await h.mod.startRemotes(h.cx);
      assert.equal(h.mod.startError,null);
      assert.equal(h.mod.started.get('io-win').next,now+300000);
      now+=300001;
    }
    assert.equal(logs.length,1);
    assert.match(logs[0],/io-win not provisioned/);
    assert(!texts(h.render()).includes('vegad start failed'));
    for (current of [{code:1},{code:77},{error:{kind:'spawn_failed',message:'cannot start sh: permission denied'}},{code:0,timed_out:true}]) {
      for(let i=0;i<3;i++){await h.mod.startRemotes(h.cx); now+=60001;}
      assert.equal(h.mod.startError,'hc io-win vegad start failed');
    }
    current={code:0};await h.mod.startRemotes(h.cx);
    assert.equal(h.mod.startError,null);
    assert.equal(h.mod.started.get('io-win').notProvisioned,false);
  }
});

test('Git sh without a Unix vega-post falls through to the native Windows worker',async()=>{
  const h=load('vega'); const calls=[];
  h.api.exec=async(_bin,args)=>{
    calls.push(Array.from(args));
    const reply=args.includes('sh') ? {code:77,stdout:Buffer.from('vega-not-provisioned\n').toString('base64')} : {code:0};
    return {code:0,stdout:JSON.stringify({code:0,stdout:JSON.stringify(reply),stderr:''})};
  };
  h.mod.machines=[{name:'io-win',state:'connected'}];
  await h.mod.startRemotes(h.cx);
  assert.equal(calls.length,2);
  assert.deepEqual(calls[1].slice(-3),['vega-post.exe','relay','start']);
  assert.equal(h.mod.startError,null);
  assert.equal(h.mod.started.get('io-win').notProvisioned,false);
});

test('native starter accepts recorded hc exec envelopes and retries a worker error',async()=>{
  // io-win hc 0.1.21: stdout/stderr are base64, signal is null; launch
  // failures are a different envelope on hc stderr with process exit 1.
  const h=load('vega'); let now=1e12,broken=false; const calls=[];
  h.api.Date={now:()=>now};
  const envelope=(code,stdout)=>({code,ms:242,signal:null,stderr:'',
    stdout:Buffer.from(stdout).toString('base64'),timed_out:false,truncated:false});
  h.api.exec=async(_bin,args)=>{
    calls.push(Array.from(args));
    const result=args.includes('sh')
      ? envelope(77,'vega-not-provisioned\n')
      : envelope(broken?1:0,broken?'error: relay_failed\n':'relay: started\r\npid: 1234\r\n');
    return {code:0,stdout:JSON.stringify({code:0,stdout:JSON.stringify(result),stderr:''})};
  };
  h.mod.machines=[{name:'io-win',state:'connected'}];
  await h.mod.startRemotes(h.cx);
  assert.deepEqual(calls[1].slice(-3),['vega-post.exe','relay','start']);
  assert.equal(h.mod.started.get('io-win').next,now+300000);
  assert.equal(h.mod.startError,null);
  broken=true;
  for(let i=0;i<3;i++){now+=300001;await h.mod.startRemotes(h.cx);}
  assert.equal(h.mod.startError,'hc io-win vegad start failed');
  assert.equal(h.mod.started.get('io-win').notProvisioned,false);
});

test('missing sh starts the Windows worker with direct argv and retries real launch failures',async()=>{
  const h=load('vega',{crlf:true}); const calls=[]; let now=1e12, broken=false;
  h.api.Date={now:()=>now};
  h.api.exec=async(_bin,args)=>{
    calls.push(Array.from(args));
    const error=args.includes('sh') ? {kind:'spawn_failed',message:'cannot start sh: program not found'} : broken ? {kind:'spawn_failed',message:'cannot start vega-post.exe: permission denied'} : null;
    return {code:0,stdout:JSON.stringify(error ? {code:1,stdout:'',stderr:JSON.stringify({error})} : {code:0,stdout:JSON.stringify({code:0}),stderr:''})};
  };
  h.mod.machines=[{name:'io-win',state:'connected'}];
  await h.mod.startRemotes(h.cx);
  assert.deepEqual(calls[1].slice(2),['hc','exec','--json','--on','io-win','--timeout','20','--','vega-post.exe','relay','start']);
  assert.equal(h.mod.started.get('io-win').notProvisioned,false);
  assert.equal(h.mod.startError,null);
  broken=true;
  for(let i=0;i<3;i++){now+=300001;await h.mod.startRemotes(h.cx);}
  assert.equal(h.mod.startError,'hc io-win vegad start failed');
});

function projectFixture(h) {
  const project='github.com/team/a';
  h.api.terminals=()=>[
    {address:'puck/t1',state:'live',labels:{'vega.role':'uac','vega.project-id':project,'vega.project':'a','herdr.space':'uac'}},
    {address:'t2',state:'live',parent:'puck/t1',labels:{'herdr.space':'sentinel'}},
    {address:'t3',state:'live',labels:{'vega.project-id':project,'herdr.space':'other'}},
    {address:'t4',state:'live',labels:{'herdr.space':'scratch'}},
  ];
  h.api.spaces=()=>['puck/uac','sentinel','other','scratch'].map(id=>({id,name:id,machine:id.startsWith('puck/')?'puck':null,tabs:[]}));
  h.api.agents=()=>[['puck/t1','puck/uac'],['t2','sentinel'],['t3','other'],['t4','scratch']].map(([address,space])=>({address,space,pane:address,tab:'tb',status:'idle',program:{}}));
  return project;
}

test('a nested sentinel with its own pane opens it; a paneless one peeks',()=>{
  const h=load('vega-sidebar');projectFixture(h);h.mod.init({},h.cx);
  const click=()=>{h.calls.length=0;elements(h.render()).find(n=>n.idValue==='sub-t2').handlers.on_click({},h.cx);};
  click();
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='pane.focus'&&c[2].pane==='t2'),'kept child focuses its pane');
  assert(!h.calls.some(c=>c[1]==='terminal.peek'));
  const agents=h.api.agents();h.api.agents=()=>agents.map(a=>a.address==='t2'?{...a,pane:null}:a);
  click();
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='terminal.peek'&&c[2].terminal==='t2'),'paneless child peeks');
});

test('sidebar shows coloured browser dots on owning agent rows and derives once per payload',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);h.mod.init({},h.cx);
  h.render();
  assert(!elements(h.render()).some(n=>typeof n.idValue==='string'&&n.idValue.includes('browsers')),'no badge without browser data');
  const model=h.mod.modelCache;
  h.state.set('vega.browsers',{at:Date.now()/1000,sessions:[
    {session:'classic-live',colour:'#d9465f',state:'live',pane:'t2',project:'spellkave',headed:true,age:60},
    {session:'classic-p2',colour:'#0e8f6f',state:'live',pane:'t2',project:'spellkave',headed:false,age:60},
    {session:'huk24',colour:'#3d73cc',state:'live',pane:'t3',project:'brainz',headed:true,age:60},
    {session:'unowned-x',colour:'#9d6bf0',state:'unowned',pane:null,project:null,headed:null,age:null},
    {session:'herdr-pane',colour:'#123456',state:'live',pane:'w1:p9',project:'x',headed:null,age:null}],
    loose:[]});
  h.render();
  assert.notEqual(h.mod.modelCache,model,'a new browser payload rebuilds the model');
  let cached=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,cached,'an unchanged payload reuses the cache');
  const nodes=elements(h.render());
  const badge=nodes.find(n=>n.idValue==='vega-t2-browsers');
  assert(badge,'the owning sentinel row carries a badge');
  const dots=badge.items.filter(n=>n.bgValue?.startsWith?.('#'));
  assert.deepEqual(dots.map(n=>n.bgValue),['#d9465f','#0e8f6f'],'one dot per session in its colour');
  assert.equal(badge.items.find(n=>n.tooltipValue).tooltipValue,'browser classic-live · live · headed');
  assert(nodes.some(n=>n.idValue==='vega-t3-browsers'),'the other owner carries its badge');
  assert(!nodes.some(n=>n.idValue==='vega-t4-browsers'),'a session-less agent has no badge');
  // More than three sessions collapse into dots plus a count; the render
  // itself never reads browsers (no exec of vega browser anywhere).
  h.state.set('vega.browsers',{at:Date.now()/1000,sessions:[1,2,3,4,5].map(i=>
    ({session:`s${i}`,colour:'#d9465f',state:'live',pane:'t2',project:'p',headed:true,age:1})),loose:[]});
  h.render();
  const capped=elements(h.render()).find(n=>n.idValue==='vega-t2-browsers');
  assert.equal(capped.items.filter(n=>n.bgValue==='#d9465f').length,3);
  assert(texts(capped).includes('+2'));
  assert(!h.calls.some(c=>c[0]==='exec'&&c[1]?.[0]==='vega'&&c[1]?.[1]?.[0]==='browser'),'the sidebar never reads browsers itself');
});

test('sidebar derivation is cached between renders and rebuilt exactly when an input changes',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);h.mod.init({},h.cx);
  h.render();
  const first=h.mod.modelCache;
  assert.ok(first,'first render derives a model');
  h.render();
  assert.equal(h.mod.modelCache,first,'unchanged inputs reuse the cached model');
  // Terminal data: a pin label rebuilds.
  const all=h.api.terminals();h.api.terminals=()=>all;
  all[1].labels['herdr.pinned']='true';
  h.render();assert.notEqual(h.mod.modelCache,first);
  let model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  const tree = elements(h.render());
  const pins = tree.filter(n=>n.svgSource==='mods/vega-sidebar/pin.svg');
  assert.ok(pins.length,'rebuilt model carries the pin marker');
  assert.ok(pins.every(pin=>!Object.hasOwn(pin,'tooltipValue')),'pin SVGs have no tooltip');
  assert.ok(tree.some(n=>n.tooltipValue==='Pinned agent' && n.items.some(child=>pins.includes(child))));
  // Agent data: a status change rebuilds.
  const agents=h.api.agents();h.api.agents=()=>agents;agents[2].status='blocked';
  h.render();assert.notEqual(h.mod.modelCache,model);
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  // Focus changes rebuild (the focused row highlight and Active retention read it).
  h.api.focused=()=>({terminal:'t3'});
  h.render();assert.notEqual(h.mod.modelCache,model);
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  // Views, badges and preferences rebuild; a stale badge row (past 60 s) rebuilds too.
  h.state.set('vega.view',`label:${'work'}`);
  h.render();assert.notEqual(h.mod.modelCache,model);
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  h.state.set('vega.badges',[{project,at:Date.now()/1000,decisions:1}]);
  h.render();assert.notEqual(h.mod.modelCache,model);
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  h.state.set('vega.badges',[{project,at:Date.now()/1000-61,decisions:1}]);
  h.render();assert.notEqual(h.mod.modelCache,model,'crossing the 60 s freshness boundary rebuilds');
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  h.mod.preferences={projects:{[project]:{pinned:true}},views:{}};
  h.render();assert.notEqual(h.mod.modelCache,model);
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  // Manual project order is a model input (drag order).
  h.mod.move([project],project,project);h.mod.order=['zzz-other'];
  h.render();assert.notEqual(h.mod.modelCache,model,'manual order is a model input');
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  // Pure build state reuses the cache: folds, theme, layout width, shared-state mirrors.
  h.mod.folded.add(`uac-${project}`);
  h.render();assert.equal(h.mod.modelCache,model);
  h.api.theme=()=>({selected:'#selected'});h.api.layout=()=>({id:'sidebar',size:{px:76}});
  h.render();assert.equal(h.mod.modelCache,model);
  h.api.layout=()=>null;h.mod.folded.clear();
  // Waiting episodes (shared state) rebuild the waiting rows.
  h.state.set('vega.waiting-state',{[JSON.stringify([project,'','t2'])]:{cleared:true}});
  h.render();assert.notEqual(h.mod.modelCache,model);
  model=h.mod.modelCache;h.render();assert.equal(h.mod.modelCache,model);
  // The localStorage fallback (restart path) is an input too: shared state unset.
  const g=load('vega-sidebar');projectFixture(g);g.mod.init({},g.cx);g.render();
  g.storage.set('active-view','all');g.render();
  const restart=g.mod.modelCache;
  g.storage.set('active-view','label:work');
  g.render();assert.notEqual(g.mod.modelCache,restart,'localStorage view fallback rebuilds');
  g.render();assert.equal(g.mod.modelCache,g.mod.modelCache);
  // The rendered tree still follows rebuilt data: unpin removes the marker.
  all[1].labels={};
  h.render();
  assert(!elements(h.render()).some(n=>n.tooltipValue==='Pinned agent'));
});

test('sidebar diagnostics: breadcrumbs detect interrupted renders, record slow/failed ones, flush a capped ring file',async()=>{
  const diagDir='/scratch/app-home/data/vega-sidebar-diagnostics',diagPath=`${diagDir}/render-log.jsonl`;
  const h=load('vega-sidebar');projectFixture(h);
  const d=h.mod.constructor.diagState();
  h.mod.init({},h.cx);
  // The module-eval load line flushes to the ring file.
  await d.flush();
  assert(h.files.get(diagPath)?.includes('"e":"load"'));
  // Happy path: renders do no file I/O and leave no breadcrumb or events behind.
  const writes=()=>h.fsCalls.filter(c=>c[0]==='write').length;
  const before=writes();
  h.render();h.render();
  assert.equal(writes(),before,'no per-render file I/O');
  assert.equal(d.current,null);assert.equal(d.pending.length,0);
  // Phase breadcrumbs: a rebuild render walks signature -> derive -> elements,
  // and the input sizes land in the in-flight breadcrumb's meta.
  const seen=[];
  const realTree=h.mod.projectTree.bind(h.mod);
  h.mod.projectTree=(...a)=>{seen.push([d.current?.phase,d.current?.meta?.terminals]);return realTree(...a);};
  h.render();
  h.mod.projectTree=realTree;
  assert.deepEqual([...seen.at(-1)],['elements',4]);
  assert.equal(d.pending.length,0);
  // A breadcrumb that never finished is detected by the next begin.
  const stale={kind:'render:main',start:Date.now()-5130,phaseAt:Date.now()-4210,phase:'derive:order',
    phases:[['begin',1],['signature',120],['derive',60],['derive:trees',4030]],
    meta:{terminals:140,agents:110,panes:96,spaces:44,badges:12,projects:40,sigLen:28410,view:'all',filtering:false}};
  d.current=stale;
  h.render();
  const interrupted=JSON.parse(d.pending.find(l=>l.includes('"e":"interrupted"')));
  assert.equal(interrupted.kind,'render:main');assert.equal(interrupted.phase,'derive:order');
  assert.equal(interrupted.by,'render:main');
  assert.ok(interrupted.phaseMs>=4210&&interrupted.phaseMs<4300);
  assert.ok(interrupted.sinceStartMs>=5130&&interrupted.sinceStartMs<5220);
  assert.deepEqual(interrupted.phases.at(-1),['derive:trees',4030]);
  assert.equal(interrupted.sizes.agents,110);assert.equal(interrupted.sizes.sigLen,28410);
  await d.flush();
  assert(h.files.get(diagPath).includes('"e":"interrupted"'),'interrupted render reaches the log');
  // A thrown render records an exception with its phase, then clears the breadcrumb.
  const realSpaces=h.api.spaces;
  h.api.spaces=()=>{throw new Error('boom');};
  assert.throws(()=>h.render());
  const failed=JSON.parse(d.pending.find(l=>l.includes('"e":"exception"')));
  assert.equal(failed.kind,'render:main');assert.equal(failed.phase,'signature');
  assert.ok(failed.error.includes('boom'));assert.equal(failed.sizes,null,'sizes are whatever the render had reached');
  assert.equal(d.current,null,'a caught render is not an interruption');
  h.api.spaces=realSpaces;h.render();
  // Slow renders carry the phase breakdown and input sizes.
  d.slowMs=0;d.pending.length=0;
  h.render();
  const slow=JSON.parse(d.pending.find(l=>l.includes('"e":"slow"')));
  d.slowMs=10;
  assert.equal(slow.kind,'render:main');
  assert.ok(slow.phases.some(([name])=>name==='signature'));
  assert.ok(slow.phases.some(([name])=>name==='elements'));
  assert.equal(slow.sizes.terminals,4);
  // Flushes are rate-limited: events inside the window wait for a later one.
  h.files.set(diagPath,'');
  d.lastWrite=Date.now();d.pending.length=0;
  d.current={...stale,start:Date.now(),phaseAt:Date.now()};
  h.render();
  await new Promise(r=>setTimeout(r,10));
  assert.equal(writes(),before+1,'rate window holds the flush');
  assert.equal(d.pending.length,1,'event stays pending');
  d.lastWrite=0;
  d.current={...stale,start:Date.now(),phaseAt:Date.now()};
  h.render();
  await new Promise(r=>setTimeout(r,10));
  assert.equal((h.files.get(diagPath).match(/"e":"interrupted"/g)||[]).length,2,'coalesced flush writes both');
  // The ring file is size-capped, keeping the newest lines.
  const filler=JSON.stringify({e:'fill',pad:'x'.repeat(120)});
  h.files.set(diagPath,Array.from({length:1200},()=>filler).join('\n')+'\n');
  d.lastWrite=0;d.pending.length=0;
  d.current={...stale,kind:'render:main',start:Date.now(),phaseAt:Date.now()};
  h.setProps({mail:'t7'});
  h.render();
  await new Promise(r=>setTimeout(r,10));
  const ring=h.files.get(diagPath);
  assert.ok(ring.length<=d.fileMax,'ring file stays capped');
  assert.ok((ring.match(/"e":"fill"/g)||[]).length<1200,'oldest lines dropped');
  assert.ok(ring.trimEnd().endsWith('"by":"render:mail"}'),'newest event kept and names the detecting copy');
  h.setProps({});
});

test('inactive filter retains UAC context and active descendants; retains done sub-agents; hides idle and unknown rows and empty groups',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  const all=h.api.terminals();
  all.push(
    {address:'t5',state:'live',parent:'puck/t1',status:'idle'},
    {address:'t6',state:'live',parent:'t5',status:'blocked'},
    {address:'t7',state:'live',parent:'puck/t1'},
    {address:'t8',state:'live',parent:'puck/t1',status:'done'},
    {address:'t9',state:'exited',parent:'puck/t1',status:'working'},
    {address:'t10',state:'live',labels:{'vega.role':'uac','vega.project-id':'idle-project'}},
    {address:'t11',state:'live',parent:'t10',status:'idle'},
  );
  h.api.terminals=()=>all;
  const agents=h.api.agents();agents[1].status='working';
  agents.push({address:'t10',status:'idle',program:{}});
  h.api.agents=()=>agents;
  h.mod.init({},h.cx);
  elements(h.render()).find(n=>n.idValue==='ag-hide-inactive').handlers.on_click({},h.cx);
  const tree=h.render(),ids=elements(tree).map(n=>n.idValue);
  for(const id of [`uac-${project}`,'ag-puck/t1','sub-t2','sub-t6','sub-t8']) assert(ids.includes(id),id);
  for(const id of ['ag-t3','ag-t4','sub-t5','sub-t7','sub-t9','uac-idle-project','ag-t10','sub-t11','space-scratch']) assert(!ids.includes(id),id);
  assert(texts(tree).includes('6 idle hidden'),'counts sessions once, including unknown, excluding retained UAC and exited child');
  assert.equal(h.state.get('vega.hide-inactive'),true);
  assert.equal(h.storage.get('hide-inactive'),'true');
  assert(h.calls.some(c=>c[0]==='stop'),'toggle stops row propagation');
  assert.deepEqual(Array.from(h.calls.filter(c=>c[0]==='order').at(-1)[1]),['puck/t1','t2']);
});

test('focused idle terminal and its UAC stay visible; activity toggle persists true and false across copies and restarts',()=>{
  const h=load('vega-sidebar');projectFixture(h);h.mod.init({},h.cx);
  h.api.focused=()=>({terminal:'t2'});
  h.state.set('vega.hide-inactive',true);
  let tree=h.render(),ids=elements(tree).map(n=>n.idValue);
  assert(ids.includes('ag-puck/t1'));assert(ids.includes('sub-t2'));assert(!ids.includes('ag-t3'));
  assert(texts(tree).includes('2 idle hidden'));
  assert.equal(h.storage.get('hide-inactive'),'true','copies shared state into this mod storage');
  h.state.delete('vega.hide-inactive');
  tree=h.render();assert(texts(tree).includes('2 idle hidden'),'restart uses localStorage fallback');
  elements(tree).find(n=>n.idValue==='ag-idle-hidden').handlers.on_click({},h.cx);
  assert.equal(h.state.get('vega.hide-inactive'),false);
  tree=h.render();assert(!elements(tree).some(n=>n.idValue==='ag-idle-hidden'));
  assert(elements(tree).some(n=>n.idValue==='space-scratch'));
  assert.equal(h.storage.get('hide-inactive'),'false');
  h.storage.set('hide-inactive','true');h.state.set('vega.hide-inactive',false);
  h.render();assert.equal(h.storage.get('hide-inactive'),'false','shared false overrides an older true copy');
  h.state.delete('vega.hide-inactive');assert(!texts(h.render()).some(s=>s.endsWith('idle hidden')));
});

test('inactive filter combines with label and machine views and hides fully idle projects',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);h.mod.init({},h.cx);
  const agents=h.api.agents();agents[1].status='working';agents[3].status='working';h.api.agents=()=>agents;
  h.mod.preferences={projects:{[project]:{labels:['work']}},views:{Remote:{labels:[],machines:['puck']}}};
  h.state.set('vega.hide-inactive',true);
  for(const view of ['label:work','Remote']) {
    h.state.set('vega.view',view);
    const tree=h.render(),ids=elements(tree).map(n=>n.idValue);
    assert(ids.includes(`uac-${project}`));assert(ids.includes('sub-t2'),'working sentinel retained');
    assert(!ids.includes('space-scratch'),'view still applies to active scratch agent');
    assert(texts(tree).includes('1 idle hidden'),'count excludes sessions outside the view');
  }
  agents[1].status='done';
  assert(elements(h.render()).some(n=>n.idValue==='sub-t2'),'done sentinel remains visible until read');
  agents[1].status='idle';
  const tree=h.render();assert(!elements(tree).some(n=>n.idValue===`uac-${project}`));
  assert(texts(tree).includes('3 idle hidden'));
  h.state.set('vega.hide-inactive',false);
  assert(elements(h.render()).some(n=>n.idValue===`uac-${project}`));
});

test('project pin icon toggles persisted argv, stops header events, and shows CLI errors',async()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);h.mod.init({},h.cx);await h.flush();
  let pinned=false;
  h.api.exec=async(bin,args)=>{
    h.calls.push(['exec',bin,args]);
    if(args.includes('--pin')) pinned=true;
    if(args.includes('--unpin')) pinned=false;
    return {code:0,stdout:JSON.stringify({projects:{[project]:{pinned,labels:[]}},views:{}})};
  };
  // A click's cx is valid only during the handler (herdr-app throws later): use it like the app.
  const click=async handler=>{
    let dead=false;const live=f=>(...a)=>{if(dead)throw new TypeError('cx is no longer valid');return f(...a);};
    const cx={stop_propagation:live(()=>h.cx.stop_propagation()),notify:live(()=>{}),spawn:live(f=>h.cx.spawn(f)),timer:h.cx.timer};
    handler({},cx);dead=true;await h.flush();
  };
  for(const flag of ['--pin','--unpin']) {
    const pin=elements(h.render()).find(n=>n.idValue===`pin-uac-${project}`);
    const stops=h.calls.filter(c=>c[0]==='stop').length;
    pin.handlers['on_mouse_down:left']({},h.cx);
    await click(pin.handlers.on_click);
    assert.equal(h.calls.filter(c=>c[0]==='stop').length,stops+2);
    assert.equal(h.mod.dragFrom,undefined,'pin does not start header drag');
    const call=h.calls.filter(c=>c[0]==='exec'&&c[2].includes(flag)).at(-1);
    assert.equal(call[1],'vega');
    assert.deepEqual(Array.from(call[2]),['project','config',project,flag,'--json']);
    assert.deepEqual(Array.from(h.calls.filter(c=>c[0]==='exec').at(-1)[2]),['project','config','--json']);
    assert.equal(h.mod.preferences.projects[project].pinned,flag==='--pin');
  }
  h.api.exec=async()=>({code:1,stdout:'pin denied'});
  await click(elements(h.render()).find(n=>n.idValue===`pin-uac-${project}`).handlers.on_click);
  assert(texts(h.render()).some(s=>s.includes('pin denied')));
  assert.equal(h.mod.preferences.projects[project].pinned,false);
});

test('pinned idle project retains its UAC only within the current view and excludes it from hidden count',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);h.mod.init({},h.cx);
  h.mod.preferences={projects:{[project]:{pinned:true,labels:['work']}},views:{Remote:{labels:[],machines:['puck']},Local:{labels:[],machines:['local']}}};
  h.state.set('vega.hide-inactive',true);
  for(const view of ['all','label:work','Remote']) {
    h.state.set('vega.view',view);
    const tree=h.render(),ids=elements(tree).map(n=>n.idValue);
    assert(ids.includes(`uac-${project}`));assert(ids.includes('ag-puck/t1'));
    assert(!ids.includes('sub-t2'),'idle sub-agent still hides');
    assert(!ids.includes('ag-t3'),'other idle project agent still hides');
    assert(texts(tree).includes(view==='all'?'3 idle hidden':'2 idle hidden'));
  }
  for(const view of ['label:other','Local']) {
    h.state.set('vega.view',view);
    const tree=h.render();
    assert(!elements(tree).some(n=>n.idValue===`uac-${project}`));
    assert(texts(tree).includes(view==='Local'?'1 idle hidden':'0 idle hidden'));
  }
  h.state.set('vega.view','all');h.api.agents=()=>[];
  assert(elements(h.render()).some(n=>n.idValue==='ag-puck/t1'),'pin retains a UAC without status');
});

test('unknown UAC without a status entry keeps active descendants; narrow rail can focus a terminal-only child and disable filtering',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  h.api.agents=()=>[];
  const all=h.api.terminals();all[1].status='working';h.api.terminals=()=>all;
  h.state.set('vega.hide-inactive',true);h.mod.init({},h.cx);
  let ids=elements(h.render()).map(n=>n.idValue);
  assert(ids.includes(`uac-${project}`));assert(ids.includes('ag-puck/t1'));assert(ids.includes('sub-t2'));
  h.api.layout=()=>({id:'sidebar',size:{px:76}});
  const tree=h.render();
  const child=elements(tree).find(n=>n.idValue==='ag-t2');assert(child);
  child.handlers.on_click({},h.cx);
  assert(h.calls.some(c=>c[0]==='run'&&c[1]==='terminal.peek'&&c[2].terminal==='t2'));
  elements(tree).find(n=>n.idValue==='ag-idle-hidden').handlers.on_click({},h.cx);
  assert.equal(h.state.get('vega.hide-inactive'),false);
});

test('project Config overlay loads once, saves argv including label removal, and keeps failures editable',async()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  h.mod.init({},h.cx);
  const header=elements(h.render()).find(n=>n.idValue===`uac-${project}`);
  header.handlers['on_mouse_down:right']({},h.cx);
  assert(!h.calls.some(c=>c[0]==='run'&&c[1]==='space.rename'));
  const win=h.calls.find(c=>c[0]==='open')[1];assert.equal(win.kind,'window');assert.equal(win.key,`vega-project-${project}`);
  const spec=win;assert.equal(spec.mod,'vega-sidebar');assert.equal(spec.props.projectConfig,project);
  const popup=load('vega-sidebar');popup.setProps({...spec.props,tab:'config',overlay:'config'});
  popup.api.exec=async(bin,args)=>{popup.calls.push(['exec',bin,args]);return {code:0,stdout:JSON.stringify({projects:{[project]:{name:'Name',labels:['work','old']}},views:{}})};};
  popup.mod.init({},popup.cx);await popup.flush();
  let text=texts(popup.render());assert(text.includes('Config'));assert(!text.includes('Rename'));
  assert.equal(popup.mod.nameInput.value(),'Name');assert.equal(popup.calls.filter(c=>c[0]==='exec'&&c[2]?.[0]==='project').length,1);
  popup.mod.nameInput.set_value('');
  popup.api.exec=async(bin,args)=>{popup.calls.push(['exec',bin,args]);return {code:1,stderr:'Read only'};};
  // Removing a label pill saves at once; a failure keeps the edited labels on screen.
  elements(popup.render()).find(n=>n.idValue==='label-old').handlers.on_click({},popup.cx);await popup.flush();
  assert(texts(popup.render()).some(s=>s.includes('Read only')));
  assert.deepEqual([...popup.mod.labels],['work']);
  assert.deepEqual(Array.from(popup.calls.filter(c=>c[0]==='exec'&&c[2]?.[0]==='project').at(-1)[2]),['project','config',project,'--name','','--label','work','--unlabel','old','--json']);
  // Enter in the add input splits commas, skips duplicates and clears the input.
  popup.mod.labelsInput.set_value('private, work, private');
  popup.mod.labelsInput.handlers.submit({},popup.cx);await popup.flush();
  assert.equal(popup.mod.labelsInput.value(),'');
  assert.deepEqual(Array.from(popup.calls.filter(c=>c[0]==='exec'&&c[2]?.[0]==='project').at(-1)[2]),['project','config',project,'--name','','--label','work','--label','private','--unlabel','old','--json']);
  popup.api.exec=async()=>({code:0,stdout:JSON.stringify({projects:{[project]:{name:'',labels:['work','private']}}})});
  await popup.mod.saveProject(popup.cx);assert(texts(popup.render()).includes('Saved'));
  popup.render().handlers.on_key_down({keystroke:'escape'},popup.cx);
  assert(popup.calls.some(c=>c[0]==='close'&&c[1]==='config'));
});

test('names use full IDs, filters keep the UAC tree together and All restores unlabelled spaces',async()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  const prefs={projects:{[project]:{name:'Display A',labels:['work']}},views:{Work:{labels:['work'],machines:['puck']},Other:{labels:['private'],machines:[]},Local:{labels:[],machines:['local']}}};
  h.api.exec=async(bin)=>({code:0,stdout:bin==='vega'?JSON.stringify(prefs):''});
  h.mod.init({},h.cx);await h.flush();
  assert(texts(h.render()).includes('Display A'));
  h.storage.set('active-view','Work');let nodes=elements(h.render());
  assert(nodes.some(n=>n.idValue==='sub-t2'));assert(nodes.some(n=>n.idValue==='ag-t3'));
  assert(!nodes.some(n=>n.idValue==='space-scratch'));
  assert.equal(JSON.stringify(h.calls.filter(c=>c[0]==='order').at(-1)[1]),JSON.stringify(['puck/t1','t2','t3']));
  h.storage.set('active-view','Other');assert(!elements(h.render()).some(n=>typeof n.idValue==='string'&&n.idValue.startsWith('uac-')));
  h.storage.set('active-view','Local');nodes=elements(h.render());
  assert(!nodes.some(n=>typeof n.idValue==='string'&&n.idValue.startsWith('uac-')));assert(nodes.some(n=>n.idValue==='space-scratch'));
  h.storage.set('active-view','all');assert(elements(h.render()).some(n=>n.idValue==='space-scratch'));
  prefs.projects[project].name='';await h.mod.refreshPreferences(h.cx);assert(texts(h.render()).includes('a'));
  h.api.exec=async()=>({code:1,stderr:'Unavailable'});await h.mod.refreshPreferences(h.cx);
  assert.equal(h.mod.preferences.projects[project].labels[0],'work','failed refresh preserves last good data');
});

test('same-named repositories have separate config targets and legacy labels cannot join ambiguously',()=>{
  const h=load('vega-sidebar'),id='github.com/team/a',other='gitlab.com/other/a';
  projectFixture(h);const terminals=h.api.terminals();
  h.api.terminals=()=>[...terminals,{address:'t5',state:'live',labels:{'vega.role':'uac','vega.project-id':other,'vega.project':'a'}},{address:'t6',state:'live',labels:{'vega.project':'a'}}];
  h.mod.init({},h.cx);h.mod.preferences={projects:{[id]:{name:'One'},[other]:{name:'Two'}},views:{}};
  const text=texts(h.render());assert(text.includes('One'));assert(text.includes('Two'));
  const grouping=h.mod.projectGroups(h.api.terminals());assert.equal(grouping.projectOfAgent({address:'t6'}),null);
});

test('projects survive a missing UAC with orphan sentinels, filters, pins and navigation',async()=>{
  const h=load('vega-sidebar'),project='github.com/b4arena/spellkave';
  let all=[
    {address:'ganymede/t1n',state:'live',labels:{}},
    {address:'ganymede/t2',state:'live',parent:'ganymede/t1n',labels:{'vega.project-id':project,'herdr.space':'s'}},
    {address:'ganymede/t3',state:'live',parent:'ganymede/t1n',labels:{'vega.project':'spellkave','herdr.space':'s'}},
    {address:'gone',state:'exited',labels:{'vega.role':'uac','vega.project-id':project}},
  ];
  let status='working';
  h.api.terminals=()=>all;
  h.api.agents=()=>all.slice(1,3).map(a=>({...a,space:'ganymede/s',status,program:{}}));
  h.mod.init({},h.cx);
  h.mod.preferences={projects:{[project]:{name:'Spellkave',labels:['work']}},views:{Work:{labels:['work'],machines:['ganymede']},Local:{labels:[],machines:['local']}}};
  h.state.set('vega.view','Work');
  let tree=h.render(),ids=elements(tree).map(n=>n.idValue);
  assert(texts(tree).includes('Spellkave'));assert(texts(tree).includes('no UAC'));
  assert(ids.includes('ag-ganymede/t2'));assert(ids.includes('ag-ganymede/t3'));
  assert.equal(JSON.stringify(h.calls.filter(c=>c[0]==='order').at(-1)[1]),JSON.stringify(['ganymede/t2','ganymede/t3']));
  elements(tree).find(n=>n.idValue===`gear-uac-${project}`).handlers.on_click({},h.cx);
  assert.equal(h.calls.find(c=>c[0]==='open')[1].props.projectConfig,project);
  h.state.set('vega.view','Local');assert(!projectHeaders(h).length);
  h.state.set('vega.view','Work');h.state.set('vega.hide-inactive',true);
  assert(projectHeaders(h).includes(`uac-${project}`));
  status='idle';assert(!projectHeaders(h).length,'Active hides idle unpinned projects');
  h.mod.preferences.projects[project].pinned=true;
  assert(projectHeaders(h).includes(`uac-${project}`),'pin keeps no-UAC header in Active');
  assert(texts(h.render()).includes('no UAC'));
  h.state.set('vega.hide-inactive',false);
  all=[...all,{address:'ganymede/new-uac',state:'live',labels:{'vega.role':'uac','vega.project-id':project}}];
  assert(!texts(h.render()).includes('no UAC'),'marker clears when a UAC claims the project');
  assert.equal(h.mod.projectGroups(all).projects.get(project).address,'ganymede/new-uac');
});

test('terminals without an agent list under their project; placeholders, exited and sub-agents stay out',()=>{
  const h=load('vega-sidebar'),project='github.com/durandom/rally-ng-meta';
  const fg=(name,argv)=>({name,argv,cwd:'/work/rally'});
  const lab={'vega.project-id':project,'herdr.space':'s'};
  const all=[
    {address:'ganymede/t1',state:'live',labels:{...lab,'vega.role':'uac'}},
    {address:'ganymede/t2s',state:'live',labels:lab,foreground:fg('token-burn',['token-burn','tui'])},
    {address:'ganymede/t2t',state:'live',labels:lab,foreground:fg('zsh',['-zsh'])},
    {address:'ganymede/t3',state:'live',labels:{...lab,'vega.worktree-owner':'t1'},foreground:fg('zsh',['-zsh'])},
    {address:'ganymede/t4',state:'exited',labels:lab,foreground:fg('pdc',['pdc'])},
    {address:'ganymede/t5',state:'live',parent:'ganymede/t1',labels:{},foreground:fg('node',['node'])},
    {address:'ganymede/t6',state:'live',labels:{'herdr.space':'free'},foreground:fg('vite',['vite'])},
  ];
  h.api.terminals=()=>all;
  h.api.agents=()=>[{...all[0],space:'ganymede/s',status:'idle',program:{key:'claude'}}];
  h.mod.init({},h.cx);
  let ids=elements(h.render()).map(n=>n.idValue);
  assert(ids.includes('term-ganymede/t2s'));assert(ids.includes('term-ganymede/t2t'));
  for(const hidden of ['t3','t4','t5','t1'])assert(!ids.includes(`term-ganymede/${hidden}`),hidden);
  assert(ids.indexOf('ag-ganymede/t1')<ids.indexOf('term-ganymede/t2s'),'agents come first');
  assert(texts(h.render()).includes('token-burn'));
  assert(!JSON.stringify(h.calls.filter(c=>c[0]==='order').at(-1)?.[1]??[]).includes('t2s'),'navigation order stays agents only');
  elements(h.render()).find(n=>n.idValue==='term-ganymede/t2s').handlers.on_click({},h.cx);
  h.state.set('vega.hide-inactive',true);
  ids=elements(h.render()).map(n=>n.idValue);
  assert(!ids.includes('term-ganymede/t2t'),'Active hides idle shells');
  h.mod.preferences={projects:{[project]:{pinned:true}}};
  ids=elements(h.render()).map(n=>n.idValue);
  assert(ids.includes('term-ganymede/t2t')&&ids.includes('term-ganymede/t2s'),'pinned project keeps them');
});

test('a child of another project renders in its own project group with a parent hint',()=>{
  const h=load('vega-sidebar'),vega='github.com/durandom/vega',spell='github.com/b4arena/spellkave';
  const all=[
    {address:'proteus/t6p',state:'live',labels:{'vega.role':'uac','vega.project-id':vega,'herdr.space':'s'}},
    {address:'proteus/t7',state:'live',parent:'proteus/t6p',labels:{'vega.role':'sentinel','vega.project-id':vega,'herdr.space':'s'}},
    {address:'proteus/t11v',state:'live',parent:'proteus/t6p',labels:{'vega.role':'sentinel','vega.project-id':spell,'herdr.space':'s'}},
    {address:'ganymede/u',state:'live',labels:{'vega.role':'uac','vega.project-id':spell,'herdr.space':'g'}},
  ];
  h.api.terminals=()=>all;
  h.api.agents=()=>all.map(a=>({...a,space:a.address.startsWith('ganymede')?'ganymede/g':'proteus/s',status:'working',program:{key:'claude'}}));
  h.mod.init({},h.cx);
  const tree=h.render(),ids=elements(tree).map(n=>n.idValue);
  assert(ids.includes('sub-proteus/t7'),'same-project child stays a branch');
  assert(!ids.includes('sub-proteus/t11v'));
  const order=ids.filter(i=>typeof i==='string'&&/^(uac-|ag-|sub-)/.test(i));
  assert(order.indexOf('ag-proteus/t11v')>order.indexOf(`uac-${spell}`)&&order.indexOf('ag-proteus/t11v')<order.indexOf(`uac-${vega}`)&&order.indexOf('sub-proteus/t7')>order.indexOf(`uac-${vega}`),'t11v is under spellkave, not vega');
  assert(texts(tree).includes('↑ t6p'));
});

test('a child of a project without a UAC stays under its parent',()=>{
  const h=load('vega-sidebar'),meta='github.com/b4arena/rally-ng-meta',app='github.com/b4arena/rally-ng-app';
  const all=[
    {address:'io/m',state:'live',labels:{'vega.role':'uac','vega.project-id':meta,'herdr.space':'s'}},
    {address:'io/gdemo',state:'live',parent:'io/m',labels:{'vega.role':'sentinel','vega.project-id':app,'herdr.space':'s'}},
  ];
  h.api.terminals=()=>all;
  h.api.agents=()=>all.map(a=>({...a,space:'io/s',status:'working',program:{key:'claude'}}));
  h.mod.init({},h.cx);
  const tree=h.render(),ids=elements(tree).map(n=>n.idValue);
  assert(ids.includes('sub-io/gdemo'),'child stays a branch of its parent');
  assert(!ids.includes('ag-io/gdemo'));
  assert(!texts(tree).includes('↑ m'));
  assert(!projectHeaders(h).includes(`uac-${app}`),'no empty no-UAC group for the child project');
  h.mod.preferences={projects:{[app]:{pinned:true}}};
  assert(projectHeaders(h).includes(`uac-${app}`),'a pinned project keeps its header');
});

test('legacy-only projects stay visible and no-UAC projects retain manual order',()=>{
  const h=load('vega-sidebar'),[a,b,otherB,c]=orderedProjectsFixture(h);
  const all=h.api.terminals();delete all[0].labels['vega.role'];delete all[3].labels['vega.role'];
  all.push({address:'legacy',state:'live',labels:{'vega.project':'legacy'}});
  h.api.terminals=()=>all;
  const agents=h.api.agents;h.api.agents=()=>[...agents(),{address:'legacy',space:'s',tab:'tb',pane:'legacy',status:'idle',program:{}}];
  h.storage.set('project-order',JSON.stringify([c,b,otherB,a,'legacy']));h.mod.init({},h.cx);
  assert.deepEqual(projectHeaders(h),[c,b,otherB,a,'legacy'].map(id=>`uac-${id}`));
  h.mod.move([c,b,otherB,a,'legacy'],a,c);
  assert.deepEqual(projectHeaders(h),[a,c,b,otherB,'legacy'].map(id=>`uac-${id}`));
  assert.equal(h.mod.projectGroups(all).projectOfAgent({address:'legacy'}),'legacy');
});

test('agent popup pins live terminals and preserves UAC actions and Escape',()=>{
  const h=load('vega-sidebar');projectFixture(h);h.mod.init({},h.cx);
  for(const id of ['ag-puck/t1','sub-t2','ag-t3']) {
    elements(h.render()).find(n=>n.idValue===id).handlers['on_mouse_down:right']({},h.cx);
    const spec=h.calls.filter(c=>c[0]==='open').at(-1)[1];
    assert.equal(spec.kind,'popup');assert.equal(spec.props.agentMenu.address,id.replace(/^(ag|sub)-/,''));
  }
  const m=load('vega-sidebar');const all=h.api.terminals();m.api.terminals=()=>all;
  m.setProps({agentMenu:{address:'puck/t1'},overlay:'menu'});m.mod.init({},m.cx);
  assert.equal(m.calls.length,0,'menu starts no navigation or reads');
  let nodes=elements(m.render());assert(texts(nodes.find(n=>n.idValue==='pin-agent')).includes('Pin'));
  nodes.find(n=>n.idValue==='pin-agent').handlers.on_click({},m.cx);
  assert.equal(JSON.stringify(m.calls.find(c=>c[0]==='run')),JSON.stringify(['run','terminal.pin',{terminal:'puck/t1',pinned:true}]));
  all[0].labels['herdr.pinned']='true';nodes=elements(m.render());
  assert(texts(nodes.find(n=>n.idValue==='pin-agent')).includes('Unpin'));
  nodes.find(n=>n.idValue==='pin-agent').handlers.on_click({},m.cx);
  assert.equal(m.calls.filter(c=>c[0]==='run').at(-1)[2].pinned,false);
  nodes.find(n=>n.idValue==='agent-project').handlers.on_click({},m.cx);
  assert.equal(m.calls.filter(c=>c[0]==='open').at(-1)[1].props.projectConfig,'github.com/team/a');
  m.render().handlers.on_key_down({keystroke:'escape'},m.cx);
  assert(m.calls.some(c=>c[0]==='close'&&c[1]==='menu'));
  m.setProps({agentMenu:{address:'t3'},overlay:'menu'});
  assert(!elements(m.render()).some(n=>n.idValue==='agent-project'));
  all.find(t=>t.address==='t3').state='exited';
  assert(!elements(m.render()).some(n=>n.idValue==='pin-agent'),'vanished targets have no pin action');
});

test('pins survive Active and only cross saved view filters with Respect pins',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);const all=h.api.terminals();
  for(const address of ['t2','t3','t4'])all.find(t=>t.address===address).labels['herdr.pinned']='true';
  all.push({address:'puck/sibling',state:'live',parent:'puck/t1',status:'working',labels:{}});
  h.api.terminals=()=>all;h.mod.init({},h.cx);h.state.set('vega.hide-inactive',true);
  const ids=()=>elements(h.render()).map(n=>n.idValue);
  for(const id of ['ag-puck/t1','sub-t2','ag-t3','ag-t4'])assert(ids().includes(id),id);
  assert(elements(elements(h.render()).find(n=>n.idValue==='sub-t2')).some(n=>n.tooltipValue==='Pinned agent'));
  h.mod.preferences={projects:{[project]:{labels:['work']}},views:{Strict:{labels:['other'],machines:['other']}}};
  h.state.set('vega.view','Strict');assert(!ids().includes(`uac-${project}`));assert(!ids().includes('ag-t4'));
  h.mod.preferences.views.Strict.respect_pins=true;
  for(const active of [false,true]) {
    h.state.set('vega.hide-inactive',active);
    for(const id of [`uac-${project}`,'sub-t2','ag-t3','ag-t4'])assert(ids().includes(id),`${id}, Active=${active}`);
    assert(!ids().includes('sub-puck/sibling'),'unpinned working sibling cannot cross view boundary');
    h.api.layout=()=>({id:'sidebar',size:{px:76}});
    const nodes=elements(h.render());assert(nodes.some(n=>n.idValue==='ag-t2'),'rail includes pinned child');
    nodes.find(n=>n.idValue==='ag-t3').handlers['on_mouse_down:right']({},h.cx);
    assert.equal(h.calls.filter(c=>c[0]==='open').at(-1)[1].props.agentMenu.address,'t3');
    h.api.layout=()=>null;
  }
  h.mod.preferences.projects[project].pinned=true;
  assert(ids().includes('sub-puck/sibling'),'pinned project admits its working members');
  h.mod.preferences.views.Strict.respect_pins=false;
  assert(!ids().includes(`uac-${project}`),'project pins also obey strict views');
  h.state.set('vega.view','label:other');assert(!ids().includes(`uac-${project}`),'ad-hoc view remains strict');
  h.state.set('vega.view','all');delete all.find(t=>t.address==='t3').labels['herdr.pinned'];
  assert(!ids().includes('ag-t3'),'unpinning idle agent restores Active filtering');
});

test('view popup edits, saves and clears Respect pins with the whole saved view',async()=>{
  const h=load('vega-sidebar');h.setProps({viewMenu:true,overlay:'views'});
  const prefs={projects:{},views:{Work:{labels:['work'],machines:[],respect_pins:true},Old:{labels:['work'],machines:[]}}};
  h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,args]);return {code:0,stdout:JSON.stringify(prefs)};};
  h.mod.init({},h.cx);await h.flush();assert.equal(h.mod.viewRespectPins,false);
  elements(h.render()).find(n=>n.idValue==='edit-view-Work').handlers.on_click({},h.cx);
  assert.equal(h.mod.viewRespectPins,true);await h.mod.saveView(h.cx);
  const argv=()=>h.calls.filter(c=>c[0]==='exec'&&c[2][1]==='view').at(-1)[2];
  assert(argv().includes('--respect-pins'));
  elements(h.render()).find(n=>n.idValue==='respect-pins').handlers.on_click({},h.cx);
  await h.mod.saveView(h.cx);assert(!argv().includes('--respect-pins'));
  h.mod.viewRespectPins=true;
  elements(h.render()).find(n=>n.idValue==='edit-view-Old').handlers.on_click({},h.cx);
  assert.equal(h.mod.viewRespectPins,false,'old views reset the editor toggle');
});

test('view popup selects, saves and deletes CLI-owned views without persisting config in shared state',async()=>{
  const h=load('vega-sidebar');h.setProps({viewMenu:true,overlay:'views'});
  let preferences={projects:{},views:{Work:{labels:['work'],machines:['puck']}}};
  h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,args]);return {code:0,stdout:JSON.stringify(preferences)};};
  h.mod.init({},h.cx);await h.flush();
  elements(h.render()).find(n=>n.idValue==='edit-view-Work').handlers.on_click({},h.cx);
  assert.equal(h.mod.viewMachines.value(),'puck');
  h.mod.viewName.set_value('Focus');h.mod.viewMachines.set_value('puck, local');
  await h.mod.saveView(h.cx);
  const saved=h.calls.filter(c=>c[0]==='exec'&&c[2][1]==='view').at(-1);
  assert.deepEqual(Array.from(saved[2]),['project','view','Focus','--label','work','--machine','puck','--machine','local','--match','all','--json']);
  await h.mod.saveView(h.cx,true,'Work');
  assert.deepEqual(Array.from(h.calls.filter(c=>c[0]==='exec'&&c[2][1]==='view').at(-1)[2]),['project','view','Work','--delete','--json']);
  elements(h.render()).find(n=>n.idValue==='view-Work').handlers.on_click({},h.cx);
  assert.equal(h.state.get('vega.view'),'Work','the selection reaches the sidebar copy through shared state');
  assert.deepEqual([...h.state.keys()],['vega.view'],'no project config in shared state');
  const source=fs.readFileSync(new URL('../config/herdr-app/mods/vega-sidebar/main.js',import.meta.url),'utf8');
  assert(!source.includes('sortMenu'));assert(!source.includes('Grouped by space'));assert(!source.includes('Sort agents'));
});

function orderedProjectsFixture(h) {
  const ids=['github.com/team/a','github.com/team/b','gitlab.com/other/b','github.com/team/c'];
  h.api.terminals=()=>ids.map((id,i)=>({address:`t${i}`,state:'live',labels:{'vega.role':'uac','vega.project-id':id,'vega.project':id.split('/').at(-1)}}));
  h.api.agents=()=>ids.map((id,i)=>({address:`t${i}`,space:'s',tab:'tb',pane:`t${i}`,status:'idle',program:{}}));
  return ids;
}
const projectHeaders=h=>elements(h.render()).map(n=>n.idValue).filter(id=>typeof id==='string'&&id.startsWith('uac-'));

test('legacy short-name manual order applies only to unambiguous project IDs after restart',()=>{
  const h=load('vega-sidebar'),[a,b,otherB,c]=orderedProjectsFixture(h);
  h.storage.set('project-order',JSON.stringify(['c','b','a']));
  h.mod.init({},h.cx);
  assert.deepEqual(projectHeaders(h),[c,a,b,otherB].map(id=>`uac-${id}`),
    'unique c and a retain saved precedence; neither same-named b inherits the ambiguous saved b position');
  assert.equal(JSON.stringify(h.calls.filter(c=>c[0]==='order').at(-1)[1]),JSON.stringify(['t3','t0','t1','t2']),
    'navigation follows the migrated manual order');
});

test('dragging a filtered view swaps visible slots and preserves hidden project positions across restart',()=>{
  const h=load('vega-sidebar'),[a,b,otherB,c]=orderedProjectsFixture(h);
  h.storage.set('project-order',JSON.stringify([a,b,otherB,c]));
  h.storage.set('active-view','Focus');
  h.mod.init({},h.cx);
  h.mod.preferences={projects:{[a]:{labels:['work']},[c]:{labels:['work']}},views:{Focus:{labels:['work'],machines:[]}}};
  assert.deepEqual(projectHeaders(h),[`uac-${a}`,`uac-${c}`]);
  const press=(id,event)=>elements(h.render()).find(n=>n.idValue===`uac-${id}`).handlers[event]({},h.cx);
  press(c,'on_mouse_down');press(a,'on_mouse_move');press(a,'on_mouse_up');
  assert.deepEqual(projectHeaders(h),[`uac-${c}`,`uac-${a}`]);
  const stored=h.storage.get('project-order');
  assert.deepEqual(JSON.parse(stored),[c,b,otherB,a], 'hidden b projects keep their exact saved slots');
  h.storage.set('active-view','all');assert.deepEqual(projectHeaders(h),[c,b,otherB,a].map(id=>`uac-${id}`));
  const restarted=load('vega-sidebar');orderedProjectsFixture(restarted);
  restarted.storage.set('project-order',stored);restarted.mod.init({},restarted.cx);
  assert.deepEqual(projectHeaders(restarted),[c,b,otherB,a].map(id=>`uac-${id}`));
});

test('deleting the selected view restores the All header and unfiltered projects and spaces',async()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  let prefs={projects:{[project]:{name:'Project A',labels:['work']}},views:{Focus:{labels:['work'],machines:['puck']}}};
  h.api.exec=async(bin)=>({code:0,stdout:bin==='vega'?JSON.stringify(prefs):''});
  h.storage.set('active-view','Focus');h.mod.init({},h.cx);await h.flush();
  let tree=h.render();assert(texts(tree).includes('Focus ▾'));
  assert(!elements(tree).some(n=>n.idValue==='space-scratch'));
  prefs={...prefs,views:{}};await h.mod.refreshPreferences(h.cx);
  tree=h.render();const selector=elements(tree).find(n=>n.idValue==='ag-views');
  assert.deepEqual(texts(selector),['All ▾']);
  assert(elements(tree).some(n=>n.idValue==='space-scratch'));
  assert(elements(tree).some(n=>n.idValue===`uac-${project}`));
});

test('view editor explains invalid names and empty filters; Escape cancels without writing',async()=>{
  const h=load('vega-sidebar');h.setProps({viewMenu:true,overlay:'views'});
  h.api.exec=async()=>({code:0,stdout:'{"projects":{},"views":{}}'});
  h.mod.init({},h.cx);await h.flush();
  h.api.exec=async()=>{assert.fail('invalid view must not execute a write');};
  for(const name of ['', 'all']) {
    h.mod.viewName.set_value(name);h.mod.viewLabels.set_value('work');
    await h.mod.saveView(h.cx);
    assert(texts(h.render()).some(s=>s.includes('Enter a view name')));
  }
  h.mod.viewName.set_value('Work');h.mod.viewLabels.set_value('');await h.mod.saveView(h.cx);
  assert(texts(h.render()).some(s=>s.includes('Choose at least one label or machine')));
  h.render().handlers.on_key_down({keystroke:'escape'},h.cx);
  assert(h.calls.some(c=>c[0]==='close'&&c[1]==='views'));
});

test('Asks tab lists project asks from vega notify, acks and resolves by argv, shows failures and empty state',async()=>{
  const project='github.com/team/a';
  const w=load('vega-sidebar');w.setProps({projectConfig:project,tab:'asks',uac:'puck/t1'});
  let asks=[{id:'puck=17',host:'puck',level:'action',project,pane:'hc:t9',line:'a: please test apply -> t9',bead:'a-1',cmd:'mise run apply',acknowledged:false}];
  let reply={code:0,stdout:''};
  w.api.exec=async(bin,args,opts)=>{w.calls.push(['exec',bin,[...args],opts]);
    return args[1]==='list'?{code:0,stdout:JSON.stringify({schema:1,asks,registry_error:null})}:args[0]==='notify'?reply:{code:0,stdout:'{}'};};
  w.mod.init({},w.cx);await w.flush();
  const list=w.calls.find(c=>c[0]==='exec'&&c[2][1]==='list');
  assert.deepEqual(list[2],['notify','list','--json','--project',project],'remote project asks read locally from the registry');
  let text=texts(w.render());
  assert(text.includes('a: please test apply -> t9'));assert(text.some(s=>s.includes('$ mise run apply')));
  let nodes=elements(w.render());
  nodes.find(n=>n.idValue==='ask-focus-puck=17').handlers.on_click({},w.cx);await w.flush();
  assert(w.calls.some(c=>c[0]==='run'&&c[1]==='pane.focus'&&c[2].terminal==='t9'));
  asks=[{...asks[0],acknowledged:true}];
  nodes.find(n=>n.idValue==='ask-ack-puck=17').handlers.on_click({},w.cx);await w.flush();
  assert(w.calls.some(c=>c[0]==='exec'&&c[1]==='vega'&&JSON.stringify(c[2])===JSON.stringify(['notify','ack','puck=17'])));
  nodes=elements(w.render());
  assert(!nodes.some(n=>n.idValue==='ask-ack-puck=17'),'acknowledged ask hides Ack');
  assert(texts(w.render()).includes('Acknowledged.'));
  reply={code:2,stderr:'origin update timed out\nhint'};
  nodes.find(n=>n.idValue==='ask-resolve-puck=17').handlers.on_click({},w.cx);await w.flush();
  assert(texts(w.render()).includes('origin update timed out'),'failure keeps the ask with the first error line');
  reply={code:0,stdout:'ask: resolved'};asks=[];
  elements(w.render()).find(n=>n.idValue==='ask-resolve-puck=17').handlers.on_click({},w.cx);await w.flush();
  assert(w.calls.some(c=>c[0]==='exec'&&JSON.stringify(c[2])===JSON.stringify(['notify','resolve','puck=17'])));
  assert(texts(w.render()).includes('No open asks'));
});

test('label pills filter the sidebar ad hoc; the view editor toggles known labels and machines',async()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  const prefs={projects:{[project]:{name:'A',labels:['work','ops','x']}},views:{}};
  h.api.exec=async(bin)=>({code:0,stdout:bin==='vega'?JSON.stringify(prefs):''});
  h.mod.init({},h.cx);await h.flush();
  let nodes=elements(h.render());
  assert(nodes.some(n=>n.idValue===`plabel-uac-${project}-work`));assert(nodes.some(n=>n.idValue===`plabel-uac-${project}-more`));
  nodes.find(n=>n.idValue===`plabel-uac-${project}-ops`).handlers.on_click({},h.cx);
  assert.equal(h.state.get('vega.view'),'label:ops');
  nodes=elements(h.render());assert.equal(h.storage.get('active-view'),'label:ops','sidebar keeps the view across restarts');
  assert(!nodes.some(n=>n.idValue==='space-scratch'),'label filter hides unlabelled spaces');
  assert.deepEqual(texts(nodes.find(n=>n.idValue==='ag-views')),['#ops ▾']);
  nodes.find(n=>n.idValue===`plabel-uac-${project}-ops`).handlers.on_click({},h.cx);
  assert.equal(h.state.get('vega.view'),'all','second click shows all');
  const m=load('vega-sidebar');m.setProps({viewMenu:true,overlay:'views'});m.api.terminals=h.api.terminals;
  m.api.exec=async(bin,args)=>{m.calls.push(['exec',bin,[...args]]);return {code:0,stdout:JSON.stringify(prefs)};};
  m.mod.init({},m.cx);await m.flush();
  nodes=elements(m.render());
  nodes.find(n=>n.idValue==='pick-label-work').handlers.on_click({},m.cx);
  nodes.find(n=>n.idValue==='pick-machine-puck').handlers.on_click({},m.cx);
  assert.equal(m.mod.viewLabels.value(),'work');assert.equal(m.mod.viewMachines.value(),'puck');
  elements(m.render()).find(n=>n.idValue==='match-any').handlers.on_click({},m.cx);
  m.mod.viewName.set_value('Work');await m.mod.saveView(m.cx);
  assert.deepEqual(m.calls.filter(c=>c[0]==='exec'&&c[2][1]==='view').at(-1)[2],['project','view','Work','--label','work','--machine','puck','--match','any','--json']);
  assert.equal(m.state.get('vega.view'),'Work','a saved view is shown at once');
  elements(m.render()).find(n=>n.idValue==='view-label-ops').handlers.on_click({},m.cx);
  assert.equal(m.state.get('vega.view'),'label:ops');
});

test('waiting panes offer Dismiss for silent and asking only, run vega mux dismiss on the badge machine',async()=>{
  const project='github.com/team/a';
  const w=load('vega-sidebar');
  w.api.terminals=()=>[{address:'ganymede/t1n',state:'live',labels:{'vega.role':'uac','vega.project-id':project}}];
  w.setProps({projectConfig:project,tab:'overview',uac:'ganymede/t1n'});
  w.state.set('vega.badges',[{project,machine:'ganymede',checkout:'/remote/a',at:Date.now()/1000,decisions:0,notes:0,waiting:2,
    waiting_panes:[{pane:'hc:ganymede/t1w',name:'fable-chat',attention:'silent',title:''},{pane:'hc:ganymede/t1x',name:'dialog',attention:'input',title:'approve'}]}]);
  w.api.exec=async(bin,args)=>{w.calls.push(['exec',bin,[...args]]);return bin==='python3'&&args.includes('dismiss')?{code:0,stdout:JSON.stringify({code:0,stdout:'dismissed',stderr:''})}:{code:0,stdout:'{}'};};
  w.mod.init({},w.cx);await w.flush();
  const nodes=elements(w.render());
  assert(nodes.some(n=>n.idValue==='ov-wait-go-hc:ganymede/t1x'));
  assert(!nodes.some(n=>n.idValue==='ov-wait-dismiss-hc:ganymede/t1x'),'a live dialog cannot be dismissed');
  nodes.find(n=>n.idValue==='ov-wait-dismiss-hc:ganymede/t1w').handlers.on_click({},w.cx);await w.flush();
  const call=w.calls.find(c=>c[0]==='exec'&&c[2].includes('dismiss'));
  assert.equal(call[1],'python3');
  assert.deepEqual(call[2].slice(2),['ganymede','/remote/a','vega','mux','dismiss','hc:t1w'],'machine-local pane id on the parent machine');
  assert(!elements(w.render()).some(n=>n.idValue==='ov-wait-hc:ganymede/t1w'),'successful dismissal removes the item immediately');

  const i=load('vega-inbox');i.setProps({project});i.api.terminals=()=>[{address:'t5',state:'live'}];
  i.state.set('vega.badges',[{project,at:Date.now()/1000,checkout:'/repo',decision_texts:[],waiting_panes:[{pane:'hc:t5',name:'chat',attention:'asking',title:'asks: ok?'}]}]);
  i.api.exec=async(bin,args)=>{i.calls.push(['exec',bin,[...args]]);return {code:0,stdout:'dismissed'};};
  i.mod.init({},i.cx);
  elements(i.render()).find(n=>n.idValue==='dismiss-wait-hc:t5').handlers.on_click({},i.cx);await new Promise(r=>setImmediate(r));
  assert.deepEqual(i.calls.find(c=>c[0]==='exec'&&c[2].includes('dismiss')).slice(1),['vega',['mux','dismiss','hc:t5']],'local badge: local vega');
});

test('a match-any view shows projects with a chosen label or on a chosen machine, and that machine spaces',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  h.mod.init({},h.cx);
  h.mod.preferences={projects:{[project]:{labels:['vega']}},views:{Work:{labels:['vega'],machines:['local'],match:'any'},Strict:{labels:['vega'],machines:['local'],match:'all'}}};
  h.storage.set('active-view','Work');let nodes=elements(h.render());
  assert(nodes.some(n=>n.idValue===`uac-${project}`),'label vega matches although the UAC runs on puck');
  assert(nodes.some(n=>n.idValue==='space-scratch'),'local spaces match by machine');
  h.storage.set('active-view','Strict');nodes=elements(h.render());
  assert(!nodes.some(n=>n.idValue===`uac-${project}`),'all: label and machine both needed');
});

test('keychain popup opens once globally per lock, click reopens only its own terminal, stays until closed',async()=>{
  const h=load('vega'), now=Date.now()/1000;
  const row={host:'puck',state:'locked',since:now-10,at:now};
  h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,[...args]]);return {code:0,stdout:'{"address":"puck/t42"}'};};
  h.mod.applyKeychains([row],now,h.cx);
  h.mod.applyKeychains([{...row,at:now+1}],now+1,h.cx);
  await h.flush();
  let opens=h.calls.filter(c=>c[0]==='open');
  assert.equal(opens.length,1);
  const start=h.calls.find(c=>c[0]==='exec'&&c[2][0]==='run');
  assert.deepEqual(start[2].slice(0,8),['run','--json','--bg','--no-parent','--no-notify','--on','puck','--']);
  assert(!start[2].includes('--label'),'unlock terminal is never adopted into a tab');
  const script=start[2][start[2].indexOf('-c')+1];
  assert(script.indexOf('read -r')<script.indexOf('stty -echo; security unlock-keychain'),'Enter before the unlock runs');
  assert(script.includes('stty -echo; security unlock-keychain'));
  assert(script.includes('Command (runs on'),'shows the concrete command');
  assert(script.trimEnd().endsWith('IFS= read -r enter'),'the terminal stays after the unlock');
  assert(!script.includes('unlock-keychain -p'));
  assert.equal(opens[0][1].layout.children[1].content.terminal,'puck/t42');
  assert.equal(opens[0][1].kind,'window');
  h.api.terminals=()=>[{address:'puck/t42',state:'live'}];
  // Another copy shares the same dedupe state (as multiple status bars do).
  const copy=new h.mod.constructor();copy.applyKeychains([row],now,h.cx);await h.flush();
  assert.equal(h.calls.filter(c=>c[0]==='open').length,1);
  h.api.focused=()=>({terminal:'puck/t1'});
  const red=elements(h.render()).find(e=>e.idValue==='keychain-puck');
  red.handlers.on_click({},h.cx);await h.flush();
  assert.equal(h.calls.filter(c=>c[0]==='open').length,2,'click reopens a dismissed popup');
  assert.equal(h.calls.filter(c=>c[0]==='exec'&&c[2][0]==='run').length,1,'reuse existing dedicated terminal');
  const before=h.calls.length;
  h.mod.applyKeychains([{...row,state:'unlocked',since:now}],now,h.cx);await h.flush();
  assert(!h.calls.slice(before).some(c=>c[0]==='close'),'an unlock keeps the window for the Slayer to close');
  assert(!h.calls.some(c=>c[0]==='exec'&&c[2][0]==='kill'));
  const title=load('vega');title.state.set('vega.keychains',h.state.get('vega.keychains'));
  title.state.set('vega.keychain-popups',h.state.get('vega.keychain-popups'));
  title.mod.init({keychainTitle:'puck',overlay:'kc'},title.cx);
  assert(texts(title.render()).some(x=>x.startsWith('unlocked')),'title shows the new state');
  elements(title.render()).find(e=>e.idValue==='keychain-close').handlers.on_click({},title.cx);await title.flush();
  assert(title.calls.some(c=>c[0]==='close'));
  assert(title.calls.some(c=>c[0]==='exec'&&c[2][0]==='kill'&&c[2][1]==='puck/t42'),'closing kills only its terminal');
  for(const k of ['vega.keychain-popups']) h.state.set(k,title.state.get(k));
  assert(!h.calls.some(c=>c[0]==='exec'&&['send','keys','read'].includes(c[2][0])));
  h.mod.applyKeychains([{...row,since:now+1}],now,h.cx);await h.flush();
  assert.equal(h.calls.filter(c=>c[0]==='open').length,3,'new since is a new lock');
});

test('keychain status uses theme colours, host and since tooltips, and stale data becomes unknown',async()=>{
  const h=load('vega'), now=Date.now()/1000;
  const t={ansi1:'theme-red',ansi2:'theme-green',dim:'theme-dim'};h.api.theme=()=>t;
  h.mod.applyKeychains(['unlocked','locked','unknown'].map((state,i)=>({host:['localmac','puck','offline'][i],state,since:now-30,at:now,agent:{state:'ready',keys:2}})),now,h.cx);
  await h.flush();
  h.mod.localKeychainHost='localmac';
  const lockOf=focus=>{h.api.focused=()=>({terminal:focus});return elements(h.render()).filter(n=>String(n.idValue).startsWith('keychain-'));};
  for(const [focus,host,color] of [['t1','localmac',t.ansi2],['puck/t1','puck',t.ansi1],['offline/t3','offline',t.dim]]) {
    const locks=lockOf(focus);
    assert.equal(locks.length,1,'one lock: the focused terminal\'s machine');
    const n=locks[0];assert.equal(n.idValue,`keychain-${host}`);
    assert.equal(n.text_colorValue,color);assert(n.tooltipValue.includes(host));assert(n.tooltipValue.includes('since'));
  }
  const opens=h.calls.filter(c=>c[0]==='open').length;
  lockOf('t1')[0].handlers.on_click({},h.cx);
  lockOf('offline/t3')[0].handlers.on_click({},h.cx);await h.flush();
  assert.equal(h.calls.filter(c=>c[0]==='open').length,opens);
  h.mod.applyKeychains([{host:'puck',state:'locked',since:1,at:now}],now-181,h.cx);
  assert.equal(h.mod.keychains[0].state,'unknown','stopped vegad cannot leave a red/green claim');
});

test('unlock observation during terminal creation suppresses the popup and kills only the created terminal',async()=>{
  const h=load('vega'), now=Date.now()/1000;let finish;
  h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,[...args]]);return args[0]==='run'?await new Promise(r=>{finish=r;}):{code:0,stdout:''};};
  const row={host:'puck',state:'locked',since:now-1,at:now};
  h.mod.applyKeychains([row],now,h.cx);
  h.mod.applyKeychains([{...row,state:'unlocked',since:now}],now,h.cx);
  finish({code:0,stdout:'{"address":"puck/t42"}'});await h.flush();
  assert(!h.calls.some(c=>c[0]==='open'));
  assert(h.calls.some(c=>c[0]==='exec'&&c[2][0]==='kill'&&c[2][1]==='puck/t42'));
});

test('keychain popup titlebar has no pollers, provides close, and never adopts or focuses a terminal',()=>{
  const h=load('vega');h.setProps({keychainTitle:'puck',overlay:'unlock-window'});
  h.mod.init(undefined,h.cx);
  assert(texts(h.render()).includes('Login keychain · puck'));
  elements(h.render()).find(e=>e.idValue==='keychain-close').handlers.on_click({},h.cx);
  assert(h.calls.some(c=>c[0]==='close'&&c[1]==='unlock-window'));
  assert(!h.calls.some(c=>c[0]==='exec'||c[0]==='run'));
});

test('persisted keychain overlay ids and pending launches cannot affect a new app session',async()=>{
  const h=load('vega'),now=Date.now()/1000;
  h.state.set('vega.keychain-popups',{puck:{overlay:'reused-id',terminal:'puck/t42',session:'previous-app'}});
  h.state.set('vega.keychain-opening',[{host:'puck',at:Date.now(),session:'previous-app'}]);
  h.api.terminals=()=>[{address:'puck/t42',state:'live'}];
  h.mod.applyKeychains([{host:'puck',state:'locked',since:now-10,at:now}],now,h.cx);await h.flush();
  assert.equal(h.calls.filter(c=>c[0]==='open').length,1,'old pending launch does not suppress reopening');
  assert(!h.calls.some(c=>c[0]==='close'&&c[1]==='reused-id'),'old overlay id may now belong to another window');
  h.state.set('vega.keychain-popups',{puck:{overlay:'other-reused-id',terminal:'puck/t42',session:'previous-app'}});
  h.mod.applyKeychains([{host:'puck',state:'unlocked',since:now,at:now}],now,h.cx);await h.flush();
  assert(!h.calls.some(c=>c[0]==='close'&&c[1]==='other-reused-id'));
});

test('an open unlock window probes its host directly and turns the lock green before vegad',async()=>{
  const h=load('vega'),now=Date.now()/1000;
  h.mod.localKeychainHost='proteus';
  h.api.exec=async(bin,args)=>{h.calls.push(['exec',bin,[...args]]);return bin==='hc'&&args[0]==='exec'?{code:0,stdout:'{"code":0}'}:{code:0,stdout:'{}'};};
  const row={host:'ganymede',state:'locked',since:now-100,at:now-30};
  h.state.set('vega.keychain-seen',['ganymede/'+row.since]);
  h.mod.applyKeychains([row],now,h.cx);
  h.mod.probeOpenPopups(h.cx);await h.flush();
  assert(!h.calls.some(c=>c[2]?.[0]==='exec'),'no window: no direct probe');
  h.state.set('vega.keychain-popups',{ganymede:{terminal:'ganymede/t9',overlay:'o',session:'x'}});
  h.mod.probeOpenPopups(h.cx);h.mod.probeOpenPopups(h.cx);await h.flush();
  const probes=h.calls.filter(c=>c[1]==='hc'&&c[2][0]==='exec');
  assert.equal(probes.length,1,'throttled across calls');
  assert.deepEqual(probes[0][2].slice(0,5),['exec','--json','--on','ganymede','--timeout']);
  assert(probes[0][2].at(-1).startsWith('security show-keychain-info'),'read-only probe');
  assert.equal(h.mod.keychains[0].state,'unlocked');
  h.mod.applyKeychains([row],now,h.cx);
  assert.equal(h.mod.keychains[0].state,'unlocked','an older registry sample cannot turn it red again');
  h.mod.applyKeychains([{...row,at:now+60,since:now+60}],now+60,h.cx);
  assert.equal(h.mod.keychains[0].state,'locked','a newer locked probe wins');
});

test('Models tab reads host cells including inbox-only hosts and independent defaults; exact set/clear argv and inline errors',async()=>{
  const h=load('vega-sidebar');h.setProps({globalConfig:true});
  const cell={seat:'herdr-agents',lane:'codex',tier:'budget',model:'project-model',source:'project',until:1234567890,allowed:['gpt-6-luna','gpt-6.1-terra']};
  let rows=[{...cell,host:'proteus'},{...cell,host:'puck',model:'gpt-6.1-terra',source:'host'},
    {...cell,host:'puck',lane:'pi',model:'glm-4.7',source:'global',allowed:['google/gemini-4-argon']}];
  let mutation={code:0,stdout:'action: set'}, readError=null, defaultError=null;
  h.api.exec=async(bin,args)=>{
    h.calls.push(['exec',bin,[...args]]);
    if(args[0]!=='lanista') return {code:0,stdout:'{}'};
    if(args[1]==='hosts') return readError??{code:0,stdout:JSON.stringify({rows})};
    if(args.includes('--set')||args.includes('--clear')) return mutation;
    return defaultError??{code:0,stdout:'model: gpt-6-luna\nsource: default\nexpires: never\n'};
  };
  h.mod.init({},h.cx);await h.flush();
  let nodes=elements(h.render()), text=texts(h.render());
  assert(nodes.some(n=>n.idValue==='settings-tab-models'));
  assert(nodes.some(n=>n.idValue==='models-group-puck'),'host list is independent of connected hc machines');
  assert(text.includes('project-model · project'));assert(text.includes('gpt-6-luna · default'));
  assert(text.includes('glm-4.7 · global'),'pi can show its zai fallback outside the pi allowlist');
  assert(!nodes.some(n=>typeof n.idValue==='string'&&n.idValue.startsWith('model-default/herdr-agents/pi')));
  assert(!h.calls.some(c=>c[0]==='exec'&&c[2][2]==='pi'&&c[2].includes('--default')));
  const key='puck/herdr-agents/codex/budget';
  nodes.find(n=>n.idValue===`model-select-${key}`).handlers.on_mouse_down({},h.cx);
  assert(h.calls.some(c=>c[0]==='stop'),'controls stop window focus mouse-down');
  nodes.find(n=>n.idValue===`model-select-${key}`).handlers.on_click({},h.cx);
  nodes=elements(h.render());
  assert.deepEqual(nodes.filter(n=>typeof n.idValue==='string'&&n.idValue.startsWith(`model-option-${key}/`)).map(n=>texts(n)[0]),cell.allowed);
  rows=rows.map(r=>r.host==='puck'&&r.lane==='codex'?{...r,model:'gpt-6-luna',source:'host'}:r);
  nodes.find(n=>n.idValue===`model-option-${key}/gpt-6-luna`).handlers.on_click({},h.cx);await h.flush();
  const argv=()=>h.calls.filter(c=>c[0]==='exec'&&c[2][0]==='lanista').map(c=>c[2]);
  assert(argv().some(a=>JSON.stringify(a)===JSON.stringify(['lanista','pin','codex','--tier','budget','--host','puck','--set','gpt-6-luna'])));
  assert(texts(h.render()).includes('gpt-6-luna · host'));
  let reads=argv().filter(a=>a[1]==='hosts').length;
  elements(h.render()).find(n=>n.idValue===`model-clear-${key}`).handlers.on_click({},h.cx);await h.flush();
  assert(argv().some(a=>JSON.stringify(a)===JSON.stringify(['lanista','pin','codex','--tier','budget','--host','puck','--clear'])));
  assert(argv().filter(a=>a[1]==='hosts').length>reads,'refresh after each action');
  const def='default/herdr-agents/codex/budget';
  elements(h.render()).find(n=>n.idValue===`model-select-${def}`).handlers.on_click({},h.cx);
  elements(h.render()).find(n=>n.idValue===`model-option-${def}/gpt-6.1-terra`).handlers.on_click({},h.cx);await h.flush();
  assert(argv().some(a=>JSON.stringify(a)===JSON.stringify(['lanista','pin','codex','--tier','budget','--default','--set','gpt-6.1-terra'])));
  mutation={code:2,stdout:'error: broker write denied'};
  elements(h.render()).find(n=>n.idValue===`model-clear-${def}`).handlers.on_click({},h.cx);await h.flush();
  assert(argv().some(a=>JSON.stringify(a)===JSON.stringify(['lanista','pin','codex','--tier','budget','--default','--clear'])));
  assert(texts(h.render()).includes('error: broker write denied'));
  defaultError={code:1,stderr:'default unavailable'};await h.mod.readModels(h.cx);
  assert(texts(h.render()).includes('default unavailable'));
  readError={code:1,stdout:'error: hosts unavailable'};await h.mod.readModels(h.cx);
  assert(texts(h.render()).includes('error: hosts unavailable'));
  assert(elements(h.render()).some(n=>n.idValue==='models-group-puck'),'failed reads retain last snapshot');
  h.api.exec=async()=>{throw new Error('exec failed');};await h.mod.readModels(h.cx);
  assert(texts(h.render()).includes('exec failed'));
  const other=load('vega-sidebar');other.setProps({globalConfig:true});other.state.set('vega.models',h.state.get('vega.models'));
  assert(texts(other.mod.modelsTab({})).includes('gpt-6-luna · host'),'other mod copies use shared data');
  readError=null;rows=[];h.api.exec=async()=>({code:0,stdout:JSON.stringify({rows})});await h.mod.readModels(h.cx);
  assert(texts(h.render()).includes('No known hosts'));
});

test('vega settings gear works without a project, closes from its titlebar and adds no timers',async()=>{
  const h=load('vega-sidebar');h.mod.init({},h.cx);
  elements(h.render()).find(n=>n.idValue==='vega-settings').handlers.on_click({},h.cx);
  const spec=h.calls.find(c=>c[0]==='open')[1];
  assert.equal(spec.kind,'window');assert.equal(spec.key,'vega-settings');assert.equal(spec.props.globalConfig,true);
  const popup=load('vega-sidebar');popup.setProps({...spec.props,overlay:'settings'});
  let timers=0;popup.cx.timer.every=()=>timers++;
  popup.mod.init({},popup.cx);await popup.flush();
  assert.equal(timers,0);
  let tree=popup.render();
  const close=elements(tree).find(n=>n.idValue==='close-settings');
  close.handlers.on_mouse_down({},popup.cx);close.handlers.on_click({},popup.cx);
  assert(popup.calls.some(c=>c[0]==='stop'));
  assert(popup.calls.some(c=>c[0]==='close'&&c[1]==='settings'));
  tree.handlers.on_key_down({keystroke:'escape'},popup.cx);
  assert.equal(popup.calls.filter(c=>c[0]==='close').length,2);
});

test('settings lists profiles, routes Open and existing Edit tabs dialog, and preserves theme delivery status',async()=>{
  const h=load('vega-sidebar');h.setProps({globalConfig:true,overlay:'settings'});
  let error=false;
  const status='os: dark\nmode: auto (theme.toml)\napplied: dark\nclaude_panes[3]{pane,mode,wanted,status}:\n  t1,dark,dark,applied\n  t2,light,dark,pending\n  t3,unknown,dark,failed: hc_timeout\n';
  h.api.exec=async(bin,args)=>{
    h.calls.push(['exec',bin,[...args]]);
    if(error) return {code:1,stdout:'error: unavailable'};
    return {code:0,stdout:args[0]==='theme'?status:args.length===1?'profiles[2]{name,applied}:\n  status,true\n  spare,false\n':'window: status'};
  };
  h.mod.settingsTab='windows';await h.mod.readSettings('windows',h.cx);
  let nodes=elements(h.render());
  assert(texts(h.render()).includes('spare · not applied'));
  assert(!nodes.some(n=>n.idValue==='profile-open-spare'));
  nodes.find(n=>n.idValue==='profile-edit-status').handlers.on_click({},h.cx);
  const edit=h.calls.find(c=>c[0]==='open')[1];
  assert.equal(edit.mod,'vega-window-tabs');assert.equal(edit.props.mode,'config');assert.equal(edit.props.profile,'status');
  nodes.find(n=>n.idValue==='profile-open-status').handlers.on_click({},h.cx);await h.flush();
  assert(h.calls.some(c=>c[0]==='exec'&&JSON.stringify(c[2])==='["window","status"]'));
  assert(texts(h.render()).includes('Opened status'));
  nodes=elements(h.render());nodes.find(n=>n.idValue==='settings-tab-theme').handlers.on_click({},h.cx);await h.flush();
  for(const line of status.trimEnd().split('\n')) assert(texts(h.render()).includes(line));
  error=true;
  elements(h.render()).find(n=>n.idValue==='settings-refresh').handlers.on_click({},h.cx);await h.flush();
  assert(texts(h.render()).includes('error: unavailable'));
  assert(texts(h.render()).includes('  t3,unknown,dark,failed: hc_timeout'),'last successful theme snapshot survives errors');
  h.api.exec=async()=>({code:0,stdout:'profiles[0]{name,applied}:\n'});
  h.mod.settingsTab='windows';await h.mod.readSettings('windows',h.cx);
  assert(texts(h.render()).includes('No profiles available.'));
});

test('project Models edits only project pins in its recorded checkout and keeps estate data separate',async()=>{
  const h=load('vega-sidebar');h.setProps({projectConfig:'project-a',tab:'models'});
  h.state.set('vega.badges',[row()]);
  const cell={host:'puck',seat:'herdr-agents',lane:'codex',tier:'budget',allowed:['gpt-6-luna']};
  h.api.exec=async(bin,args,opts)=>{
    h.calls.push(['exec',bin,[...args],opts]);
    return {code:0,stdout:args[1]==='hosts'?JSON.stringify({rows:[cell]}):'project: project-a\nmodel: gpt-6-luna\nsource: project (proteus)\nexpires: never\n'};
  };
  await h.mod.readModels(h.cx);
  let nodes=elements(h.render());
  assert(nodes.some(n=>n.idValue==='project-tab-models'));
  assert(nodes.some(n=>n.idValue==='models-group-Project overrides'));
  assert(!nodes.some(n=>n.idValue==='models-group-Default'||n.idValue==='models-group-puck'));
  assert.equal(h.state.get('vega.models'),undefined);
  assert(h.state.has('vega.models.project-a'));
  await h.mod.changeModel(cell,false,'gpt-6-luna',h.cx);
  await h.mod.changeModel(cell,false,null,h.cx);
  const pins=h.calls.filter(c=>c[0]==='exec'&&c[2][1]==='pin');
  assert(pins.every(c=>c[3].cwd==='/repo with spaces'));
  assert(pins.every(c=>!c[2].includes('--host')&&!c[2].includes('--default')));
  assert(pins.some(c=>JSON.stringify(c[2])==='["lanista","pin","codex","--tier","budget","--set","gpt-6-luna","--until","never"]'));
  assert(pins.some(c=>JSON.stringify(c[2])==='["lanista","pin","codex","--tier","budget","--clear"]'));
  h.state.set('vega.badges',[]);await h.mod.readModels(h.cx);
  assert(texts(h.render()).includes('Project checkout unavailable.'));
});

test('context and overview read beads in the registry checkout, not the drifted terminal cwd',async()=>{
  const uac={address:'t6p',state:'live',cwd:'/other/repo',labels:{'vega.role':'uac','vega.project-id':'project-a'}};
  const h=load('vega');h.state.set('vega.badges',[{...row(),checkout:'/repo'}]);
  h.mod.ctx={project:'project-a',uac,term:uac,kind:''};
  await h.mod.readContext(h.cx);
  const bd=h.calls.find(c=>c[0]==='exec'&&c[1]==='bd');assert.equal(bd[3].cwd,'/repo');
  h.state.set('vega.badges',[]);h.calls.length=0;await h.mod.readContext(h.cx);
  assert.equal(h.calls.find(c=>c[0]==='exec'&&c[1]==='bd')[3].cwd,'/other/repo','without a badge row the terminal cwd remains');
  const s=load('vega-sidebar');s.api.terminals=()=>[uac];s.setProps({uac:'t6p',projectConfig:'project-a'});
  s.state.set('vega.badges',[{...row(),checkout:'/repo'}]);
  await s.mod.readOverview(s.cx);
  for(const c of s.calls.filter(c=>c[0]==='exec'))assert.equal(c[3].cwd,'/repo');
  assert(s.calls.some(c=>c[0]==='exec'&&c[1]==='bd'));
});

test('done is blue and kept by the Active filter only while unread (turn past herdr.seen), like the tab bar',()=>{
  const h=load('vega-sidebar'),project=projectFixture(h);
  const all=h.api.terminals();
  all.push(
    {address:'t5',state:'live',parent:'puck/t1',status:'done',turn:3,labels:{'herdr.seen':'3'}},
    {address:'t6',state:'live',parent:'puck/t1',status:'done',turn:4,labels:{'herdr.seen':'3'}},
    {address:'t8',state:'live',parent:'puck/t1',status:'done',turn:2},
  );
  h.api.terminals=()=>all;
  h.mod.init({},h.cx);h.state.set('vega.hide-inactive',true);
  const ids=elements(h.render()).map(n=>n.idValue);
  for(const id of ['sub-t6','sub-t8']) assert(ids.includes(id),`${id}: unread done stays`);
  assert(!ids.includes('sub-t5'),'seen done hides like idle');
  // A top-level agent whose done was seen reads as idle too.
  all.find(t=>t.address==='t3').turn=5;all.find(t=>t.address==='t3').labels['herdr.seen']='5';
  const agents=h.api.agents();agents[2].status='done';h.api.agents=()=>agents;
  assert(!elements(h.render()).map(n=>n.idValue).includes('ag-t3'),'seen top-level done hides');
  all.find(t=>t.address==='t3').turn=6;
  assert(elements(h.render()).map(n=>n.idValue).includes('ag-t3'),'new turn makes it unread again');
});

test('headless done is a filled grey dot in child, top-level and rail rows without changing Active or navigation',()=>{
  const h=load('vega-sidebar');projectFixture(h);
  const all=h.api.terminals(),agents=h.api.agents().filter(a=>a.address!=='t4');
  for(const address of ['t2','t3']) {
    const term=all.find(t=>t.address===address);
    term.labels['vega.mode']='headless';term.turn=4;term.labels['herdr.seen']='3';
    term.parent=address==='t2'?'puck/t1':'t4';
    agents.find(a=>a.address===address).status='done';
  }
  // An unlisted child uses its terminal status, rather than agents().
  all.push({address:'puck/t5',state:'live',parent:'puck/t1',status:'done',labels:{'vega.mode':'headless'}});
  h.api.terminals=()=>all;h.api.agents=()=>agents;
  h.api.theme=()=>({done:'blue',dim:'grey',idle:'idle-grey'});
  h.mod.init({},h.cx);h.state.set('vega.hide-inactive',true);
  function dot(tree,id) {
    const row=elements(tree).find(n=>n.idValue===id);assert(row,id);
    assert(elements(row).some(n=>n.bgValue==='grey'),`${id}: filled grey`);
    assert(!elements(row).some(n=>n.bgValue==='blue'),`${id}: no blue news`);
  }
  let tree=h.render();
  for(const id of ['sub-t2','sub-puck/t5','ag-t3'])dot(tree,id);
  assert.equal(h.mod.topStatus(agents.find(a=>a.address==='t3')),'done');
  assert.deepEqual(Array.from(h.calls.filter(c=>c[0]==='order').at(-1)[1]),['puck/t1','t2','t3']);
  h.api.layout=()=>({id:'sidebar',size:{px:76}});
  tree=h.render();for(const id of ['ag-t2','ag-puck/t5','ag-t3'])dot(tree,id);
});

test('handed-done colour requires a live parent and excludes headed sessions and UACs; other looks stay unchanged',()=>{
  const h=load('vega-sidebar'),t={done:'blue',dim:'grey',idle:'idle-grey',working:'amber',blocked:'red'};
  const parent={address:'puck/t1',state:'live'};
  const child={address:'t2',parent:parent.address,labels:{'vega.mode':'headless'}};
  h.mod.vegaTerminals=new Map([[parent.address,parent],[child.address,child]]);
  const dot=()=>h.mod.status(t,{address:child.address,status:'done'},true);
  assert.equal(dot().bgValue,'grey');
  for(const mode of ['headed',undefined]) {
    child.labels['vega.mode']=mode;assert.equal(dot().bgValue,'blue');
  }
  child.labels['vega.mode']='headless';
  for(const role of ['uac','uber']) {
    child.labels['vega.role']=role;assert.equal(dot().bgValue,'blue');
  }
  delete child.labels['vega.role'];
  parent.state='exited';assert.equal(dot().bgValue,'blue');
  h.mod.vegaTerminals.delete(parent.address);assert.equal(dot().bgValue,'blue');
  h.mod.vegaTerminals.set(parent.address,{...parent,state:'live'});
  const idle=h.mod.status(t,{address:child.address,status:'idle'},true);
  assert.equal(idle.border_colorValue,'idle-grey');assert(!Object.hasOwn(idle,'bgValue'));
  const working=h.mod.status(t,{address:child.address,status:'working'},true);
  assert.equal(working.bgValue,'amber');assert(!Object.hasOwn(working,'spinValue'));
  const blocked=h.mod.status(t,{address:child.address,status:'blocked'},true);
  assert.deepEqual(blocked.items,['!']);assert.equal(blocked.text_colorValue,'red');
});

test('profile window terminals never enter sidebar agent, plain or child rows',()=>{
  const h=load('vega-sidebar');
  h.mod.init({},h.cx);
  const regular={address:'t1',state:'live',foreground:{},labels:{'vega.project-id':'repo','vega.role':'uac'}};
  const status={address:'ganymede/t2',state:'live',parent:'t1',foreground:{},labels:{'vega.window':'status','vega.project-id':'repo'}};
  h.api.terminals=()=>[regular,status];
  h.api.agents=()=>[{address:'t1',status:'working'}, {address:'ganymede/t2',status:'working'}];
  h.render();
  assert(!h.mod.vegaTerminals.has('ganymede/t2'));
  assert(!h.mod.plainTerminals.some(t=>t.address==='ganymede/t2'));
  assert(!h.mod.listed.has('ganymede/t2'));
  assert.deepEqual(Array.from(vm.runInContext('visibleTerminals()',h.api),t=>t.address),['t1']);
  assert(!h.calls.filter(c=>c[0]==='order').flatMap(c=>c[1]).includes('ganymede/t2'));
});

test('profile tab bar clicks and keyboard navigate layout groups without touching core tabs', async () => {
  // Load the small local helper into the same fake host as the view.
  const helper = fs.readFileSync(new URL('../config/herdr-app/mods/vega-window-tabs/navigation.js', import.meta.url), 'utf8').replaceAll('export function', 'function');
  const h = load('vega-window-tabs');
  vm.runInContext(helper, h.api);
  const stack = {id:'profile:tabs', active:'proteus', children:[{id:'proteus'}, {id:'ganymede'}, {id:'tokens'}]};
  h.api.layout = () => ({id:'root', children:[stack]});
  h.api.run = (command, args) => {
    h.calls.push(['run',command,args]);
    assert.equal(command,'layout.select');
    assert.equal(args.stack,'profile:tabs');
    stack.active=args.child;
  };
  h.mod.init({},h.cx);
  const key = async command => {
    assert.equal(h.commands.length,1);
    h.commands.shift()(command);
    await new Promise(resolve => setImmediate(resolve));
  };
  await key('prev'); assert.equal(stack.active,'tokens');
  await key('next'); assert.equal(stack.active,'proteus');
  await key('select-2'); assert.equal(stack.active,'ganymede');
  await key('select-9'); assert.equal(stack.active,'ganymede');
  const bar=h.render();
  assert.equal(bar.items.length,3);
  bar.items[0].items[2].handlers.on_mouse_up({},h.cx); assert.equal(stack.active,'tokens');
  assert.equal(bar.items[0].items[2].items[0].items[0],'tokens');
  stack.children=[];
  await key('next'); assert.equal(stack.active,'tokens');
});

test('profile tabs rename inline, reorder, close and choose a machine through argv', async () => {
  const h = load('vega-window-tabs', {allowFocus:true});
  vm.runInContext(fs.readFileSync(new URL('../config/herdr-app/mods/vega-window-tabs/navigation.js', import.meta.url),'utf8').replaceAll('export function','function'), h.api);
  h.setProps({profile:'status', titles:{a:'Alpha'}});
  h.api.layout=()=>({id:'profile:tabs',active:'a',children:[{id:'a'},{id:'b'}]});
  h.mod.init({},h.cx);
  let tabs=h.render().items[0].items;
  assert.equal(tabs[0].items[0].handlers.on_click,undefined,'passive title must not create a nested click hit target');
  tabs[0].handlers.on_mouse_down({click_count:2},h.cx);
  assert.equal(h.mod.input.value(),'Alpha');
  h.mod.input.set_value('My tab');
  h.mod.input.handlers.submit({},h.cx); await h.flush();
  assert(h.calls.some(c=>c[0]==='exec' && JSON.stringify(c[2])===JSON.stringify(['window','rename','status','a','My tab'])));
  tabs=h.render().items[0].items;
  tabs[0].handlers.on_mouse_down({},h.cx);
  tabs[1].handlers.on_mouse_up({},h.cx); await h.flush();
  assert(h.calls.some(c=>c[0]==='exec' && JSON.stringify(c[2])===JSON.stringify(['window','move','status','a','2'])));
  tabs[0].items[1].handlers.on_click({},h.cx); await h.flush();
  assert(h.calls.some(c=>c[0]==='exec' && c[2][1]==='close'));
  h.render().items[1].handlers.on_click({},h.cx);
  assert.equal(h.calls.find(c=>c[0]==='open')[1].props.mode,'add');
  const picker=load('vega-window-tabs');
  picker.setProps({profile:'status',mode:'add',overlay:'picker'});
  picker.api.machines=()=>[{name:null},{name:'remote'},{name:'temporary',temporary:true}];
  picker.mod.init({},picker.cx);
  const choices=picker.render().items[1].items;
  assert.equal(choices.length,2);
  choices[1].handlers.on_click({},picker.cx); await picker.flush();
  assert(picker.calls.some(c=>c[0]==='exec' && JSON.stringify(c[2])===JSON.stringify(['window','add','status','--machine','remote'])));
  assert(picker.calls.some(c=>c[0]==='close' && c[1]==='picker'));
});

test('profile config dialog displays a diff, promotes to the build checkout (picker fallback) and reports failures', async () => {
  const h=load('vega-window-tabs');
  h.setProps({profile:'status',mode:'config',overlay:'config'});
  h.api.exec=async (_bin,args)=>{h.calls.push(['exec',args]);return {code:0,stdout:JSON.stringify({diff:'--- repo\n+++ effective\n@@ -1 +1 @@\n-title = "Old"\n+title = "New"\n'})};};
  h.api.pickFolder=async ()=>'/checkout with spaces';
  h.mod.init({},h.cx); await h.flush();
  assert.equal(h.mod.rows.length,3);
  assert.equal(h.mod.rows[1].kind,'-');
  assert.equal(h.mod.rows[2].text,'title = "New"');
  assert(elements(h.render()).some(n=>n.idValue==='changes-diff'),'stock virtualized diff viewport');
  h.render().items[2].items[0].handlers.on_click({},h.cx); await h.flush(); await h.flush();
  assert(h.calls.some(c=>JSON.stringify(c[1])===JSON.stringify(['window','promote','status'])),'saves to the build checkout first');
  assert(!h.calls.some(c=>c[1]?.includes('--repo')),'no folder picker when the checkout is known');
  const ok=h.api.exec;
  h.api.exec=async (_bin,args)=>{h.calls.push(['exec',args]);return args.includes('--repo')?{code:0,stdout:''}:{code:1,stdout:'error: no vega checkout known; pass --repo <checkout>'};};
  h.render().items[2].items[0].handlers.on_click({},h.cx); await h.flush(); await h.flush(); await h.flush();
  assert(h.calls.some(c=>JSON.stringify(c[1])===JSON.stringify(['window','promote','status','--repo','/checkout with spaces'])),'picker fallback');
  h.api.exec=ok;
  h.api.exec=async ()=>({code:1,stdout:'error: profile busy'});
  h.render().items[2].items[1].handlers.on_click({},h.cx); await h.flush();
  assert(h.mod.error.includes('profile busy'));
  assert.equal(h.mod.busy,false);
});

test('stock profile tab gestures use full order, survive renders and cancel on the bar', async () => {
  const h=load('vega-window-tabs');
  const stack={id:'profile:tabs',active:'a',children:[{id:'a'},{id:'b'},{id:'c'}]};
  h.setProps({profile:'status',order:['a','missing','b','c']});
  h.api.layout=()=>stack;
  h.mod.init({},h.cx);
  let bar=h.render();
  bar.items[0].items[0].handlers.on_mouse_down({click_count:1},h.cx);
  assert.equal(h.mod.editing,null,'single press is selection/drag, not rename');
  bar=h.render();
  bar.items[0].items[2].handlers.on_mouse_up({},h.cx);
  bar.handlers.on_mouse_up({},h.cx);
  await h.flush();
  assert(h.calls.some(c=>c[0]==='exec' && JSON.stringify(c[2])===JSON.stringify(['window','move','status','a','4'])));
  bar.items[0].items[2].handlers.on_mouse_down({},h.cx);
  bar.items[0].items[0].handlers.on_mouse_up({},h.cx); await h.flush();
  assert(h.calls.some(c=>c[0]==='exec' && JSON.stringify(c[2])===JSON.stringify(['window','move','status','c','1'])));
  const moves=h.calls.filter(c=>c[0]==='exec').length;
  bar.items[0].items[0].handlers.on_mouse_down({},h.cx);
  bar.handlers.on_mouse_up({},h.cx);
  bar.items[0].items[1].handlers.on_mouse_up({},h.cx); await h.flush();
  assert.equal(h.calls.filter(c=>c[0]==='exec').length,moves,'release on background cancels drag');
  assert.equal(h.calls.at(-1)[1],'layout.select');
  assert(h.calls.some(c=>c[0]==='scroll' && c[1]===0));
  stack.children=[];
  assert(elements(h.render()).some(n=>n.idValue==='tab-new'));
  assert(elements(h.render()).some(n=>n.idValue==='config'));
});

test('profile bar keeps stock window controls and reads program icons from slot terminals', () => {
  const h=load('vega-window-tabs');
  h.api.window=()=>({buttons:true,platform:'windows',maximized:true});
  h.api.layout=()=>({id:'profile:tabs',active:'a',children:[{id:'a',children:[{content:{terminal:'remote/t7'}}]}]});
  h.api.terminals=()=>[{address:'remote/t7',program:{name:'Shell',icon:'shell.svg',tint:true}}];
  h.mod.init({},h.cx);
  const nodes=elements(h.render());
  for(const id of ['minimize','maximize','close-window']) nodes.find(n=>n.idValue===id).handlers.on_click({},h.cx);
  assert.deepEqual(h.calls.filter(c=>c[0]==='run').map(c=>c[1]),['window.minimize','window.maximize','window.close']);
  assert.equal(nodes.find(n=>n.idValue==='tab-a').items.length,3,'icon, title, active close');
});

test('stock diff rows number hunks and config reset refreshes to a definitive empty state', async () => {
  const h=load('vega-window-tabs');
  const rows=h.api.diffRows(['@@ -3,2 +7,2 @@','-old','+new',' context','@@ -20 +30 @@','+\tvalue']);
  assert.deepEqual(Array.from(rows,r=>r.number),[undefined,3,7,8,undefined,30]);
  assert.equal(rows[5].text,'    value');
  h.setProps({profile:'status',mode:'config'});
  let changed=true;
  h.api.exec=async (_bin,args)=>{
    if(args[1]==='reset') changed=false;
    return {code:0,stdout:JSON.stringify({diff:changed?'@@ -1 +1 @@\n-old\n+new\n':''})};
  };
  h.mod.init({},h.cx); await h.flush();
  assert.equal(h.mod.rows.length,3);
  elements(h.render()).find(n=>n.idValue==='reset').handlers.on_click({},h.cx);
  await h.flush(); await h.flush();
  assert.equal(h.mod.rows.length,0);
  assert(elements(h.render()).some(n=>n.items.includes('No local changes')));
});


test('services attributes by machine and ancestry before cwd, with UAC project grouping',()=>{
  const h=load('vega-services');
  const attribute=vm.runInContext('attribute',h.api);
  const spaces=[{id:'ganymede/s1',name:'UAC'},{id:'ganymede/s2',name:'Worktree'}];
  const terms=[{address:'ganymede/t1',state:'live',pid:10,labels:{'herdr.space':'s1','vega.role':'uac','vega.project-id':'spellkave'},foreground:{cwd:'/repo'}},
    {address:'ganymede/t2',state:'live',pid:20,labels:{'herdr.space':'s2'},foreground:{cwd:'/repo/wt'}},
    {address:'t3',state:'live',pid:30,labels:{'herdr.space':'s1'}}];
  const snapshot={home:'/home/me',parents:{100:10},cwds:{100:'/repo/wt',200:'/repo/wt/src',300:'/outside'},
    listeners:[100,200,300].map(pid=>({pid,port:5187,host:'127.0.0.1',name:'node'}))};
  const rows=attribute(snapshot,'ganymede',terms,spaces,[]);
  assert.equal(rows.length,2); assert.equal(rows[0].space,'ganymede/s1');
  assert.equal(rows[0].project,'spellkave'); assert.equal(rows[1].space,'ganymede/s2');
  assert.equal(attribute(snapshot,null,terms,spaces,[]).length,0);
  terms.push({...terms[1],address:'ganymede/t4',labels:{'herdr.space':'s1'}});
  assert.equal(attribute(snapshot,'ganymede',terms,spaces,[]).length,1,'ambiguous cwd omitted');
});

test('services owns forward lifecycle and refuses puck and unknown targets',async()=>{
  const h=load('vega-services');
  // Avoid initialization reads; every host call below is fake.
  Object.assign(h.mod,{rows:[],errors:[],forwards:new Map(),pending:new Set(),owner:'test',next:0,targets:new Map([['ganymede','ganymede'],['puck','puck']]),helper:'/temp/helper.py'});
  const row={key:'service',machine:'ganymede',port:5187,host:'127.0.0.1'};
  h.api.exec=async(...args)=>{h.calls.push(args);return {code:0,stdout:'{"local":40518}'};};
  h.mod.toggle(row,h.cx);h.mod.toggle(row,h.cx);await h.flush();
  assert.equal(h.mod.forwards.get(row.key).local,40518); assert.equal(h.calls.length,1);
  assert.deepEqual(Array.from(h.calls[0][1]).slice(1),['start','test','1','ganymede','127.0.0.1','5187']);
  h.mod.toggle(row,h.cx);await h.flush();assert.equal(h.mod.forwards.size,0);
  h.mod.toggle({...row,machine:'puck'},h.cx);await h.flush();assert.equal(h.calls.length,2);
  h.mod.targets.set('ganymede','unknown');h.mod.toggle(row,h.cx);await h.flush();assert.equal(h.calls.length,2);
  await h.mod.close();assert.equal(h.calls.at(-1)[1][1],'close');
  h.mod.toggle(row,h.cx);await h.flush();assert.equal(h.calls.length,3);
});

test('registered bundle forwards all ports in one action and stops its one master',async()=>{
  const h=load('vega-services');
  Object.assign(h.mod,{rows:[],errors:[],forwards:new Map(),pending:new Set(),owner:'test',next:0,targets:new Map([['ganymede','ganymede']])});
  const row={key:'bundle',machine:'ganymede',port:5320,host:'127.0.0.1',registered:true,also_forward:[8367]};
  h.mod.call=async(action,args)=>{h.calls.push([action,Array.from(args)]);return {local:5320};};
  h.mod.toggle(row,h.cx);await h.flush();
  assert.deepEqual(h.calls[0],['start',['1','ganymede','127.0.0.1','5320','same','8367']]);
  const dependency={...row,key:'world',port:8367,also_forward:[]};
  assert.equal(h.mod.forwardFor(dependency).local,8367,'separate registered dependency shows its bundled forward');
  h.mod.toggle(dependency,h.cx);await h.flush();
  assert.deepEqual(h.calls[1],['stop',['1']]);assert.equal(h.mod.forwards.size,0);
});

test('services refresh renders fake snapshots and opens the forwarded URL',async()=>{
  const h=load('vega-services');
  let scans=0;
  h.api.appHome=()=>'/temporary-home/herdr-app';
  h.api.readTextFile=async()=> 'fake script';
  h.api.hc=async()=>({code:0,stdout:JSON.stringify({machines:[{name:'ganymede',target:'ganymede'},{name:'puck',target:'puck'},{name:'io',target:'io',via:['ssh']}]})});
  h.api.spaces=()=>[{id:'ganymede/s1',name:'Spire',machine:'ganymede'}];
  h.api.terminals=()=>[{address:'ganymede/t1',state:'live',pid:10,labels:{'herdr.space':'s1','vega.project-id':'spellkave','vega.role':'uac'}}];
  h.api.focused=()=>({terminal:'ganymede/t1',space:'ganymede/s1'});
  h.api.window=()=>({platform:'macos'});
  h.api.exec=async(...args)=>{
    h.calls.push(args);
    if(args[1][0]==='-c') { scans++; return {code:0,stdout:JSON.stringify({listeners:[{pid:100,port:5187,name:'spire',host:'127.0.0.1',scheme:'http'}],parents:{100:10},cwds:{},home:'/temporary-home'})}; }
    if(args[1][1]==='status') return {code:0,stdout:'{"alive":true}'};
    return {code:0,stdout:'{"local":40518}'};
  };
  h.mod.init({},h.cx);await h.flush();
  assert.equal(scans,1);assert.equal(h.mod.rows[0].project,'spellkave');
  assert.equal(h.mod.targets.has('io'),false,'command transport cannot forward');
  const nodes=root=>[root,...(root?.items??[]).filter(x=>x&&typeof x==='object').flatMap(nodes)];
  const row=h.mod.rows[0];
  nodes(h.render()).find(n=>n.idValue===`address-${row.key}`).handlers.on_click({},h.cx);await h.flush();
  assert.equal(h.calls.at(-1)[0],'open-url');assert.equal(h.calls.at(-1)[1],'http://127.0.0.1:40518');
  h.mod.refresh(h.cx);await h.flush();assert.equal(scans,2);assert.equal(h.mod.forwards.size,1);
  h.api.exec=async(...args)=>({code:0,stdout:args[1][0]==='-c'?'{"listeners":[],"parents":{},"cwds":{}}':'{"alive":true}'});
  h.mod.refresh(h.cx);await h.flush();assert(h.mod.rows[0].gone);h.render();
  h.api.exec=async()=>({code:0,stdout:'{"alive":false}'});
  h.api.spaces=()=>[];h.mod.refresh(h.cx);await h.flush();assert.equal(h.mod.forwards.size,0);
  h.render();
});

test('services follows project focus across machines; only a live focused uber sees all',()=>{
  const h=load('vega-services');
  const term=(address,space,project,role,parent)=>({address,state:'live',parent,labels:{'herdr.space':space,'vega.project-id':project,'vega.role':role}});
  const terms=[term('t1','s1','org/a','uac'),term('ganymede/t1','s1','org/a','uac'),
    term('t2','s2','org/b','uac'),term('t3','s1','stale','sentinel'),
    term('t4','s4',null,'sentinel','t1'),term('uber','su','org/a','uber')];
  let focus={terminal:'t1',space:'s1'};
  h.api.terminals=()=>terms;h.api.focused=()=>focus;
  Object.assign(h.mod,{errors:[],forwards:new Map(),pending:new Set(),targets:new Map(),rows:[
    {key:'a-local',project:'org/a',space:'s1',host:'127.0.0.1',port:80},
    {key:'a-remote',project:'org/a',space:'ganymede/s1',machine:'ganymede',host:'127.0.0.1',port:80},
    {key:'b',project:'org/b',space:'s2',host:'127.0.0.1',port:80},
    {key:'other',project:'Other spaces',space:'s3',host:'127.0.0.1',port:80}]});
  const nodes=root=>[root,...(root?.items??[]).filter(x=>x&&typeof x==='object').flatMap(nodes)];
  const shown=()=>nodes(h.render()).filter(n=>typeof n.idValue==='string'&&n.idValue.startsWith('address-')).map(n=>n.idValue);
  assert.deepEqual(shown(),['address-a-local','address-a-remote']);
  focus={terminal:'t2',space:'s2'};assert.deepEqual(shown(),['address-b']);
  focus={terminal:'t3',space:'s1'};assert.equal(shown().length,2,'UAC space wins stale sentinel project');
  focus={terminal:'t4',space:'s4'};assert.equal(shown().length,2,'unlabelled child inherits ancestor');
  focus={space:'ganymede/s1'};assert.equal(shown().length,2,'space context while panel has focus');
  focus=null;assert.equal(shown().length,0);
  focus={terminal:'uber'};assert.equal(shown().length,4);
  terms.at(-1).state='exited';assert.equal(shown().length,0,'stale uber cannot enable estate view');
  const selected=vm.runInContext('selectedProject',h.api);
  terms.push(term('t5','s5','else/a','uac'),{address:'legacy',state:'live',labels:{'vega.project':'a'}});
  assert.equal(selected({terminal:'legacy'},terms).project,null,'ambiguous short project stays unresolved');
  terms.at(-1).labels['vega.project']='b';
  assert.equal(selected({terminal:'legacy'},terms).project,'org/b');
  assert.equal(h.calls.length,0,'render does not execute, focus, notify or start timers');
});

test('services URL clicks await one forward, preserve HTTPS path, copy puck and non-web addresses',async()=>{
  const h=load('vega-services');
  Object.assign(h.mod,{errors:[],forwards:new Map(),pending:new Set(),next:0,targets:new Map([['ganymede','ganymede'],['puck','puck']])});
  const remote={key:'r',machine:'ganymede',host:'::1',port:8443,url:'https://[::1]:8443/path?q=1#part'};
  let finish;
  h.mod.call=async(action,args)=>{h.calls.push([action,args]);return new Promise(resolve=>{finish=resolve;});};
  h.mod.activate(remote,h.cx);h.mod.activate(remote,h.cx);
  assert.equal(h.calls.length,1);assert.equal(h.calls[0][0],'start');
  assert(!h.calls.some(c=>c[0]==='open-url'));
  finish({local:40518});await h.flush();
  assert.equal(h.calls.at(-1)[1],'https://127.0.0.1:40518/path?q=1#part');
  h.mod.activate(remote,h.cx);await h.flush();assert.equal(h.calls.filter(c=>c[0]==='start').length,1,'reuse does not stop or restart');
  const local={...remote,key:'local',machine:null,url:'http://[::1]:8443/'};
  h.mod.activate(local,h.cx);assert.deepEqual(h.calls.at(-1),['open-url',local.url]);
  for(const row of [{...remote,machine:'puck'}, {...remote,machine:'unknown'},
    {...remote,url:null,scheme:'tcp'}, {...local,url:null,address:'/tmp/service.sock'}]) {
    const before=h.calls.length;
    h.mod.activate(row,h.cx);await h.flush();
    assert.equal(h.calls.length,before+2);
    assert.equal(h.calls[before][0],'clipboard');
    assert.equal(h.calls[before+1][0],'toast');
    assert.equal(h.calls[before+1][1].timeout,1800);
  }
  h.mod.call=async()=>{throw new Error('SSH unavailable');};
  const before=h.calls.length;h.mod.activate({...remote,key:'failure'},h.cx);await h.flush();
  assert.equal(h.calls.length,before);assert.match(h.mod.errors[0],/SSH unavailable/);
  let complete;
  h.mod.call=async action=>action==='start'?new Promise(resolve=>{complete=resolve;}):{};
  h.mod.activate({...remote,key:'closing'},h.cx);await h.mod.close();complete({local:40519});await h.flush();
  assert.equal(h.calls.length,before,'closing an in-flight click never opens');
  assert.equal(h.mod.forwards.has('closing'),false);
});

test('registered 5320/8367 survive failed attribution and PID restart with correct forward state',()=>{
  const h=load('vega-services');
  const attribute=vm.runInContext('attribute',h.api);
  const snapshot={parents:{},cwds:{},listeners:[
    {pid:31560,port:5320,host:'127.0.0.1',name:'node',registered:true,title:'Classic app (local Elven Mini)',scheme:'http'},
    {pid:20931,port:8367,host:'127.0.0.1',name:'node',registered:true,title:'Classic world node',scheme:'tcp'}]};
  const rows=attribute(snapshot,'ganymede',[],[],[]);
  assert.equal(rows.length,2);
  Object.assign(h.mod,{rows,errors:[],forwards:new Map(),pending:new Set(),targets:new Map([['ganymede','ganymede']])});
  let text=JSON.stringify(h.render());
  assert(text.includes('Classic app (local Elven Mini) :5320'));
  assert(text.includes('Classic world node :8367'));
  assert(text.includes('Running · ganymede')); assert(!text.includes('Stop"'));
  h.mod.forwards.set(rows[0].key,{local:40518});
  snapshot.listeners[0].pid=999;
  h.mod.rows=attribute(snapshot,'ganymede',[],[],[]);
  assert.equal(h.mod.rows[0].key,rows[0].key);
  text=JSON.stringify(h.render());
  assert(text.includes('Forwarded · ganymede')); assert(text.includes('Local forward → ganymede:5320'));
  assert(text.includes('http://127.0.0.1:40518')); assert(!text.includes('stopped'));
  h.mod.rows[1].gone=true;
  text=JSON.stringify(h.render());
  assert(text.includes('Stopped · ganymede'));
  h.mod.forwards.clear();
  assert(!JSON.stringify(h.render()).includes('Stop"'));
});

test('services displays registered metadata and toggles hidden agent ports without rescanning',()=>{
  const h=load('vega-services');
  const attribute=vm.runInContext('attribute',h.api);
  const terms=[{address:'t1',state:'live',pid:10,labels:{'herdr.space':'s1','vega.project-id':'spellkave','vega.role':'uac'}}];
  const spaces=[{id:'s1',name:'Spire'}];
  const snapshot={home:'/temp',parents:{100:10,101:10,102:10},cwds:{},listeners:[
    {pid:100,port:5187,host:'127.0.0.1',name:'node',title:'Spire',description:'Evidence browser',url:'https://127.0.0.1:5187/path?q=a#hash',agent:true},
    {pid:101,port:5000,host:'127.0.0.1',name:'codex',agent:true},
    {pid:102,port:8080,host:'127.0.0.1',name:'vite',agent:false}]};
  Object.assign(h.mod,{rows:attribute(snapshot,null,terms,spaces,[]),errors:[],forwards:new Map(),pending:new Set(),targets:new Map(),showAgentPorts:false});
  h.api.terminals=()=>terms;
  h.api.focused=()=>({terminal:'t1',space:'s1'});
  const nodes=root=>[root,...(root?.items??[]).filter(x=>x&&typeof x==='object').flatMap(nodes)];
  let text=JSON.stringify(h.render());
  assert(text.includes('Spire :5187')); assert(text.includes('Evidence browser'));
  assert(text.includes('https://127.0.0.1:5187/path?q=a#hash'));
  assert(text.includes('vite :8080')); assert(!text.includes('codex :5000'));
  const toggle=nodes(h.render()).find(n=>n.idValue==='agent-ports');
  toggle.handlers.on_click({},h.cx);
  assert.equal(h.mod.showAgentPorts,true);
  assert(JSON.stringify(h.render()).includes('codex :5000'));
  assert.equal(h.calls.length,0,'filter changes do not scan or forward');
  toggle.handlers.on_click({},h.cx);
  assert(!JSON.stringify(h.render()).includes('codex :5000'));
});


test('services shows a Browsers group with colours, scope and a two-step close',async()=>{
  const h=load('vega-services');
  const terms=[{address:'t1',state:'live',pid:10,labels:{'herdr.space':'s1','vega.project-id':'org/a','vega.role':'uac'}},
    {address:'t2',state:'live',pid:20,labels:{'herdr.space':'s2','vega.project-id':'org/b','vega.role':'uac'}}];
  h.api.terminals=()=>terms;
  h.api.focused=()=>({terminal:'t1',space:'s1'});
  Object.assign(h.mod,{rows:[],errors:[],forwards:new Map(),pending:new Set(),targets:new Map(),
    browserConfirm:new Set(),browserClosing:new Set(),browserError:null,
    browsers:{at:Date.now()/1000,sessions:[
      {session:'mine-live',colour:'#d9465f',state:'live',pane:'t1',project:'a',headed:true,age:120},
      {session:'other-proj',colour:'#0e8f6f',state:'live',pane:'t2',project:'b',headed:false,age:120},
      {session:'no-owner',colour:'#3d73cc',state:'unowned',pane:null,project:null,headed:null,age:null}],
      loose:[{pid:900,headed:true,n:3}]}});
  const nodes=root=>[root,...(root?.items??[]).filter(x=>x&&typeof x==='object').flatMap(nodes)];
  const text=JSON.stringify(h.render());
  assert(text.includes('Browsers'));
  assert(text.includes('mine-live'));assert(text.includes('headed'));
  assert(text.includes('no-owner'));assert(text.includes('unowned browser · pid 900'));
  assert(!text.includes('other-proj'),'another project session is scoped out');
  const dot=nodes(h.render()).find(n=>n.idValue==='browser-mine-live');
  assert(dot.items[0].items[0].bgValue==='#d9465f','colour dot in the session colour');
  // Two-step close: the first click asks, the second runs vega browser close.
  const close=nodes(h.render()).find(n=>n.idValue==='browser-close-mine-live');
  close.handlers.on_click({},h.cx);
  assert.equal(h.calls.length,0,'no exec before the confirm');
  assert(JSON.stringify(h.render()).includes('Close session?'));
  h.api.exec=async(...args)=>{h.calls.push(args);return {code:0,stdout:''};};
  const confirm=nodes(h.render()).find(n=>n.idValue==='browser-close-mine-live');
  confirm.handlers.on_click({},h.cx);await h.flush();
  assert(h.calls.some(c=>c[1]?.[0]==='browser'&&c[1]?.[1]==='close'&&c[1]?.[2]==='mine-live'),'runs vega browser close after the confirm');
  assert(!h.mod.browserConfirm.has('mine-live'),'confirm resets after the action');
});

test('services refresh reads vega browser ls once and publishes the badge payload',async()=>{
  const h=load('vega-services');
  h.api.appHome=()=>'/temporary-home/herdr-app';
  h.api.readTextFile=async()=> 'fake script';
  h.api.hc=async()=>({code:0,stdout:JSON.stringify({machines:[]})});
  h.api.spaces=()=>[];h.api.terminals=()=>[];h.api.focused=()=>null;
  let browserReads=0;
  h.api.exec=async(...args)=>{
    h.calls.push(args);
    if(args[1][0]==='browser') {
      browserReads++;
      return {code:0,stdout:JSON.stringify({schema:1,error:null,
        sessions:[{session:'s1',state:'live',colour:'#d9465f',pane:'t7',project:'vega',headed:true,pid:11,age:5,idle:5},
          {session:'s2',state:'closed',colour:'#0e8f6f',pane:null,project:null,headed:null,pid:null,age:5,idle:5}],
        browsers:[{pid:11,kind:'chrome-for-testing',headed:true,age:5,port:0,user_data_dir:'/x/s1-profile',global:false},
          {pid:12,kind:'chrome-for-testing',headed:true,age:5,port:0,user_data_dir:'/x/s1-profile',global:false},
          {pid:20,kind:'chromium',headed:true,age:5,port:9223,user_data_dir:'/g',global:true},
          {pid:30,kind:'chrome-for-testing',headed:false,age:5,port:0,user_data_dir:'/tmp/uuid',global:false}]})};
    }
    return {code:0,stdout:JSON.stringify({listeners:[],parents:{},cwds:{},home:'/temporary-home'})};
  };
  h.mod.init({},h.cx);await h.flush();
  assert.equal(browserReads,1,'one browser read per refresh');
  const shared=h.state.get('vega.browsers');
  assert.equal(JSON.stringify(shared.sessions),JSON.stringify(
    [{session:'s1',colour:'#d9465f',state:'live',pane:'t7',project:'vega',headed:true,age:5}]));
  assert.equal(JSON.stringify(shared.loose),JSON.stringify([{pid:30,headed:false,n:1}]),'global and session-owned browsers are not loose');
  // A failing read keeps the panel alive with an inline error.
  h.api.exec=async(...args)=>{
    if(args[1][0]==='browser') return {code:1,stderr:'vega browser ls failed'};
    return {code:0,stdout:JSON.stringify({listeners:[],parents:{},cwds:{},home:'/h'})};
  };
  h.mod.refresh(h.cx);await h.flush();
  assert.match(h.mod.browserError,/vega browser ls failed/);
  assert(JSON.stringify(h.render()).includes('vega browser ls failed'));
});

test('keychain indicator includes agent count, fix hint and expires agent claims',async()=>{
  const h=load('vega'),now=Date.now()/1000;
  h.mod.localKeychainHost='localmac';
  const sample={host:'localmac',state:'unlocked',since:now-30,at:now,agent:{state:'ready',keys:3}};
  h.mod.applyKeychains([sample],now,h.cx);
  let lock=elements(h.render()).find(n=>n.idValue==='keychain-localmac');
  assert(lock.tooltipValue.includes('ready · 3 keys'));
  assert(texts(lock).includes(' 3'));
  h.mod.applyKeychains([{...sample,agent:{state:'empty',keys:0}}],now,h.cx);
  lock=elements(h.render()).find(n=>n.idValue==='keychain-localmac');
  assert(lock.tooltipValue.includes('empty · 0 keys'));
  assert(lock.tooltipValue.includes('Terminal.app'));
  assert(lock.tooltipValue.includes('--apple-use-keychain'));
  h.mod.applyKeychains([sample],now-181,h.cx);
  lock=elements(h.render()).find(n=>n.idValue==='keychain-localmac');
  assert(lock.tooltipValue.includes('agent.sock: unknown'));
  assert(!lock.tooltipValue.includes('3 keys'));
});

test('registered services retain project ownership across fleet scans and legacy entries degrade',()=>{
  const h=load('vega-services'), attribute=vm.runInContext('attribute',h.api);
  const terms=['a','b'].map((p,i)=>({address:`ganymede/t${i}`,state:'live',pid:10+i,
    labels:{'herdr.space':p,'vega.project-id':`git:example/${p}`},foreground:{cwd:`/repo/${p}`}}));
  const spaces=['a','b'].map(p=>({id:`ganymede/${p}`,name:p}));
  const listeners=[
    {pid:100,project:'git:example/a',title:'Explicit A'},
    {pid:101,cwd:'/repo/a/src',title:'Legacy cwd A'},
    {pid:102,title:'Legacy pid B'},
    {pid:103,title:'Unknown global'},
    {pid:104,project:'git:example/absent',title:'Absent project'},
  ].map((r,i)=>({...r,registered:true,port:5300+i,host:'127.0.0.1',name:'node'}));
  const snapshot={parents:{100:11,102:11},cwds:{},listeners};
  const rows=attribute(snapshot,'ganymede',terms,spaces,[]);
  assert.equal(rows[0].project,'git:example/a','explicit owner overrides foreign ancestry');
  assert.equal(rows[0].space,'');
  assert.equal(rows[1].project,'git:example/a');assert.equal(rows[2].project,'git:example/b');
  assert.equal(rows[3].global,true);assert.equal(rows[4].global,false);
  Object.assign(h.mod,{rows,errors:[],forwards:new Map(),pending:new Set(),targets:new Map()});
  h.api.terminals=()=>terms;h.api.focused=()=>({terminal:'ganymede/t1'});
  let text=JSON.stringify(h.render());
  assert(text.includes('Legacy pid B'));assert(text.includes('Global services (owner unknown)'));
  for(const title of ['Explicit A','Legacy cwd A','Absent project']) assert(!text.includes(title),title);
  h.mod.showAgentPorts=true;assert(!JSON.stringify(h.render()).includes('Explicit A'));
  h.api.focused=()=>({terminal:'ganymede/t0'});text=JSON.stringify(h.render());
  assert(text.includes('Explicit A'));assert(text.includes('Legacy cwd A'));assert(!text.includes('Legacy pid B'));
  assert.equal(h.calls.length,0);
});

test('browser listeners and daemon descendants never become services',()=>{
  const h=load('vega-services'), attribute=vm.runInContext('attribute',h.api);
  const processes={100:'/opt/agent-browser-darwin-arm64 --daemon',101:'/opt/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing --remote-debugging-port=0',102:'node node /lib/agent-browser/dist/daemon.js',103:'worker',104:'node node /repo/server.js --title chrome',105:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',106:'/usr/bin/chromium --remote-debugging-port=0'};
  const snapshot={parents:{100:10,101:10,102:10,103:100,104:10,105:10,106:10},cwds:{},processes,
    listeners:Object.keys(processes).map(pid=>({pid:Number(pid),port:60000+Number(pid),name:'node',host:'127.0.0.1',registered:true,title:'Registered'}))};
  const terms=[{address:'t1',state:'live',pid:10,labels:{'herdr.space':'s','vega.project-id':'a'}}];
  const rows=attribute(snapshot,null,terms,[{id:'s',name:'A'}],[]);
  assert.deepEqual(Array.from(rows,r=>r.pid),[104]);
});
