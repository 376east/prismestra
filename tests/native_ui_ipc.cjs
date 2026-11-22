const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const source=fs.readFileSync(path.join(__dirname,'..','src','ui','windows','web','app.js'),'utf8');
const outputMapSource=fs.readFileSync(path.join(__dirname,'..','src','ui','windows','web','output-map.js'),'utf8');
const nodes=new Map(),commands=[],events={},rafQueue=[];
class Node {constructor(){this.tagName='INPUT';this.validity={valid:true};this.dataset={};this.width=104;this.height=72;this.value='';this.disabled=false;this.innerHTML='';this.textContent='';}setAttribute(){}focus(){document.activeElement=this;}select(){}hasAttribute(name){return name==='data-target'&&this.dataset.target!==undefined;}before(){}append(){}addEventListener(){}setCustomValidity(){}reportValidity(){}showModal(){this.open=true;}close(){this.open=false;}getContext(){return {fillRect(){},clearRect(){},createImageData:(w,h)=>({data:new Uint8ClampedArray(w*h*4)}),putImageData:pixels=>{this.lastPixels=pixels;}};}}
const find=s=>{if(!nodes.has(s))nodes.set(s,new Node());return nodes.get(s);};
const main=find('#main');let mainMarkup='';Object.defineProperty(main,'innerHTML',{get:()=>mainMarkup,set:markup=>{mainMarkup=markup;for(const match of markup.matchAll(/<input id="(field-\d+)" data-field="([^"]*)" value="([^"]*)"/g)){const field=new Node();field.id=match[1];field.dataset.field=match[2];field.value=match[3];nodes.set('#'+match[1],field);}for(const match of markup.matchAll(/<input id="(native-field-\d+)" data-native-field="([^"]*)" data-native-kind="([^"]*)"[^>]*type="([^"]*)"[^>]*value="([^"]*)"/g)){const field=new Node();field.id=match[1];field.dataset.nativeField=match[2];field.dataset.nativeKind=match[3];field.type=match[4];field.value=match[5];nodes.set('#'+match[1],field);}if(markup.includes('data-local-discard')){const button=new Node();button.dataset.action='discard-page-fields';button.dataset.localDiscard='';nodes.set('local-discard',button);}for(const match of markup.matchAll(/<canvas data-library-preview="([^"]*)"/g)){const canvas=new Node();canvas.dataset.libraryPreview=match[1];nodes.set('library-canvas-'+match[1],canvas);}}});
const document={querySelector:find,querySelectorAll:selector=>[...nodes.values()].filter(n=>selector==='[data-output-retry]'?n.dataset.outputRetry!==undefined:selector==='[data-local-discard]'?n.dataset.localDiscard!==undefined:selector==='[data-library-preview]'?n.dataset.libraryPreview!==undefined:selector==='[data-native-kind="secret"]'?n.dataset.nativeKind==='secret':selector==='[data-native-control]'?(n.dataset.nativeField!==undefined||n.dataset.nativeAction!==undefined):n.dataset.action),getElementById:id=>find('#'+id),createElement:()=>new Node(),body:new Node(),activeElement:null,addEventListener:(name,fn)=>events[name]=fn};
const context=vm.createContext({document,window:{addEventListener:(name,fn)=>events[name]=fn,ipc:{postMessage:v=>commands.push(JSON.parse(v))}},requestAnimationFrame:fn=>rafQueue.push(fn),CSS:{escape:s=>s},structuredClone,setTimeout:()=>1,clearTimeout(){},console});
vm.runInContext(source,context);
assert.equal(commands[0].op,'snapshot');
const exportMarkup=vm.runInContext('projectActions.innerHTML',context);
assert.match(exportMarkup,/<button[^>]*data-action="export"[^>]*aria-describedby="profile-export-scope">Экспорт профиля<\/button>/);
assert.match(exportMarkup,/<span id="profile-export-scope"[^>]*>[^<]*«Мои сцены» хранятся отдельно\.<\/span>/);
const scene={effect:'noise_map',colors:[0xff0000,0x00ff00],brightness:80,speed:30,fps:48,temperature:0,tint:0,noise:{frequency:.055,amplitude:1,lacunarity:.75,persistence:.5,octaves:2,motion:1,motion_speed:1},noise_palette:'custom',rainbow_width:10};
const initial={scene, draftScene:{...scene,brightness:65},library:[{id:'saved',name:'Живая сцена',scene}],devices:[],groups:{pc:true,door:false},native:true,pending:false,dirty:false,status:'Готово',error:null,profilePath:'C:\\sample.json',editor:null};
context.payload={kind:'snapshot',state:initial};vm.runInContext('window.prismestraReceive(payload)',context);
const navigationIcons=vm.runInContext('Object.keys(pages).map(key => glyphs[key])',context);
assert.ok(navigationIcons.every(Boolean),'each navigation item has its own glyph definition');
assert.equal(new Set(navigationIcons).size,navigationIcons.length,'navigation glyphs are distinct');
assert.match(find('nav').innerHTML,/aria-label="Сцены"[^>]*aria-current="page"/);
assert.equal(vm.runInContext('draft.brightness',context),65,'restores backend draft once');
vm.runInContext('draft.noise.frequency=.123;draft.brightness=55',context);
context.payload={kind:'snapshot',state:structuredClone(initial)};vm.runInContext('window.prismestraReceive(payload)',context);
assert.equal(vm.runInContext('draft.brightness',context),55,'snapshot preserves user draft');
context.payload={kind:'status',state:{scene:{...scene,brightness:90},groups:{pc:true,door:true},native:true,pending:false,status:'Применено',error:null,preview:{width:2,height:1,colors:[0xff0000,0x00ff00]}}};vm.runInContext('window.prismestraReceive(payload)',context);
assert.equal(vm.runInContext('draft.brightness',context),55,'status preserves user draft');
assert.equal(vm.runInContext('draft.noise.frequency',context),.123,'unmodified nested fields survive');
vm.runInContext("changeView('editor')",context);assert.ok(nodes.get('#main').innerHTML.includes('data-param="noise.frequency"'));
vm.runInContext("send({op:'apply_scene',scene:clone(draft)},true)",context);assert.equal(commands.at(-1).op,'apply_scene');assert.equal(commands.at(-1).scene.brightness,55);assert.equal(commands.at(-1).scene.noise.frequency,.123);
vm.runInContext("send({op:'delete_scene',id:'saved'},true)",context);assert.equal(commands.at(-1).op,'apply_scene','pending blocks mutations');
context.payload={kind:'error',message:'Проверочная ошибка'};vm.runInContext('window.prismestraReceive(payload)',context);assert.equal(nodes.get('#native-error').hidden,false);assert.ok(nodes.get('#native-error').innerHTML.includes('Проверочная ошибка'));
assert.ok(!/localStorage|fetch\(|XMLHttpRequest|WebSocket/.test(source),'project has no browser storage or networking');

// Recovery operations remain usable while the provider reports pending/fault.
vm.runInContext('state.pending=true;state.error="Provider fault";state.editor={page:"devices",index:0,targets:["Device"],fields:[],dirty:false};commandPending=true',context);
for(const [action,op] of [['revert-document','revert_document'],['retry-document','retry_document'],['classic','classic']]){
 const control=new Node();control.dataset.action=action;context.control=control;assert.equal(vm.runInContext('controlDisabled(control)',context),false,action+' remains enabled');
 context.op=op;vm.runInContext('send({op,page:"devices"},true)',context);assert.equal(commands.at(-1).op,op);
}
// A pending provider must not discard the latest debounced preview.
vm.runInContext('draft.brightness=37;requestPreview();flushPreview()',context);assert.equal(commands.at(-1).op,'preview_scene');assert.equal(commands.at(-1).scene.brightness,37);
vm.runInContext('state.pending=false;commandPending=false;draft.brightness=41;requestPreview();send({op:"save"},true)',context);assert.equal(commands.at(-2).op,'preview_scene');assert.equal(commands.at(-2).scene.brightness,41);assert.equal(commands.at(-1).op,'save');
vm.runInContext('draft.brightness=42;requestPreview()',context);events.pagehide();assert.equal(commands.at(-1).scene.brightness,42);assert.equal(commands.at(-1).op,'preview_scene');
// Full snapshots refresh devices/forms while preserving a valid focused edit on the same target.
context.payload={kind:'snapshot',state:{...structuredClone(initial),editor:{page:'devices',index:0,targets:['Device'],fields:[{key:'label',label:'Name',value:'Server',choices:[],hint:'',file:false}],dirty:true}}};
vm.runInContext('view="devices";window.prismestraReceive(payload)',context);
const field=find('#field-0');field.id='field-0';field.dataset.field='label';field.value='Still typing';field.selectionStart=3;field.selectionEnd=3;document.activeElement=field;events.input({target:field});
context.payload.state.devices=[{id:'one',label:'Updated device',role:'canvas',driver:{type:'stub'},leds:10,rate_hz:48,enabled:true}];
vm.runInContext('window.prismestraReceive(payload)',context);assert.notEqual(find('#field-0'),field,'snapshot replaces form nodes');assert.equal(find('#field-0').value,'Still typing');assert.ok(find('#main').innerHTML.includes('Updated device'));
document.activeElement=find('#field-0');
vm.runInContext('formStages.clear();send({op:"revert_document"},true);window.prismestraReceive(payload)',context);assert.equal(find('#field-0').value,'Server','explicit revert accepts backend field value');
console.log('PASS: fault recovery, latest preview during pending, synchronous flush before save/unload, focused edit plus refreshed device snapshot');
console.log('PASS: snapshot IPC, draft restore/preservation, native preview painting, full-scene command, pending guard, persistent error, no project storage/network');

// Closing captures the newest draft before debounce and locks until explicit cancellation/error.
vm.runInContext('state.pending=false;commandPending=false;draft.brightness=73;requestPreview();window.prismestraReceive({kind:"flush",id:91})',context);
assert.equal(commands.at(-1).op,'close_ready');assert.equal(commands.at(-1).id,91);assert.equal(commands.at(-1).scene.brightness,73);assert.equal(vm.runInContext('previewTimer',context),null);assert.equal(vm.runInContext('busy()',context),true);
for(const action of ['revert-document','retry-document','classic']){context.control={dataset:{action}};assert.equal(vm.runInContext('controlDisabled(control)',context),true,'closing also freezes recovery controls');}
const closeCommandCount=commands.length;vm.runInContext('send({op:"revert_document"},true);flushPreview(true)',context);assert.equal(commands.length,closeCommandCount,'no IPC edits after close_ready');
context.payload={kind:'snapshot',state:{...structuredClone(initial),pending:false}};vm.runInContext('window.prismestraReceive(payload)',context);assert.equal(vm.runInContext('closing',context),true,'snapshot cannot unlock closing');
vm.runInContext('window.prismestraReceive({kind:"status",state:{pending:false}})',context);assert.equal(vm.runInContext('closing',context),true,'status cannot unlock closing');
vm.runInContext('window.prismestraReceive({kind:"close_cancelled"})',context);assert.equal(vm.runInContext('busy()',context),false,'cancel unlocks editing');
vm.runInContext('draft.brightness=74;requestPreview();window.prismestraReceive({kind:"flush",id:92});window.prismestraReceive({kind:"error",message:"Close failed"})',context);assert.equal(vm.runInContext('closing',context),false,'error unlocks closing');assert.equal(vm.runInContext('busy()',context),false);
assert.equal(commands.at(-1).scene.brightness,74,'second flush uses latest scene');
console.log('PASS: close handshake flushes latest draft, cancels debounce, locks mutations, ignores ordinary unlocks, explicit cancel/error unlocks');

// Advanced forms render backend metadata, keep secrets blank, and dispatch actual ID types.
const form={page:'connections',fields:[{id:101,label:'Server',kind:'text',value:'Backend',choices:[],enabled:true},{id:102,label:'Token',kind:'secret',value:'NEVER_ECHO_THIS',choices:[],enabled:true},{id:'mode',label:'Mode',kind:'select',value:'second',choices:[{label:'First',value:'first'},{label:'Second',value:'second'}],enabled:true},{id:'enabled',label:'Enabled',kind:'checkbox',value:'true',choices:[],enabled:true}],actions:[{id:201,label:'Reconnect',enabled:true},{id:202,label:'Unavailable',enabled:false}],notes:['Backend real status']};
context.payload={kind:'snapshot',state:{...structuredClone(initial),pending:true,nativeForm:form}};
vm.runInContext('view="connections";window.prismestraReceive(payload)',context);
assert.ok(!find('#main').innerHTML.includes('NEVER_ECHO_THIS'));assert.ok(find('#main').innerHTML.includes('type="password" autocomplete="off" value=""'));assert.ok(find('#main').innerHTML.includes('Backend real status'));assert.ok(!find('#main').innerHTML.includes('Расширенные настройки'));
const nativeField=find('#native-field-0');nativeField.value='Typing';nativeField.selectionStart=2;nativeField.selectionEnd=2;document.activeElement=nativeField;events.input({target:nativeField});
context.payload={kind:'status',state:{pending:true,nativeForm:{...structuredClone(form),notes:['Updated real status']}}};
vm.runInContext('window.prismestraReceive(payload)',context);assert.notEqual(find('#native-field-0'),nativeField);assert.equal(find('#native-field-0').value,'Typing');assert.ok(find('#main').innerHTML.includes('Updated real status'));
const beforeTextBlur=commands.length;events.change({target:find('#native-field-0')});assert.equal(commands.length,beforeTextBlur,'text blur is local only');vm.runInContext('send({op:"native_action",id:201},true)',context);assert.equal(commands.at(-1).op,'native_action');assert.equal(commands.at(-1).fields[0].id,101);assert.equal(commands.at(-1).fields[0].value,'Typing');
vm.runInContext('window.prismestraReceive({kind:"status",state:{pending:false}})',context);assert.equal(vm.runInContext('commandPending',context),true,'periodic status cannot acknowledge native action');
const beforeBlockedAction=commands.length;vm.runInContext('send({op:"native_action",id:"201"},true)',context);assert.equal(commands.length,beforeBlockedAction,'native commands serialize until snapshot');
context.payload={kind:'snapshot',state:{...structuredClone(initial),pending:true,nativeForm:structuredClone(form)}};vm.runInContext('window.prismestraReceive(payload)',context);
const button={dataset:{nativeAction:'201'},disabled:false,hasAttribute:()=>false};events.click({target:{closest:()=>button}});assert.equal(commands.at(-1).op,'native_action');assert.equal(commands.at(-1).id,201,'numeric backend id stays numeric despite DOM dataset');
vm.runInContext('window.prismestraReceive(payload)',context);const blockedCount=commands.length;vm.runInContext('send({op:"native_action",id:"202"},true)',context);assert.equal(commands.length,blockedCount,'disabled metadata action stays blocked');
const secret=find('#native-field-1');secret.value='typed-private-token';const beforeSecretBlur=commands.length;events.input({target:secret});events.change({target:secret});assert.equal(commands.length,beforeSecretBlur);vm.runInContext('send({op:"native_action",id:201},true)',context);assert.equal(commands.at(-1).op,'native_action');assert.equal(commands.at(-1).fields[0].id,102);assert.equal(commands.at(-1).fields[0].value,'typed-private-token');assert.equal(secret.value,'','secret cleared after dispatch');
console.log('PASS: metadata forms, no secret echo, typed secrets dispatched once, async notes/focused edit preservation, authoritative enabled and numeric IDs, action serialization');

// Page RPC rejection rolls back, while dirty generic forms cannot silently lose context.
context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeForm:structuredClone(form)}};vm.runInContext('window.prismestraReceive(payload);view="connections";render();changeView("sources")',context);assert.equal(vm.runInContext('view',context),'sources');assert.equal(vm.runInContext('pendingView.previous',context),'connections');
vm.runInContext('window.prismestraReceive({kind:"error",message:"Save connection first"})',context);assert.equal(vm.runInContext('view',context),'connections');assert.equal(vm.runInContext('pendingView',context),null);assert.ok(find('#main').innerHTML.includes('Backend real status'));
vm.runInContext('changeView("schedule");window.prismestraReceive(payload)',context);assert.equal(vm.runInContext('pendingView',context),null,'snapshot acknowledges requested view');
vm.runInContext('view="devices";state.editor={page:"devices",index:0,targets:["Device"],fields:[],dirty:true};render()',context);const beforeDirtyNavigation=commands.length;vm.runInContext('changeView("sources")',context);assert.equal(vm.runInContext('view',context),'devices');assert.equal(commands.length,beforeDirtyNavigation);assert.ok(find('#native-error').innerHTML.includes('Примените или отмените'));
// Additional controls use details, keeping basic controls visible and open state across render.
vm.runInContext('state.editor.dirty=false;view="editor";advancedOpen=false;render()',context);let markup=find('#main').innerHTML;assert.ok(markup.indexOf('data-param="brightness"')<markup.indexOf('<details'));assert.ok(markup.indexOf('data-param="speed"')<markup.indexOf('<details'));assert.ok(markup.indexOf('data-param="fps"')>markup.indexOf('<details'));assert.ok(markup.includes('Основной свет'));assert.ok(markup.includes('Дополнительный свет'));assert.ok(!markup.includes('Дверная лента'));events.toggle({target:{id:'advanced-scene',open:true}});vm.runInContext('render()',context);assert.ok(find('#main').innerHTML.includes('<details id="advanced-scene" open>'));
// Palette structure edits reuse scene preview and leave the authoritative profile alone.
const paletteScene={...structuredClone(scene),colors:[0x081426,0x154760,0x2db6ae,0xe3b769,0xf2d9a2]};
context.paletteScene=paletteScene;
vm.runInContext('draft=clone(paletteScene);palettePosition=0;render()',context);
const paletteAction=action=>events.click({target:{closest:()=>({dataset:{action},disabled:false,hasAttribute:name=>name==='data-mutation'})}});
const paletteControl=action=>{context.paletteControl={dataset:{action}};return vm.runInContext('controlDisabled(paletteControl)',context);};
const beforePalette=commands.length;
assert.ok(find('#main').innerHTML.includes('id="palette-position"'));
assert.equal(paletteControl('move-palette-earlier'),true);
events.change({target:Object.assign(new Node(),{id:'palette-position',value:'1',dataset:{}})});
paletteAction('insert-palette-color');
vm.runInContext('flushPreview()',context);
assert.deepEqual(commands.at(-1).scene,{...paletteScene,colors:[0x081426,0x154760,0x154760,0x2db6ae,0xe3b769,0xf2d9a2]});
assert.equal(vm.runInContext('palettePosition',context),2);
events.change({target:Object.assign(new Node(),{id:'palette-position',value:'3',dataset:{}})});
paletteAction('move-palette-earlier');
vm.runInContext('flushPreview()',context);
assert.deepEqual(commands.at(-1).scene,{...paletteScene,colors:[0x081426,0x154760,0x2db6ae,0x154760,0xe3b769,0xf2d9a2]});
assert.equal(vm.runInContext('palettePosition',context),2);
assert.ok(commands.slice(beforePalette).every(command=>command.op==='preview_scene'));
assert.deepEqual(JSON.parse(vm.runInContext('JSON.stringify(state.scene)',context)),scene);
assert.deepEqual(JSON.parse(vm.runInContext('JSON.stringify(state.library[0].scene)',context)),scene);
vm.runInContext('send({op:"save_scene",name:"Palette copy",scene:clone(draft)},true)',context);
assert.equal(commands.at(-1).op,'save_scene');
assert.equal(commands.at(-1).id,undefined,'copy has no existing catalog ID');
assert.deepEqual(commands.at(-1).scene,{...paletteScene,colors:[0x081426,0x154760,0x2db6ae,0x154760,0xe3b769,0xf2d9a2]});
vm.runInContext('commandPending=false;awaitingCommandAck=false',context);
vm.runInContext('draft.colors=Array.from({length:8},(_,index)=>index);palettePosition=7;render()',context);
assert.equal(paletteControl('insert-palette-color'),true);
assert.equal(paletteControl('move-palette-later'),true);
paletteAction('insert-palette-color');paletteAction('move-palette-later');
assert.equal(vm.runInContext('draft.colors.length',context),8);
vm.runInContext('draft=clone(paletteScene);draft.colors=[0x081426];render()',context);
assert.equal(vm.runInContext('palettePosition',context),0,'position clamps when opening a shorter palette');
assert.equal(paletteControl('move-palette-earlier'),true);assert.equal(paletteControl('move-palette-later'),true);
events.change({target:Object.assign(new Node(),{id:'noise-palette',dataset:{sceneSelect:'noise_palette'},value:'rainbow'})});
assert.ok(!find('#main').innerHTML.includes('id="palette-position"'));
paletteAction('insert-palette-color');assert.equal(vm.runInContext('draft.colors.length',context),1);
events.change({target:Object.assign(new Node(),{id:'noise-palette',dataset:{sceneSelect:'noise_palette'},value:'custom'})});
assert.ok(find('#main').innerHTML.includes('id="palette-position"'));
vm.runInContext('draft.effect="rainbow_wave";render()',context);
assert.ok(!find('#main').innerHTML.includes('id="palette-position"'));
vm.runInContext('draft=clone(paletteScene);draft.effect="breathing";state.pending=true;render()',context);
assert.equal(paletteControl('insert-palette-color'),true);
paletteAction('insert-palette-color');assert.equal(vm.runInContext('draft.colors.length',context),5);
vm.runInContext('state.pending=false;draft=null;previewQueued=false',context);
console.log('PASS: palette insertion and reorder preview ordered RGB, scene untouched, palette mode and bounds respected');

// Built-in templates are available beside existing entries and edit as unsaved copies.
const plumDawn={id:'builtin:plum-dawn',name:'Сливовый рассвет',builtIn:true,scene:{...structuredClone(scene),speed:24,colors:[0x243467,0x7452a4,0xcc9dd5,0xfdb87d]}};
context.payload={kind:'snapshot',state:{...structuredClone(initial),draftScene:null,draftOrigin:null,library:[plumDawn,...structuredClone(initial.library)]}};
vm.runInContext('draft=null;view="library";window.prismestraReceive(payload)',context);
let builtInMarkup=find('#main').innerHTML;
assert.ok(builtInMarkup.includes('Сливовый рассвет'));
assert.ok(builtInMarkup.includes('Шаблон'));
assert.ok(!builtInMarkup.includes('data-remove="builtin:plum-dawn"'));
assert.ok(builtInMarkup.includes('data-remove="saved"'));
const beforeBuiltIn=commands.length;
const presetClick=dataset=>events.click({target:{closest:()=>({dataset,disabled:false,hasAttribute:()=>true})}});
presetClick({edit:plumDawn.id});
assert.equal(commands.at(-1).op,'preview_scene');
assert.equal(commands.at(-1).origin.id,null);
assert.deepEqual(commands.at(-1).scene,plumDawn.scene);
assert.ok(commands.slice(beforeBuiltIn).every(command=>command.op==='preview_scene'));
assert.ok(!find('#main').innerHTML.includes('data-action="rename"'));
vm.runInContext('draft.speed=35;requestPreview();flushPreview();saveDraft("My plum scene")',context);
assert.equal(commands.at(-1).op,'save_scene');
assert.equal(commands.at(-1).id,null,'saving a template creates a personal entry');
assert.equal(commands.at(-1).scene.speed,35);
assert.equal(vm.runInContext('state.library[0].scene.speed',context),24);
assert.deepEqual(JSON.parse(vm.runInContext('JSON.stringify(state.library[1].scene)',context)),scene);
vm.runInContext('commandPending=false;awaitingCommandAck=false;savedSceneRequest=null;draft=null;draftOrigin=null;selectedId=null',context);
console.log('PASS: built-in template preview and copy leave the preset and the legacy catalog entry untouched');

// Native library frames go through the same pixel painter; missing frames keep a palette fallback.
context.payload={kind:'snapshot',state:{...structuredClone(initial),library:[{id:'saved',name:'Native frame',scene,preview:{width:2,height:1,colors:[0xff0000,0x00ff00]}},{id:'fallback',name:'No cached frame',scene}]}};vm.runInContext('view="library";window.prismestraReceive(payload)',context);
assert.ok(find('#main').innerHTML.includes('data-library-preview="saved"'));assert.ok(find('#main').innerHTML.includes('Палитра сцены'));const artwork=find('library-canvas-saved');assert.equal(artwork.width,2);assert.equal(artwork.height,1);assert.deepEqual([...artwork.lastPixels.data],[255,0,0,255,0,255,0,255]);assert.ok(find('#main').innerHTML.includes('параметры RGB'));
console.log('PASS: page rollback/ack and dirty guard, universal RGB/profile copy, basic/advanced controls with retained details, native library preview pixels');
// Text staging prevents blur RPC/first-click loss; invalid edits survive full snapshots and errors.
const editorState={...structuredClone(initial),editor:{page:'devices',index:0,targets:['One','Two'],fields:[{key:'port',label:'Port',value:'8080',choices:[],hint:'',file:false},{key:'name',label:'Name',value:'Device',choices:[],hint:'',file:false},{key:'mode',label:'Mode',value:'a',choices:[{label:'A',value:'a'},{label:'B',value:'b'}],hint:'',file:false}],dirty:false},nativeAux:{page:'devices',fields:[],actions:[{id:410,label:'Add device',enabled:true}],notes:['Aux state']}};
context.payload={kind:'snapshot',state:structuredClone(editorState)};vm.runInContext('formStages.clear();view="devices";window.prismestraReceive(payload)',context);
const invalidPort=find('#field-0');invalidPort.value='not-a-number';invalidPort.validity={valid:false};const stagedName=find('#field-1');stagedName.value='Unfocused text';
const beforeStaging=commands.length;events.input({target:invalidPort});events.input({target:stagedName});events.change({target:invalidPort});assert.equal(commands.length,beforeStaging,'text input and blur never post RPC');assert.equal(vm.runInContext('currentGenericStages().length',context),2);
const target=new Node();target.dataset.target='';target.value='1';events.change({target});assert.equal(target.value,'0','target switch restores selected target while local text is dirty');assert.equal(commands.length,beforeStaging);
events.pointerdown({});const originalMarkup=find('#main').innerHTML;context.metadata={kind:'status',state:{nativeAux:{...structuredClone(editorState.nativeAux),notes:['Background update']}}};vm.runInContext('window.prismestraReceive(metadata)',context);assert.equal(find('#main').innerHTML,originalMarkup,'background full render deferred during pointer gesture');assert.equal(vm.runInContext('renderQueued',context),true);
events.pointerup({});const applyButton={dataset:{action:'apply-document'},disabled:false,hasAttribute:()=>true};events.click({target:{closest:()=>applyButton}});assert.equal(commands.length,beforeStaging+1,'first click sends exactly one batch');assert.equal(commands.at(-1).op,'apply_fields');assert.deepEqual(commands.at(-1).fields,[{key:'port',value:'not-a-number'},{key:'name',value:'Unfocused text'}]);while(rafQueue.length)rafQueue.shift()();
vm.runInContext('window.prismestraReceive({kind:"error",message:"Invalid port"});window.prismestraReceive(payload)',context);assert.equal(find('#field-0').value,'not-a-number','invalid text retained after backend validation error');assert.equal(find('#field-1').value,'Unfocused text','nonfocused stage retained across snapshot');assert.ok(find('#native-error').innerHTML.includes('Отменить незавершённый ввод'));
// Choices include the outstanding generic text atomically, then acknowledge only submitted stages.
const mode=new Node();mode.tagName='SELECT';mode.dataset.field='mode';mode.value='b';events.change({target:mode});assert.equal(commands.at(-1).op,'set_field');assert.equal(commands.at(-1).fields[0].value,'not-a-number');vm.runInContext('window.prismestraReceive({kind:"error",message:"Still invalid"})',context);
document.activeElement=find('#field-0');
const beforeClose=commands.length;
vm.runInContext('window.prismestraReceive({kind:"flush",id:500})',context);
assert.equal(commands.length,beforeClose+1,'close sends only the draft receipt and never applies fields');
assert.equal(commands.at(-1).op,'close_ready');
assert.equal(commands.at(-1).form_dirty,true,'close handshake reports unblurred form input');
assert.ok(commands.at(-1).scene,'scene draft accompanies the dirty-form receipt');
assert.ok(!JSON.stringify(commands.at(-1)).includes(document.activeElement.value),'close receipt excludes staged field values');
vm.runInContext('window.prismestraReceive({kind:"close_cancelled"})',context);
assert.equal(vm.runInContext('closing',context),false);
assert.equal(vm.runInContext('stagedDirty()',context),true,'returning to edits preserves the staged form');
vm.runInContext('const previousSceneFile=window.sceneFile;window.sceneFile={blockClose:()=>true};window.prismestraReceive({kind:"flush",id:501});window.sceneFile=previousSceneFile',context);
assert.deepEqual(commands.at(-1),{op:'close_ready',id:501,scene:null,blocked:true},'an active scene-file operation cannot be discarded as a form');
const discard={dataset:{action:'discard-fields'},disabled:false,hasAttribute:()=>false};events.click({target:{closest:()=>discard}});assert.equal(vm.runInContext('stagedDirty()',context),false);assert.equal(find('#field-0').value,'8080','explicit discard renders backend values');
// Empty device profiles expose actual native auxiliary Add actions and blank select placeholders.
const aux={page:'devices',fields:[{id:411,label:'Name',kind:'text',value:'',choices:[],enabled:true},{id:412,label:'Driver',kind:'select',value:'missing',choices:[{label:'First driver',value:'first'}],enabled:true}],actions:[{id:410,label:'Add device',enabled:true}],notes:['No configured devices']};context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeAux:aux,editor:{page:'devices',index:0,targets:[],fields:[],dirty:false}}};vm.runInContext('window.prismestraReceive(payload)',context);assert.ok(find('#main').innerHTML.includes('Add device'));assert.ok(!find('#main').innerHTML.includes('Расширенные настройки'));assert.ok(find('#main').innerHTML.includes('<option value="" disabled selected>Выберите…</option>'));
const auxName=find('#native-field-0');auxName.value='New device';events.input({target:auxName});const add={dataset:{nativeAction:'410'},disabled:false,hasAttribute:()=>false};events.click({target:{closest:()=>add}});assert.equal(commands.at(-1).op,'native_action');assert.equal(commands.at(-1).id,410);assert.deepEqual(commands.at(-1).fields,[{id:411,value:'New device'}]);vm.runInContext('window.prismestraReceive(payload)',context);
// Metadata recovery skips invalid and disabled text payloads and clears only after acknowledgment.
const recoveryForm={page:'schedule',dirty:true,fields:[{id:401,label:'Time',kind:'text',value:'10:00',choices:[],enabled:true}],actions:[{id:408,label:'Reset schedule',enabled:true}],notes:[]};context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeForm:recoveryForm}};vm.runInContext('view="schedule";window.prismestraReceive(payload)',context);const time=find('#native-field-0');time.value='bad-time';events.input({target:time});const beforeScheduleNavigation=commands.length;vm.runInContext('changeView("sources")',context);assert.equal(vm.runInContext('view',context),'schedule');assert.equal(commands.length,beforeScheduleNavigation);
vm.runInContext('send({op:"native_action",id:408},true)',context);assert.equal(commands.at(-1).fields,undefined);assert.equal(commands.at(-1).editor_fields,undefined);assert.equal(vm.runInContext('stagedDirty()',context),true,'reset waits for ack before discarding local stage');vm.runInContext('window.prismestraReceive(payload)',context);assert.equal(vm.runInContext('stagedDirty()',context),false);
const retryForm={...structuredClone(recoveryForm),fields:[{...recoveryForm.fields[0],enabled:true}],actions:[{id:409,label:'Retry',enabled:true}]};context.payload={kind:'snapshot',state:{...structuredClone(initial),pending:true,nativeForm:retryForm}};vm.runInContext('window.prismestraReceive(payload)',context);find('#native-field-0').value='invalid';events.input({target:find('#native-field-0')});context.metadata={kind:'status',state:{nativeForm:{...retryForm,fields:[{...retryForm.fields[0],enabled:false}]}}};vm.runInContext('window.prismestraReceive(metadata);send({op:"native_action",id:409},true)',context);assert.deepEqual(commands.at(-1).fields,[],'disabled fault fields cannot block retry');assert.equal(vm.runInContext('stagedDirty()',context),true,'unsent disabled text is retained');vm.runInContext('window.prismestraReceive(payload)',context);
// Composition and keyboard gestures also defer asynchronous replacement until the next frame.
events.compositionstart({});const beforeComposition=find('#main').innerHTML;context.metadata={kind:'status',state:{nativeForm:{...retryForm,notes:['Composition update']}}};vm.runInContext('window.prismestraReceive(metadata)',context);assert.equal(find('#main').innerHTML,beforeComposition);events.compositionend({});while(rafQueue.length)rafQueue.shift()();assert.ok(find('#main').innerHTML.includes('Composition update'));
events.keydown({key:' ',target:{closest:()=>({})}});context.metadata={kind:'status',state:{nativeForm:{...structuredClone(retryForm),notes:['Keyboard update']}}};vm.runInContext('window.prismestraReceive(metadata)',context);assert.ok(!find('#main').innerHTML.includes('Keyboard update'));events.keyup({});while(rafQueue.length)rafQueue.shift()();assert.ok(find('#main').innerHTML.includes('Keyboard update'));
console.log('PASS: local invalid/nonfocused text staging, first-click atomic apply, choice batches, unblurred-close dirty, explicit discard, empty-profile Aux Add, select placeholder, recovery/disabled-field payloads, pointer/IME/keyboard render deferral');

// Text-only Schedule drafts can be cancelled immediately while backend Reset remains disabled.
const cleanSchedule={...structuredClone(recoveryForm),dirty:false,actions:[{id:408,label:'Reset schedule',enabled:false}]};context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeForm:cleanSchedule}};vm.runInContext('formStages.clear();view="schedule";window.prismestraReceive(payload)',context);const localDiscard=find('local-discard');assert.equal(localDiscard.hidden,true);const cleanTime=find('#native-field-0');cleanTime.value='11:35';const unchangedMarkup=find('#main').innerHTML;const beforeLocalCancel=commands.length;events.input({target:cleanTime});assert.equal(find('local-discard'),localDiscard,'input never rebuilds discard button');assert.equal(find('#main').innerHTML,unchangedMarkup,'availability update does not rebuild form');assert.equal(localDiscard.hidden,false);assert.equal(localDiscard.disabled,false);assert.equal(vm.runInContext('state.nativeForm.dirty',context),false);context.control={dataset:{nativeAction:'408'}};assert.equal(vm.runInContext('nativeControlDisabled(control)',context),true,'backend Reset remains disabled');
vm.runInContext('formStages.set("other-page",{kind:"native",page:"sources",id:999,value:"Keep me"})',context);events.click({target:{closest:()=>localDiscard}});assert.equal(commands.length,beforeLocalCancel,'local cancellation performs no backend operation');assert.equal(vm.runInContext('currentPageStaged()',context),false);assert.equal(vm.runInContext('formStages.get("other-page").value',context),'Keep me','other page edits preserved');assert.equal(find('#native-field-0').value,'10:00');assert.equal(find('local-discard').hidden,true);assert.equal(vm.runInContext('state.nativeForm.dirty',context),false);
console.log('PASS: Schedule text-only draft has immediate local discard without rerender/RPC; current page clears, other page and backend stay untouched');

// Binding Save can consume text typed in WebView before the native form is dirty.
const bindingForm={...structuredClone(form),actions:[{id:1811,label:'Save binding',enabled:false,accepts_staged_text:true}]};
context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeForm:bindingForm}};
vm.runInContext('formStages.clear();view="connections";window.prismestraReceive(payload)',context);
context.control={dataset:{nativeAction:'1811'}};
const bindingSaveDisabled=()=>vm.runInContext('nativeControlDisabled(control)',context);
assert.equal(bindingSaveDisabled(),true);
const endpointField=find('#native-field-0');
const beforeBindingEdit=commands.length;
endpointField.value='192.0.2.27:4048';
events.input({target:endpointField});
events.change({target:endpointField});
assert.equal(commands.length,beforeBindingEdit);
assert.equal(bindingSaveDisabled(),false);
context.payload={kind:'status',state:{nativeForm:{...bindingForm,actions:[{id:1811,label:'Save binding',enabled:false}]}}};
vm.runInContext('window.prismestraReceive(payload)',context);
assert.equal(bindingSaveDisabled(),true,'an unavailable host may not save locally staged input');
context.payload={kind:'status',state:{nativeForm:bindingForm}};
vm.runInContext('window.prismestraReceive(payload); send({op:"native_action",id:1811},true)',context);
assert.equal(commands.length,beforeBindingEdit+1);
assert.deepEqual(commands.at(-1),{op:'native_action',id:1811,fields:[{id:101,value:'192.0.2.27:4048'}],editor_fields:[]});
assert.equal(bindingSaveDisabled(),true,'the save remains locked until the native acknowledgement');
console.log('PASS: first click saves staged connection text only when the host can accept it');

// The selected LCD can have a different index from its binding.
const connectionDevices=['first-lcd','selected-lcd'].map(id=>({
 id,label:'LCD',role:'lcd',driver:{type:'tt_lcd'},leds:0,rate_hz:30,enabled:false
}));
const connectionEditor={page:'lcd',index:0,targets:['LCD','LCD'],fields:[],dirty:false,deviceId:'selected-lcd'};
context.payload={kind:'snapshot',state:{...structuredClone(initial),devices:connectionDevices,editor:connectionEditor}};
vm.runInContext('formStages.clear();view="lcd";window.prismestraReceive(payload)',context);
assert.ok(find('#main').innerHTML.includes('data-connection-device="selected-lcd"'));
const repairButton=new Node();
repairButton.dataset={view:'connections',connectionDevice:'selected-lcd'};
events.click({target:{closest:()=>repairButton}});
assert.deepEqual(commands.at(-1),{op:'page',page:'connections',device:'selected-lcd'});
vm.runInContext('window.prismestraReceive({kind:"error",message:"Target unavailable"})',context);
assert.equal(vm.runInContext('view',context),'lcd','a rejected target restores the source page');
context.payload.state.editor.deviceId='draft-only';
vm.runInContext('window.prismestraReceive(payload)',context);
assert.ok(!find('#main').innerHTML.includes('data-connection-device'),'draft-only outputs cannot edit applied bindings');
context.payload.state.editor.deviceId='selected-lcd';
context.payload.state.editor.dirty=true;
vm.runInContext('window.prismestraReceive(payload)',context);
const beforeRepair=commands.length;
events.click({target:{closest:()=>repairButton}});
assert.equal(commands.length,beforeRepair,'targeted navigation keeps unsaved editor changes protected');
console.log('PASS: repair opens the selected logical output and preserves navigation guards');

// Command errors survive failed-followup snapshots/status, and clear only on a new command's successful ack.
context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeAux:structuredClone(aux),editor:{page:'devices',index:0,targets:[],fields:[],dirty:false}}};vm.runInContext('formStages.clear();view="devices";window.prismestraReceive(payload)',context);const errorName=find('#native-field-0');errorName.value='UI test light';events.input({target:errorName});vm.runInContext('send({op:"native_action",id:410},true);window.prismestraReceive({kind:"error",message:"fixedLEDcount"});window.prismestraReceive(payload)',context);assert.equal(find('#native-error').hidden,false);assert.ok(find('#native-error').innerHTML.includes('fixedLEDcount'));assert.ok(find('#native-error').innerHTML.includes('discard-fields'),'failed command keeps local discard while text remains staged');
vm.runInContext('window.prismestraReceive({kind:"status",state:{error:null,pending:false}})',context);assert.equal(find('#native-error').hidden,false,'periodic success cannot dismiss validation error');
vm.runInContext('send({op:"native_action",id:410},true);window.prismestraReceive(payload)',context);assert.equal(find('#native-error').hidden,true,'new successful command snapshot clears old command error');assert.equal(find('#native-error').innerHTML,'','stale notice actions are removed');assert.equal(vm.runInContext('stagedDirty()',context),false);
context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeAux:structuredClone(aux),error:'Provider still unavailable'}};vm.runInContext('send({op:"native_action",id:410},true);window.prismestraReceive(payload)',context);assert.equal(find('#native-error').hidden,false);assert.ok(find('#native-error').innerHTML.includes('Provider still unavailable'),'successful ack cannot hide an actual backend error');
console.log('PASS: failed snapshot/status retain command error; causal successful ack removes stale error/actions; current backend error remains');

// The cards show applied outputs, whereas the properties selector edits the draft.
const appliedDevices = ['first','second','third'].map(id=>({
 id,label:id,role:'canvas',driver:{type:'stub'},leds:10,rate_hz:48,enabled:true
}));
context.payload={kind:'snapshot',state:{
 ...structuredClone(initial),devices:appliedDevices,draftDevices:appliedDevices.slice(1),
 editor:{page:'devices',index:0,targets:['second','third'],fields:[],dirty:true}
}};
vm.runInContext('formStages.clear();view="devices";window.prismestraReceive(payload)',context);
const secondButtonMarkup=find('#main').innerHTML.match(/<button[^>]*aria-label="Свойства: second"[^>]*>/)[0];
const secondButton=new Node();
for(const [,name,value] of secondButtonMarkup.matchAll(/data-(device-index|device-id)="([^"]+)"/g)){
 secondButton.dataset[name==='device-index'?'deviceIndex':'deviceId']=value;
}
const beforeDeviceSelection=commands.length;
events.click({target:{closest:()=>secondButton}});
assert.equal(commands.length,beforeDeviceSelection+1);
assert.deepEqual(commands.at(-1),{op:'select_device',id:'second'},
 'Open the same output after an earlier device was removed in the draft');
vm.runInContext('window.prismestraReceive(payload)',context);
const objectSelector=new Node();objectSelector.dataset.target='';objectSelector.value='1';
events.change({target:objectSelector});
assert.deepEqual(commands.at(-1),{op:'select_item',index:1},'Native draft dropdown still selects by its own index');
vm.runInContext('window.prismestraReceive(payload);pendingView={previous:"library",next:"devices"}',context);
const beforePendingPage=commands.length;
events.click({target:{closest:()=>secondButton}});
assert.equal(commands.length,beforePendingPage,'Do not send a card selection before native navigation is acknowledged');
vm.runInContext('pendingView=null',context);
console.log('PASS: device cards select by logical identity and the draft dropdown still uses indices');

// A retry belongs to one saved row, request and profile scope.
{
 const button=new Node(), status=new Node(), help=new Node(), disabled=new Map();
 button.dataset.outputRetry='rgb';
 button.parentElement={querySelector:selector=>selector==='[data-retry-help]'?help:status};
 button.setAttribute=(name,value)=>disabled.set(name,value);
 nodes.set('openrgb-retry-test',button);
 const device={id:'rgb',label:'OpenRGB',role:'canvas',leds:2,rate_hz:25,enabled:true};
 const stateForScope=(scope,ticket,index=0)=>({
  ...initial,devices:[device],draftDevices:[device],
  editor:{page:'devices',index,targets:[device.label],fields:[],dirty:false},
  deviceActivity:{retryScope:scope,retryTickets:{rgb:ticket},reasons:{rgb:'endpointLeaseBusy'}},
 });
 const receive=message=>{context.payload=message;vm.runInContext('window.prismestraReceive(payload)',context);};
 const click=()=>events.click({target:{closest:()=>button}});
 receive({kind:'snapshot',state:stateForScope('before','first',1)});
 assert(button.hidden);
 assert(help.hidden);
 assert.match(mainMarkup,/data-output-status="rgb">Подключение занято другим выходом Prismestra/);
 const beforeSelection=commands.length;
 click();
 assert.equal(commands.length,beforeSelection);
 receive({kind:'snapshot',state:stateForScope('before','first')});
 assert(!button.hidden);
 assert.equal(help.hidden,false);
 assert.match(help.textContent,/другим выходом Prismestra/);
 button.focus();
 const start=commands.length;
 click();click();
 const first=commands.at(-1);
 assert.deepEqual(first,{op:'retry_output',id:'rgb',ticket:'first',request:first.request});
 assert.equal(commands.length,start+1);
 assert.equal(status.textContent,'Подключаемся…');
 assert(help.hidden);
 assert.equal(disabled.get('aria-disabled'),'true');
 const reply=(id,request,scope,message)=>receive({kind:'output_retry',id,request,scope,status:'failed',message});
 reply('wled',first.request,'before','wrong row');
 reply('rgb',first.request,'older','wrong profile');
 assert.equal(status.textContent,'Подключаемся…');
 reply('rgb',first.request,'before','Identity changed');
 assert.equal(status.textContent,'Identity changed');
 receive({kind:'status',state:{deviceActivity:{retryScope:'before',retryTickets:{},reasons:{}}}});
 assert(help.hidden);
 assert.equal(vm.runInContext('deviceStatus(state.devices[0])',context),'Недоступно');
 receive({kind:'snapshot',state:stateForScope('after','next')});
 click();
 assert.equal(status.textContent,'Подключаемся…');
 reply('rgb',first.request,'before','late response');
 reply('rgb',first.request,'after','wrong request');
 assert.equal(status.textContent,'Подключаемся…');
 assert.equal(document.activeElement,button);
 receive({kind:'snapshot',state:initial});
 nodes.delete('openrgb-retry-test');
}
console.log('PASS: targeted OpenRGB retry, duplicate click suppression, isolated row error and stale replies');

// An active device inspection announces transitions once and follows its logical ID.
{
 const wled = {id:'desk rgb',label:'WLED',role:'canvas',leds:32,rate_hz:30,enabled:true};
 const other = appliedDevices[0];
 const deviceSnapshot = {
  ...initial, devices:[other,wled], draftDevices:[other,wled],
  editor:{page:'devices',index:0,targets:[other.label,wled.label],fields:[],dirty:false},
  deviceActivity:{labels:{[wled.id]:'Недоступно',[other.id]:'Выключено'}},
};
 const updateDeviceStatus = (selected, unrelated) => {
  context.payload={kind:'status',state:{deviceActivity:{labels:{[wled.id]:selected,[other.id]:unrelated}}}};
  vm.runInContext('window.prismestraReceive(payload)',context);
};
 context.payload={kind:'snapshot',state:deviceSnapshot};
 vm.runInContext('window.prismestraReceive(payload)',context);
 assert.match(mainMarkup,/id="device-status-desk%20rgb" data-output-status="desk rgb"/);
 assert.match(mainMarkup,/aria-label="Свойства: WLED" aria-describedby="device-status-desk%20rgb"/);
 assert.match(mainMarkup,/<p class="parameter-note" id="native-runtime-detail">/);
 const live = find('#device-status-announcement');
 const announcements = [];
 let liveText = live.textContent;
 Object.defineProperty(live,'textContent',{
  get:()=>liveText,
  set:value=>{liveText=value; if(value)announcements.push(value);},
 });
 const inspectedRow = find('[data-device-id="desk rgb"]');
 inspectedRow.dataset.deviceId=wled.id;
 inspectedRow.focus();
 events.click({target:{closest:()=>inspectedRow}});
 assert.deepEqual(commands.at(-1),{op:'select_device',id:wled.id});
 context.payload={kind:'snapshot',state:{...deviceSnapshot,editor:{...deviceSnapshot.editor,index:1}}};
 vm.runInContext('window.prismestraReceive(payload)',context);
 assert.deepEqual(announcements,[],'Activation and the initial state are silent');
 updateDeviceStatus('Подключается','Недоступно');
 updateDeviceStatus('Подключается','Выключено');
 const properties = new Node();
 properties.closest = selector=>selector==='#device-properties'?properties:null;
 properties.focus();
 events.focusin();
 updateDeviceStatus('Кадры отправлены','Выключено');
 assert.deepEqual(announcements,['WLED. Подключается.','WLED. Кадры отправлены.']);
 const escapeProperties = () => events.keydown({key:'Escape',target:properties,preventDefault(){}});
 escapeProperties();
 assert.equal(document.activeElement,inspectedRow);
 properties.focus();
 events.focusin();
 inspectedRow.disabled=true; // The selector mock keeps detached nodes; the browser test removes the row.
 escapeProperties();
 assert.equal(document.activeElement,find('h1'));
 events.focusin();
 updateDeviceStatus('Недоступно','Выключено');
 assert.equal(announcements.length,2,'Status changes outside the inspection stay silent');
 vm.runInContext('deviceInspection=null',context);
 console.log('PASS: selected device status changes only, spoken description, and logical focus return');
}

const candidateFixtures = require('./fixtures/device-candidates.json');
const [foundRgb, foundLcd] = candidateFixtures.proposal.deviceCandidates;
const foundState = {
 ...initial,
 editor: {page: 'devices', index: 0, targets: [], fields: [], dirty: false},
 ...candidateFixtures.proposal,
};
const receiveDeviceState = (kind, state) => {
 context.payload = {kind, state};
 vm.runInContext('window.prismestraReceive(payload)', context);
};
const hasCandidate = id => mainMarkup.includes(`data-candidate-id="${id}"`);
receiveDeviceState('snapshot', foundState);
assert.ok(hasCandidate(foundRgb.id) && hasCandidate(foundLcd.id), 'same-label candidates keep separate IDs');
const discoveryAnnouncement = find('#device-discovery-announcement');
assert.equal(discoveryAnnouncement.textContent, 'Найдено устройств: 2.');
receiveDeviceState('status', {detail: 'Keep current candidates'});
assert.ok(hasCandidate(foundRgb.id) && hasCandidate(foundLcd.id));
receiveDeviceState('snapshot', {...foundState, devices: [{...foundRgb, leds: 1}], editor: {
 page: 'devices', index: 1, targets: ['Fixture'], fields: [], dirty: false,
}});
assert.ok(!hasCandidate(foundRgb.id) && hasCandidate(foundLcd.id), 'applied ID replaces a matching candidate');
assert.ok(mainMarkup.includes('<option value="" disabled selected>Выберите устройство…</option>'));
receiveDeviceState('status', candidateFixtures.cancelled);
assert.ok(!hasCandidate(foundLcd.id), 'empty metadata removes stale candidate rows');
assert.equal(discoveryAnnouncement.textContent, '');
receiveDeviceState('status', candidateFixtures.replacement);
assert.ok(hasCandidate(candidateFixtures.replacement.deviceCandidates[0].id));
assert.equal(discoveryAnnouncement.textContent, 'Найдено устройство: Fixture.');
console.log('PASS: candidate identity, applied precedence, metadata replacement, empty device selection');

vm.runInContext('view="devices";state.editor={page:"devices",index:0,targets:["Strip"],fields:[],dirty:false};state.detail="Ошибка RGB: <test>";render()',context);
assert.ok(find('#main').innerHTML.includes('Ошибка RGB: &lt;test&gt;'));

// Map replies, connection changes and focus all belong to the current logical output.
const mapEvents = {};
const mapNodes = new Map();
const mapNode = id => {
 if (!mapNodes.has(id)) mapNodes.set(id, new Node());
 return mapNodes.get(id);
};
const mapDocument = {
 getElementById: mapNode,
 createElement: () => Object.assign(new Node(), {style: {}, insertCell: () => new Node()}),
};
const mapDialog = mapNode('output-map');
mapDialog.addEventListener = (event, handler) => { mapEvents[event] = handler; };
const mapTable = mapNode('output-map-table');
mapTable.tBodies = [{replaceChildren(...rows) { this.rows = rows; }}];
mapNode('native-target').after = (...buttons) => {
 for (const button of buttons) mapNodes.set(button.id, button);
};
const mapWindow = {};
vm.runInNewContext(outputMapSource, {window: mapWindow, document: mapDocument});
const map = mapWindow.outputMap;
const strip = {id: 'strip', role: 'canvas', leds: 1};
const other = {...strip, id: 'other'};
const mapState = {
 profilePath: 'fixture',
 editor: {page: 'devices', index: 1},
 devices: [other, strip],
 draftDevices: [other, strip],
 draftMapRevision: '12',
 mapConnection: {device: 'strip', state: 'ready'},
};
const mapRequests = [];
const mountMap = () => map.mount(mapState, command => !!mapRequests.push(command), () => []);
const reply = (command, extra = {}) => ({
 request: command.request, device: command.device, source: command.source,
 revision: command.revision, availability: 'ready',
 preview: {device: command.device, label: 'Strip', width: 1, height: 1, total: 1, offset: 0,
  entries: [{led: 0, x: 0, y: 0, color: 0xff0000}]},
 ...extra,
});
mountMap();
mapNode('draft-output-map-open').onclick();
const draftRequest = mapRequests.at(-1);
assert.equal(draftRequest.device, 'strip');
map.receive(reply(draftRequest, {revision: '11'}));
map.receive(reply(draftRequest, {device: 'other'}));
assert.equal(mapTable.hidden, true);
map.receive(reply(draftRequest));
assert.equal(mapTable.hidden, false);
assert.equal(mapNode('output-map-mode').hidden, false);
map.update({...mapState, mapConnection: {device: 'other', state: 'offline'}});
assert.equal(mapNode('output-map-connection').hidden, true);
mapState.mapConnection.state = 'offline';
map.update(mapState);
assert.equal(mapNode('output-map-connection').textContent, 'Выход отключён.');
assert.equal(mapTable.hidden, false, 'offline transport leaves draft rows visible');
mapNode('output-map-applied').onclick();
assert.equal(mapNode('output-map-mode').hidden, true);
assert.equal(document.activeElement, mapNode('output-map-refresh'));
assert.equal(mapRequests.length, 1, 'offline return does not request a buffered frame');
map.receive(reply(draftRequest));
assert.equal(mapTable.hidden, true);
mapState.mapConnection.state = 'ready';
map.update(mapState);
const appliedRequest = mapRequests.at(-1);
assert.equal(appliedRequest.source, 'applied');
assert.equal(appliedRequest.offset, 0);
map.receive(reply(appliedRequest));
assert.equal(mapTable.hidden, false);
mapState.mapConnection.state = 'offline';
map.update(mapState);
map.receive(reply(appliedRequest));
assert.equal(mapTable.hidden, true, 'late applied reply cannot restore an offline frame');
const originalOpener = mapNode('draft-output-map-open');
mountMap();
mapEvents.cancel({preventDefault() {}});
assert.equal(mapDialog.open, false);
assert.notEqual(mapNode('draft-output-map-open'), originalOpener);
assert.equal(document.activeElement, mapNode('draft-output-map-open'));
assert.ok(mapRequests.every(command => command.op === 'preview_output' && command.device === 'strip'));
console.log('PASS: one map output, draft revision, offline draft, return and reconnect, stale replies, focus after remount');
context.payload={kind:'status',state:{detail:'RGB: передано кадров 153'}};
vm.runInContext('window.prismestraReceive(payload)',context);
assert.equal(find('#native-runtime-detail').textContent,'RGB: передано кадров 153');
console.log('PASS: selected output diagnostics are escaped and update from runtime status messages');

// Audit regression: selecting B must preserve the unsaved draft of A until the user decides.
const entryA={id:'scene-a',name:'A',scene:{...scene,colors:[0x112233]}};
const entryB={id:'scene-b',name:'B',scene:{...scene,colors:[0x445566]}};
const draftState={...initial,editor:null,draftScene:null,library:[entryA,entryB]};
const evaluate=expression=>vm.runInContext(expression,context);
function restoreDraft(snapshot){
 context.payload={kind:'snapshot',state:structuredClone(snapshot)};
 evaluate('draft=null;view="library";window.prismestraReceive(payload)');
}
function selectScene(id){
 events.click({target:{closest:()=>({dataset:{edit:id},disabled:false,hasAttribute:()=>true})}});
}
restoreDraft(draftState);
selectScene(entryA.id);
evaluate('draft.colors[0]=0xaabbcc;editDraft();flushPreview();changeView("library")');
const draftA=commands.at(-1);
assert.equal(draftA.origin.id,entryA.id);
assert.equal(draftA.origin.scene.colors[0],0x112233);
const beforeSwitch=commands.length;
selectScene(entryB.id);
assert.equal(find('#draft-switch').open,true);
assert.equal(commands.length,beforeSwitch);
assert.equal(evaluate('draft.colors[0]'),0xaabbcc);
find('#draft-stay').onclick();
assert.equal(evaluate('view'),'editor');
assert.equal(evaluate('draft.colors[0]'),0xaabbcc);
evaluate('changeView("library")');
selectScene(entryA.id);
assert.equal(find('#draft-switch').open,false,'reopening A resumes its draft');
assert.equal(evaluate('draft.colors[0]'),0xaabbcc);

// Even if the same edits were applied live, they are still unsaved in the library.
context.payload={kind:'status',state:{scene:draftA.scene,pending:false}};
evaluate('window.prismestraReceive(payload);changeView("library")');
selectScene(entryB.id);
find('#draft-save').onclick();
assert.equal(commands.at(-1).op,'save_scene');
assert.equal(commands.at(-1).id,entryA.id);
assert.equal(commands.at(-1).scene.colors[0],0xaabbcc);
evaluate('window.prismestraReceive({kind:"status",state:{pending:false}})');
assert.equal(evaluate('busy()'),true,'periodic output status does not acknowledge saving');
context.payload={kind:'snapshot',state:{...draftState,scene:draftA.scene}};
evaluate('window.prismestraReceive(payload)');
assert.equal(evaluate('busy()'),true,'an unrelated snapshot without the saved entry does not unlock the switch');
evaluate('window.prismestraReceive({kind:"draft_error",message:"Could not autosave preview"})');
assert.equal(evaluate('busy()'),true,'preview autosave errors do not cancel the pending catalog save');
assert.equal(evaluate('selectedId'),entryA.id);
evaluate('window.prismestraReceive({kind:"error",message:"Cannot write catalog"})');
assert.equal(evaluate('selectedId'),entryA.id);
assert.equal(evaluate('draft.colors[0]'),0xaabbcc);
selectScene(entryB.id);
find('#draft-save').onclick();
context.payload={kind:'snapshot',state:{...draftState,library:[{...entryA,scene:draftA.scene},entryB]}};
evaluate('window.prismestraReceive(payload)');
assert.equal(evaluate('selectedId'),entryB.id);
assert.equal(commands.at(-1).op,'preview_scene');
assert.equal(commands.at(-1).origin.id,entryB.id);
assert.ok(!commands.slice(beforeSwitch).some(command=>command.op==='apply_scene'));

restoreDraft({...draftState,draftScene:draftA.scene,draftOrigin:draftA.origin});
assert.equal(evaluate('draftName'),'A');
assert.equal(evaluate('selectedId'),entryA.id);
selectScene(entryB.id);
find('#draft-discard').onclick();
assert.equal(evaluate('selectedId'),entryB.id);
assert.equal(commands.at(-1).scene.colors[0],0x445566);

restoreDraft({...draftState,draftScene:draftA.scene});
assert.equal(evaluate('draftName'),'Восстановленный черновик');
selectScene(entryB.id);
find('#draft-save').onclick();
assert.equal(find('#name-dialog').open,true);
find('#scene-name').value='Recovered';
find('#name-form').onsubmit({preventDefault(){}});
assert.equal(commands.at(-1).id,null,'legacy draft saves as a new entry');
assert.equal(evaluate('draft.colors[0]'),0xaabbcc);
context.payload={kind:'snapshot',state:{...draftState,library:[entryA,entryB,{id:'restored',name:'Recovered',scene:draftA.scene}]}};
evaluate('window.prismestraReceive(payload)');
assert.equal(evaluate('selectedId'),entryB.id);

restoreDraft({...draftState,draftScene:draftA.scene,draftOrigin:draftA.origin,library:[{...entryA,name:'Reused ID'},entryB]});
assert.equal(evaluate('selectedId'),null,'reused source ID does not silently reattach');
assert.equal(evaluate('draftName'),'A');
assert.equal(evaluate('draft.colors[0]'),0xaabbcc);
evaluate('window.prismestraReceive({kind:"flush",id:93})');
assert.equal(commands.at(-1).origin.id,null);
assert.equal(commands.at(-1).origin.name,'A');
console.log('PASS: scene draft guard, all three choices, acknowledged save and error recovery, named/legacy restore, stale source detachment');

const navFixture = (devices = [], extra = {}) => ({
 ...structuredClone(initial), devices, draftDevices: devices, deviceCandidates: [],
 schedule: {enabled: false, paused: false}, ...extra,
});
const navHas = page => find('nav').innerHTML.includes(`data-view="${page}"`);
const viewNow = () => vm.runInContext('view', context);
const openView = page => vm.runInContext('changeView('+JSON.stringify(page)+')', context);
const navLcd = {id: 'lcd', label: 'LCD', role: 'lcd', enabled: true, leds: 0};
const navAmbient = {id: 'ambient', label: 'Screen strip', role: 'ambient', enabled: false, leds: 10};
vm.runInContext('window.prismestraReceive({kind:"close_cancelled"})', context);
vm.runInContext('formStages.clear();view="library";pendingView=null', context);
receiveDeviceState('snapshot', navFixture([], {deviceCandidates: [navLcd, navAmbient]}));
assert.ok(navHas('devices') && navHas('settings'));
assert.ok(['lcd','ambient','connections','temperatures','sources','schedule'].every(page => !navHas(page)));
const beforeHiddenOutput = commands.length;
openView('lcd');
assert.equal(commands.length, beforeHiddenOutput, 'Unapplied discovery does not open the LCD editor');
openView('devices');
receiveDeviceState('snapshot', navFixture([], {editor: {page:'devices',index:0,targets:[],fields:[],dirty:false}, nativeAux:{
 page:'devices',fields:[{id:714,label:'Семейство',kind:'select',enabled:true,value:'lcd',choices:[]}],actions:[],notes:[],
}}));
assert.ok(mainMarkup.includes('data-action="focus-device-setup"'));
assert.ok(mainMarkup.includes('data-view="connections"'));
openView('connections');
assert.deepEqual(commands.at(-1), {op:'page',page:'connections'});
receiveDeviceState('snapshot', navFixture([navLcd, navAmbient], {deviceActivity:{labels:{lcd:'Нет связи'}}}));
assert.ok(navHas('lcd') && navHas('ambient'), 'Configured disabled and unreachable outputs stay discoverable');
openView('lcd');
receiveDeviceState('snapshot', navFixture([navLcd, navAmbient], {editor:{page:'lcd',index:0,targets:['LCD'],fields:[],dirty:false}}));
assert.equal(viewNow(), 'lcd');
assert.ok(mainMarkup.includes('Проверить подключение') && mainMarkup.includes('Температуры'));
receiveDeviceState('snapshot', navFixture([navAmbient], {pending:true}));
assert.equal(viewNow(), 'lcd', 'Wait for configuration to settle before leaving the page');
receiveDeviceState('status', {pending:false});
assert.deepEqual(commands.at(-1), {op:'page',page:'devices'});
assert.equal(viewNow(), 'devices');
assert.ok(!navHas('lcd') && navHas('ambient'));
receiveDeviceState('snapshot', navFixture([navAmbient], {schedule:{enabled:true,paused:true}}));
openView('settings');
assert.deepEqual(commands.at(-1), {op:'page',page:'settings'});
receiveDeviceState('snapshot', navFixture([navAmbient], {schedule:{enabled:true,paused:true}}));
assert.ok(mainMarkup.includes('Диагностика') && mainMarkup.includes('Расписание на паузе'));
openView('schedule');
assert.deepEqual(commands.at(-1), {op:'page',page:'schedule'});
assert.ok(find('nav').innerHTML.includes('data-view="settings" class="active"'));
console.log('PASS: hidden output links, discovery access, configured offline routes, removal fallback, settings and schedule');

vm.runInContext('formStages.clear();view="devices";pendingView=null', context);
receiveDeviceState('snapshot', navFixture([], {status:'Ищу поддерживаемые устройства…'}));
assert.ok(find('#player-status').textContent.includes('Ищу поддерживаемые устройства'));
assert.ok(navHas('devices') && !navHas('lcd') && !navHas('ambient'));
receiveDeviceState('status', {status:'Автопоиск завершён'});
assert.ok(!navHas('lcd') && !navHas('ambient'));
for (const output of [navLcd, navAmbient]) {
 const role=output.role;
 const editorFor = (id, index=0) => ({
  page:role,index,deviceId:id,targets:['Same label','Same label'],
  fields:[{key:'label',label:'Label',value:'Original',choices:[]}],dirty:false,
 });
 receiveDeviceState('snapshot', navFixture([output]));
 openView(role);
 receiveDeviceState('snapshot', navFixture([output], {editor:editorFor(output.id)}));
 const editing=find('#field-0');
 editing.value='Keep this input';
 events.input({target:editing});
 receiveDeviceState('status', {deviceActivity:{labels:{[output.id]:'Недоступно'}}});
 assert.equal(find('#field-0').value,'Keep this input','Transport loss keeps the active form');
 assert.ok(navHas(role));
 const replacement={...output,id:output.id+'-other'};
 events.compositionstart({});
 receiveDeviceState('snapshot', navFixture([replacement], {editor:editorFor(replacement.id)}));
 editing.value='Keep this composed input';
 events.input({target:editing});
 events.compositionend({});
 while(rafQueue.length)rafQueue.shift()();
 assert.ok(mainMarkup.includes('Незавершённый ввод сохранён'));
 assert.ok(!mainMarkup.includes('id="field-0"'),'Do not offer replacement fields while the original input is retained');
 const beforeRepair=commands.length;
 vm.runInContext('send({op:"apply_fields"},true)',context);
 assert.equal(commands.length,beforeRepair,'Pending fields cannot be sent to a replacement at the same index');
 receiveDeviceState('snapshot', navFixture([replacement,output], {editor:editorFor(output.id,1)}));
 assert.equal(find('#field-0').value,'Keep this composed input','Original ID restores input after a reorder');
 receiveDeviceState('snapshot', navFixture([], {editor:null}));
 assert.equal(viewNow(),role,'Explicit removal cannot navigate away from unsaved input');
 assert.ok(!navHas(role));
 vm.runInContext('discardPageFields()',context);
 assert.equal(viewNow(),'devices','Explicit discard lets the normal empty-page fallback run');
 receiveDeviceState('snapshot',navFixture([]));
}
console.log('PASS: startup waiting, detached output input, no wrong-target mutation, reconnect and explicit discard');

vm.runInContext('formStages.clear();view="settings";pendingView=null',context);
const settingsState = navFixture();
receiveDeviceState('snapshot',settingsState);
const scheduleOpener = new Node();
scheduleOpener.dataset.view = 'schedule';
scheduleOpener.closest = selector => selector === '.settings-links' ? new Node() : null;
const scheduleLink = find('#main .settings-links [data-view="schedule"]');
const scheduleForm = {...settingsState,nativeForm:{page:'schedule',fields:[],actions:[],notes:[]}};
const returnFromSchedule = () => {
 events.click({target:{closest:()=>scheduleOpener}});
 receiveDeviceState('snapshot',scheduleForm);
 openView('settings');
 assert.equal(viewNow(),'settings');
 assert.notEqual(document.activeElement,scheduleLink,'Wait for the Settings snapshot before restoring focus');
};
returnFromSchedule();
document.activeElement=find('#main h1');
receiveDeviceState('status',{pending:false});
assert.notEqual(document.activeElement,scheduleLink);
receiveDeviceState('snapshot',settingsState);
assert.equal(document.activeElement,scheduleLink,'Restore the initiating Settings link after acknowledgement');
returnFromSchedule();
const appearanceButton=find('#appearance-toggle');
appearanceButton.id='appearance-toggle';
document.activeElement=appearanceButton;
receiveDeviceState('snapshot',settingsState);
assert.equal(document.activeElement,appearanceButton,'An asynchronous return does not steal focus moved by the user');
console.log('PASS: Settings return focus waits for acknowledgement and respects user focus');

// The provider catalog drives all scene labels and remains practical for a large effect list.
{
 const metadata=[{id:'noise_map',name:'Карта шума'},{id:'breathing',name:'Дыхание'},
  ...Array.from({length:78},(_,i)=>({id:'extra_'+i,name:'Движение '+i}))];
 metadata.at(-1).name='Край <света>';
 const extraScene={...scene,effect:metadata.at(-1).id};
 vm.runInContext('view="library";draft=null;search="";libraryEffect=""',context);
 receiveDeviceState('snapshot',{...initial,scene:extraScene,effects:metadata,
  library:[{id:'extra',name:'Край',scene:extraScene},{id:'noise',name:'Шум',scene}]});
 assert.equal(find('#player-name').textContent,'Край <света>');
 assert.equal(vm.runInContext('effects.extra_77',context),'Край <света>','the portable scene dialog reads the same labels');
 assert.match(mainMarkup,/<option value="extra_77"[^>]*>Край &lt;света&gt;<\/option>/);
 assert.doesNotMatch(mainMarkup,/<option value="breathing"/,'library filter only lists effects present in its scenes');
 events.change({target:{id:'library-effect',value:'extra_77'}});
 assert.match(mainMarkup,/data-edit="extra"/);
 assert.doesNotMatch(mainMarkup,/data-edit="noise"/);

 vm.runInContext('view="editor";draft.speed=1;render()',context);
 assert.match(mainMarkup,/<label for="scene-effect">Эффект<\/label>/);
 const effectOptions=Array.from(mainMarkup.matchAll(/<option value="((?:noise_map|breathing|extra_\d+))"/g),match=>match[1]);
 assert.deepEqual(effectOptions,metadata.map(effect=>effect.id));
 const select=find('#scene-effect'),advanced=find('#advanced-scene');
 select.id='scene-effect';select.value='breathing';select.focus();advanced.open=true;
 const previousMarkup=mainMarkup,beforeChoice=commands.length;
 events.change({target:select});
 assert.equal(mainMarkup,previousMarkup,'selection leaves the native picker and stage mounted');
 assert.equal(document.activeElement,select);
 assert.equal(commands.length,beforeChoice,'changing the effect is a draft edit');
 assert.equal(vm.runInContext('draft.speed',context),10,'the effect-specific speed limit applies');
 const parameters=find('#scene-parameters').innerHTML;
 assert.match(parameters,/<details id="advanced-scene" open>/);
 assert.doesNotMatch(parameters,/data-param="noise.frequency"/);
 vm.runInContext('flushPreview()',context);
 assert.equal(commands.at(-1).op,'preview_scene');
 assert.equal(commands.at(-1).scene.effect,'breathing');
 assert.equal(vm.runInContext('state.scene.effect',context),'extra_77');
}
console.log('PASS: 80 effect choices in provider order, shared escaped labels and filtering, focused draft-only effect preview');

{
 const camera={...scene,effect:'camera_color'};
 const effects=[{id:'camera_color',name:'Цвет камеры',reactive:'camera',directColor:true}];
 vm.runInContext('view="editor";draft=null',context);
 const before=commands.filter(command=>command.op==='apply_scene').length;
 receiveDeviceState('snapshot',{...initial,scene:camera,draftScene:null,effects,native:true,captureAuthorized:false});
 assert.equal(find('#footer-apply').disabled,false,'a restored effect can be applied without editing it');
 assert.equal(commands.filter(command=>command.op==='apply_scene').length,before,'restore cannot apply the input');
 find('#footer-apply').onclick();
 assert.equal(commands.at(-1).op,'apply_scene');
 assert.equal(commands.at(-1).scene.effect,'camera_color');
 receiveDeviceState('snapshot',{...initial,scene:camera,draftScene:null,effects,native:true,captureAuthorized:true});
 assert.equal(find('#footer-apply').disabled,true,'an applied unchanged scene needs no second click');
}
console.log('PASS: restored capture is inert and can be explicitly applied without editing the effect');

{
 const entry={id:'night',name:'Ночная',scene:{...scene,brightness:37},preview:{width:1,height:1,colors:[0x123456]}};
 const libraryState={...initial,library:[entry]};
 vm.runInContext('view="library";draft=null;search="";libraryEffect=""',context);
 receiveDeviceState('snapshot',libraryState);
 assert.match(mainMarkup,/<button[^>]+data-apply="night"[^>]*>Включить сцену<\/button>/);
 assert.match(mainMarkup,/data-preview="night"/);
 assert.match(mainMarkup,/data-edit="night"/);
 const click=(key)=>events.click({target:{closest:()=>({
  dataset:{[key]:'night'},hasAttribute:name=>name==='data-mutation'&&key!=='preview',
 })}});
 const status=find('#scene-activation-night');
 const devices=find('#scene-devices-night');
 const before=commands.length;
 click('preview');
 assert.equal(find('#library-preview-dialog').open,true);
 assert.equal(commands.length,before,'preview uses the cached frame and sends no command');
 assert.equal(viewNow(),'library');
 click('apply');click('apply');
 const apply=commands.at(-1);
 assert.deepEqual(apply,{op:'apply_scene',scene:entry.scene,request:apply.request});
 assert.deepEqual(commands.slice(before).filter(command=>command.op==='apply_scene'),[apply]);
 assert.match(status.textContent,/Включаем/);
 assert.equal(devices.hidden,true);
 receiveDeviceState('snapshot',{...libraryState,scene:entry.scene});
 receiveDeviceState('status',{scene:entry.scene,pending:false});
 assert.match(status.textContent,/Включаем/,'snapshots and unrelated status cannot confirm even the same scene');
 const reply=(request,result,message)=>vm.runInContext(
  'window.prismestraReceive('+JSON.stringify({kind:'scene_activation',request,status:result,message})+')',context);
 reply(apply.request+1,'applied');
 assert.match(status.textContent,/Включаем/);
 reply(apply.request,'failed','Устройство недоступно.');
 assert.match(status.textContent,/Не удалось включить.*Устройство недоступно/);
 assert.equal(devices.hidden,true);
 click('apply');
 const second=commands.at(-1);
 assert.notEqual(second.request,apply.request);
 vm.runInContext('window.prismestraReceive({kind:"error",message:"Ошибка другой команды"})',context);
 assert.equal(find('#native-error').hidden,false);
 assert.match(find('#native-error').innerHTML,/Ошибка другой команды/);
 assert.match(status.textContent,/Включаем/,'an unrelated error cannot settle the activation');
 click('apply');
 assert.equal(commands.at(-1),second,'an unrelated error cannot unlock another activation');
 reply(apply.request,'applied');
 assert.match(status.textContent,/Включаем/);
 reply(second.request,'applied');
 assert.equal(status.textContent,'Сцена «Ночная» применена.');
 assert.equal(find('#native-error').hidden,false,'the scene result does not dismiss the unrelated error');
 assert.equal(devices.hidden,false);
 receiveDeviceState('status',{native:false});
 assert.equal(devices.hidden,true,'hardware status is not offered in preview');
 assert.equal(viewNow(),'library');
 assert.equal(vm.runInContext('draft.brightness',context),65,'activation does not replace the editor draft');
}
console.log('PASS: scene activation survives an unrelated error, ignores stale replies, and settles only on matching rejection or success');
