const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const source=fs.readFileSync(path.join(__dirname,'..','src','ui','windows','web','app.js'),'utf8');
const outputMapSource=fs.readFileSync(path.join(__dirname,'..','src','ui','windows','web','output-map.js'),'utf8');
new vm.Script(fs.readFileSync(path.join(__dirname,'..','src','ui','windows','web','argb-channel.js'),'utf8'));
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
assert.deepEqual(commands[beforeBuiltIn],{op:'editor_preview_visible',visible:true});
assert.ok(commands.slice(beforeBuiltIn+1).every(command=>command.op==='preview_scene'));
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

// Apply uses the native confirmation route. A cancelled dialog leaves the
// applied snapshot intact, and Apply can be requested again after the reply.
{
 const strip={id:'surplife-strip',label:'Desk strip',driver:{type:'surplife_e1_pixels'},role:'canvas',leds:120,rate_hz:10,enabled:true};
 const neighbor={id:'neighbor',label:'Neighbor',driver:{type:'open_rgb'},role:'canvas',leds:12,rate_hz:25,enabled:true};
 const apply={id:1818,label:'Apply',enabled:true};
 const connections={...structuredClone(initial),devices:[strip,neighbor],nativeForm:{page:'connections',fields:[],actions:[apply],notes:[]}};
 context.payload={kind:'snapshot',state:connections};
 vm.runInContext('formStages.clear();view="connections";window.prismestraReceive(payload)',context);
 const button={dataset:{nativeAction:'1818'},disabled:false,hasAttribute:()=>false};
 const click=()=>events.click({target:{closest:()=>button}});
 const before=commands.length;
 click();
 click();
 const command={op:'native_action',id:1818,fields:[],editor_fields:[]};
 assert.deepEqual(commands.slice(before),[command]);
 context.payload={kind:'snapshot',state:connections};
 vm.runInContext('window.prismestraReceive(payload)',context);
 assert.equal(vm.runInContext('state.devices[0].leds',context),120);
 click();
 assert.deepEqual(commands.slice(before),[command,command]);
 context.payload={kind:'snapshot',state:{...connections,devices:[{...strip,leds:153},neighbor]}};
 vm.runInContext('window.prismestraReceive(payload)',context);
 assert.equal(vm.runInContext('state.devices[0].leds',context),153);
 assert.equal(vm.runInContext('state.devices[1].leds',context),12);
 console.log('PASS: Connections Apply requests one native action at a time and can retry after a cancelled layout review');
}

// First-time portable setup uses the existing native action bridge.
const portableAction={id:1822,label:'Настроить локальный OpenRGB…',enabled:true};
context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeForm:{page:'connections',fields:[],actions:[portableAction],notes:[]}}};
vm.runInContext('formStages.clear();window.prismestraReceive(payload)',context);
assert.match(find('#main').innerHTML,/data-native-action="1822"[^>]*>Настроить локальный OpenRGB…<\/button>/);
const portableButton={dataset:{nativeAction:'1822'},disabled:false,hasAttribute:()=>false};
events.click({target:{closest:()=>portableButton}});
assert.deepEqual(commands.at(-1),{op:'native_action',id:1822,fields:[],editor_fields:[]});
context.payload={kind:'snapshot',state:{...structuredClone(initial),nativeForm:{page:'connections',fields:[],actions:[],notes:[]}}};
vm.runInContext('window.prismestraReceive(payload)',context);
assert.doesNotMatch(find('#main').innerHTML,/data-native-action="1822"/);
console.log('PASS: portable OpenRGB setup appears and dispatches through the native action bridge');

const helperNotice='Настройки OpenRGB закреплены за задачей. Для смены EXE попросите администратора проверить новый файл и обновить пин в задаче. Затем подтвердите файл через --reauthorize-portable-openrgb. Инструкция в managed-openrgb.md.';
const linkedForm={page:'connections',fields:[],actions:[],notes:[],notice:helperNotice};
context.payload={kind:'status',state:{nativeForm:linkedForm}};
const beforeHelperHint=commands.length;
vm.runInContext('window.prismestraReceive(payload)',context);
assert.match(find('#main').innerHTML,/<p class="parameter-note" role="status">[^<]*администратора[^<]*--reauthorize-portable-openrgb[^<]*<\/p><div class="native-form">/);
assert.doesNotMatch(find('#main').innerHTML,/data-native-action="1822"/);
assert.equal(commands.length,beforeHelperHint,'helper notice is read-only');
context.payload={kind:'status',state:{nativeForm:{...linkedForm,notice:undefined}}};
vm.runInContext('window.prismestraReceive(payload)',context);
assert.doesNotMatch(find('#main').innerHTML,/--reauthorize-portable-openrgb/);
console.log('PASS: linked portable OpenRGB explains the block and its safe next step above the form');

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

// Duplicate names must remain distinguishable on the cards, like in the native target selector.
{
 const [working, blocked] = ['rgb-left','rgb-right'].map(id=>({
  id,label:'ARGB',role:'canvas',driver:{type:'open_rgb'},leds:32,rate_hz:30,enabled:true
 }));
 const lcd = {id:'usb-lcd',label:'LCD',role:'lcd',driver:{type:'tt_lcd'},leds:0,rate_hz:30,enabled:true};
 const activity = {
  labels:{[working.id]:'Кадры отправлены',[blocked.id]:'Недоступно',[lcd.id]:'Недоступно'},
  reasons:{[blocked.id]:'endpointLeaseBusy'}
 };
 const snapshot = (devices,selected,detail) => {
  context.payload={kind:'snapshot',state:{
   ...structuredClone(initial),devices,draftDevices:devices,detail,deviceActivity:activity,
   editor:{page:'devices',deviceId:selected.id,index:devices.indexOf(selected),
    targets:devices.map(({label,id})=>`${label} (${id})`),fields:[],dirty:false}
  }};
  vm.runInContext('window.prismestraReceive(payload)',context);
 };
 const selectCard = device => {
  const button=find(`[data-device-id="${device.id}"]`);
  button.dataset.deviceId=device.id;
  button.focus();
  events.click({target:{closest:()=>button}});
  assert.deepEqual(commands.at(-1),{op:'select_device',id:device.id});
  return button;
 };
 vm.runInContext('formStages.clear();view="devices";pendingView=null',context);
 snapshot([working,blocked,lcd],working,'Кадры отправлены: 5');
 assert.match(mainMarkup,/<h2>ARGB \(rgb-left\)<\/h2>/);
 assert.match(mainMarkup,/<h2>ARGB \(rgb-right\)<\/h2>/);
 assert.match(mainMarkup,/aria-label="Свойства: ARGB \(rgb-right\)" aria-describedby="device-status-rgb-right"/);
 assert.match(mainMarkup,/aria-label="Свойства: LCD" aria-describedby="device-status-usb-lcd"/);
 const conflictCard=selectCard(blocked);
 snapshot([blocked,working,lcd],blocked,'Ожидает повтора');
 assert.equal(document.activeElement,conflictCard);
 assert.match(mainMarkup,/data-output-status="rgb-right">Подключение занято другим выходом Prismestra/);
 assert.match(mainMarkup,/data-connection-device="rgb-right"/);
 vm.runInContext('deviceInspection=null',context);
 console.log('PASS: same-name cards identify their outputs; selection, reason and connection target survive a reordered snapshot');
}

// Runtime detail is scoped to the selected LCD. Its existing connection action uses the same ID.
{
 const [missing,neighbor]=['missing-lcd','neighbor-lcd'].map(id=>({
  id,label:'LCD',role:'lcd',driver:{type:'tt_lcd'},leds:0,rate_hz:30,enabled:true
 }));
 const guidance='Windows не находит LCD по сохранённому подключению. Проверьте питание и USB-кабель экрана. Если экран подключён, откройте его подключение и проверьте привязку.';
 const snapshot = (devices,selected,detail) => {
  context.payload={kind:'snapshot',state:{
   ...structuredClone(initial),devices,draftDevices:devices,detail,
   editor:{page:'devices',deviceId:selected?.id,index:selected?devices.indexOf(selected):0,
    targets:devices.map(({label,id})=>`${label} (${id})`),fields:[],dirty:false}
  }};
  vm.runInContext('window.prismestraReceive(payload)',context);
 };
 vm.runInContext('formStages.clear();view="devices";pendingView=null',context);
 snapshot([missing,neighbor],missing,'Подключается');
 context.payload={kind:'status',state:{detail:guidance}};
 vm.runInContext('window.prismestraReceive(payload)',context);
 assert.equal(find('#native-runtime-detail').textContent,guidance);
 snapshot([neighbor,missing],missing,guidance);
 assert.match(mainMarkup,/data-connection-device="missing-lcd"/);
 const repair=new Node();
 repair.dataset={view:'connections',connectionDevice:missing.id};
 const beforeRepair=commands.length;
 events.click({target:{closest:()=>repair}});
 assert.deepEqual(commands.slice(beforeRepair),[{op:'page',page:'connections',device:missing.id}]);
 vm.runInContext('window.prismestraReceive({kind:"error",message:"Fixture return to Devices"})',context);
 snapshot([neighbor,missing],neighbor,'Кадры отправлены: 5');
 assert.equal(find('#native-runtime-detail').textContent,'Кадры отправлены: 5');
 assert.match(mainMarkup,/data-connection-device="neighbor-lcd"/);
 assert.doesNotMatch(mainMarkup,/Windows не находит LCD/);
 snapshot([],undefined,'Добавьте устройство из списка поддерживаемых протоколов.');
 assert.doesNotMatch(find('nav').innerHTML,/data-view="lcd"/);
 assert.doesNotMatch(mainMarkup,/data-connection-device|data-view="lcd"|Windows не находит LCD/);
 console.log('PASS: missing LCD guidance refreshes in place, opens only its binding, and stays hidden without LCD');
}

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
 click();
 assert.equal(commands.length,start+1,'a failure alone does not re-enable the last ticket');
 receive({kind:'status',state:{deviceActivity:{retryScope:'before',retryTickets:{},reasons:{}}}});
 assert(help.hidden);
 assert.equal(vm.runInContext('deviceStatus(state.devices[0])',context),'Недоступно');
 receive({kind:'status',state:{deviceActivity:{retryScope:'before',retryTickets:{wled:'first'}}}});
 click();
 assert.equal(commands.length,start+1,'a ticket for another output cannot re-enable the selected one');
 receive({kind:'status',state:{deviceActivity:{retryScope:'before',retryTickets:{rgb:'first'}}}});
 assert.equal(disabled.get('aria-disabled'),'false','the host republished the selected output ticket');
 click();click();
 const republished=commands.at(-1);
 assert.deepEqual(republished,{op:'retry_output',id:'rgb',ticket:'first',request:republished.request});
 assert.equal(commands.length,start+2,'exactly one new attempt uses the republished ticket');
 assert.notEqual(republished.request,first.request);
 reply('rgb',first.request,'before','late rejection');
 assert.equal(status.textContent,'Подключаемся…','the previous request cannot settle the new one');
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

{
 const empty = {...foundState, devices: [], deviceCandidates: [], startupDiscoveryRetry: true};
 vm.runInContext('view="library"',context);
 receiveDeviceState('snapshot', empty);
 assert.ok(mainMarkup.includes('data-action="retry-startup-discovery"'));
 receiveDeviceState('status',{startupDiscoveryRetry:false});
 assert.ok(!mainMarkup.includes('data-action="retry-startup-discovery"'),'running search hides the action');
 receiveDeviceState('status',{startupDiscoveryRetry:true});
 assert.ok(mainMarkup.includes('data-action="retry-startup-discovery"'),'an empty result allows a fresh user action');
 vm.runInContext('view="devices"',context);
 receiveDeviceState('snapshot', empty);
 assert.ok(mainMarkup.includes('data-action="retry-startup-discovery"'));
 const retry = {dataset:{action:'retry-startup-discovery'},disabled:false,hasAttribute:()=>true};
 const clickRetry = () => events.click({target:{closest:()=>retry}});
 const before = commands.length;
 vm.runInContext('formStages.set("pending",{kind:"native",page:"devices",id:714,value:"Keep"})',context);
 clickRetry();
 assert.equal(commands.length,before,'unapplied edits protect the profile from a rescan');
 vm.runInContext('formStages.clear()',context);
 clickRetry();
 clickRetry();
 assert.deepEqual(commands.slice(before),[{op:'retry_startup_discovery'}]);
 receiveDeviceState('status',{startupDiscoveryRetry:false,pending:true,deviceCandidates:[foundRgb]});
 assert.ok(hasCandidate(foundRgb.id));
 assert.match(mainMarkup,/Настраивается автоматически/,'the late proposal is still being applied');
 assert.ok(!mainMarkup.includes('data-action="retry-startup-discovery"'));
 receiveDeviceState('snapshot',{...empty,startupDiscoveryRetry:false,devices:[{...foundRgb,leds:1}]});
 assert.ok(!hasCandidate(foundRgb.id),'committed device replaces the late proposal');
 assert.match(mainMarkup,new RegExp(`data-device-id="${foundRgb.id}"`));
 console.log('PASS: one rescan from an empty profile; late candidate becomes an applied device');
}
{
 const empty={...foundState,pending:false,devices:[],draftDevices:[],deviceCandidates:[],startupDiscoveryRetry:false};
 const summary=find('#startup-empty-search');
 vm.runInContext('view="library";pendingView=null',context);
 receiveDeviceState('snapshot',empty);
 assert.equal(summary.hidden,true);
 receiveDeviceState('status',{startupEmptySearchResult:'continuing',startupDiscoveryRetry:true});
 assert.equal(summary.hidden,false);
 assert.match(summary.textContent,/пока не найдены.*продолжится в фоне/);
 receiveDeviceState('status',{startupDiscoveryRetry:false});
 assert.equal(summary.hidden,false,'a later scan keeps the first result visible');
 vm.runInContext('view="devices";pendingView=null',context);
 receiveDeviceState('snapshot',{...empty,startupEmptySearchResult:'continuing'});
 assert.equal(summary.hidden,false);
 receiveDeviceState('status',{deviceCandidates:[foundLcd],pending:true});
 assert.equal(summary.hidden,true,'the late proposal replaces the empty result during auto-apply');
 receiveDeviceState('snapshot',{...empty,startupEmptySearchResult:'complete',startupDiscoveryRetry:true});
 assert.equal(summary.hidden,false);
 assert.match(summary.textContent,/Автопоиск завершён.*не найдены/);
 console.log('PASS: a provisional empty result persists between scans and yields to a late candidate or completion');
}
{
 const applied={...foundRgb,leds:1};
 const partial={...initial,devices:[applied],draftDevices:[applied],startupDiscoveryRetry:false,
  nativeAux:{page:'devices',fields:[{id:714,label:'Тип',kind:'select',enabled:true,value:'openrgb',choices:[]}],actions:[],notes:[]},
  editor:{page:'devices',index:0,deviceId:applied.id,targets:[applied.label],fields:[],dirty:false}};
 const hint=/OpenRGB не определил тип.*«Добавить устройство».*«Подключениях»/;
 const guidance=find('#startup-discovery-help');
 vm.runInContext('view="devices";pendingView=null',context);
 receiveDeviceState('snapshot',partial);
 assert.equal(guidance.hidden,true);
 receiveDeviceState('status',{startupDiscoveryUnknownController:true});
 assert.equal(guidance.hidden,false);
 assert.match(mainMarkup,hint);
 assert.match(mainMarkup,/data-device-id=/,'Already applied outputs stay available');
 assert.doesNotMatch(mainMarkup,/data-action="retry-startup-discovery"/);
 const add=new Node();
 add.dataset={action:'focus-device-setup'};
 const before=commands.length;
 events.click({target:{closest:()=>add}});
 assert.equal(document.activeElement,find('[data-native-field="714"]'));
 assert.equal(commands.length,before,'Manual setup starts locally without targeting the working output');
 vm.runInContext('view="devices";pendingView=null',context);
 receiveDeviceState('snapshot',{...partial,startupDiscoveryUnknownController:true});
 assert.equal(guidance.hidden,false);
 receiveDeviceState('status',{startupDiscoveryUnknownController:false});
 assert.equal(guidance.hidden,true,'A new scan or profile can clear stale guidance');
 console.log('PASS: partial startup discovery explains an unknown OpenRGB type and leads to manual setup');
}
{
 const pending={...foundState,pending:true,deviceCandidates:[foundRgb],nativeAux:{
  page:'devices',fields:[{id:714,label:'Семейство',kind:'select',enabled:true,value:'openrgb',choices:[]}],actions:[],notes:[],
 }};
 const before=commands.length;
 receiveDeviceState('snapshot',pending);
 assert.match(mainMarkup,/Prismestra добавляет найденное устройство в профиль/);
 assert.match(mainMarkup,/Настраивается автоматически/);
 assert.doesNotMatch(mainMarkup,/data-action="apply-document"|id="native-target"/);
 receiveDeviceState('status',{pending:false,error:'Таймаут подключения'});
 assert.ok(hasCandidate(foundRgb.id),'the stale discovery row is still present');
 assert.doesNotMatch(mainMarkup,/Настраивается автоматически|Prismestra добавляет/);
 assert.match(mainMarkup,/<button[^>]*data-action="focus-device-setup" data-mutation\s*>Добавить устройство/);
 assert.match(mainMarkup,/id="native-target"/);
 receiveDeviceState('snapshot',{...pending,pending:false});
 assert.doesNotMatch(mainMarkup,/Настраивается автоматически|Prismestra добавляет/);
 assert.match(mainMarkup,/data-action="focus-device-setup"/);
 receiveDeviceState('status',{pending:true});
 assert.match(mainMarkup,/Настраивается автоматически/,'a fresh active transition can show progress again');
 assert.equal(commands.length,before,'candidate arrival must not issue a command');
 receiveDeviceState('snapshot',{...pending,editor:{...pending.editor,dirty:true}});
 assert.match(mainMarkup,/id="native-target"/,'existing edits keep their form');
 receiveDeviceState('snapshot',{...pending,pending:false,deviceCandidates:[],error:'Подключение не применилось'});
 assert.doesNotMatch(mainMarkup,/Настраивается автоматически/,'cancelled discovery restores the empty state');
 assert.match(mainMarkup,/<button[^>]*data-action="focus-device-setup" data-mutation\s*>Добавить устройство/,
  'a failed setup enables manual device setup');
 receiveDeviceState('snapshot',{...pending,pending:false,deviceCandidates:[],devices:[{...foundRgb,leds:1}],
  editor:{page:'devices',index:0,targets:[foundRgb.label],fields:[],dirty:false}});
 assert.match(mainMarkup,/id="native-target"/,'properties return after the output is applied');
 assert.doesNotMatch(mainMarkup,/Настраивается автоматически/);
}
console.log('PASS: first discovered output presents automatic setup until it becomes configurable');

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
vm.runInNewContext(outputMapSource, {window: mapWindow, document: mapDocument, ui: ru=>ru, english:false});
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
assert.deepEqual(commands.at(-1),{op:'editor_preview_visible',visible:false});
const draftA=commands.at(-2);
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

{
 vm.runInContext('formStages.clear();view="library";pendingView=null',context);
 const lcdOnly=navFixture([navLcd]);
 receiveDeviceState('snapshot',lcdOnly);
 assert.match(mainMarkup,/В профиле только LCD.*отдельно от RGB-сцен/);
 assert.match(mainMarkup,/<button class="button primary" data-view="lcd">Настроить LCD<\/button>/);
 openView('lcd');
 assert.deepEqual(commands.at(-1),{op:'page',page:'lcd'});
 receiveDeviceState('snapshot',navFixture([navLcd],{editor:{page:'lcd',index:0,targets:['LCD'],fields:[],dirty:false}}));
 assert.equal(viewNow(),'lcd');
 openView('library');
 receiveDeviceState('snapshot',navFixture([navLcd,navAmbient]));
 assert.doesNotMatch(mainMarkup,/В профиле только LCD|data-view="lcd">Настроить LCD/);
 console.log('PASS: an LCD-only profile offers its settings from the startup library');
}

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
 assert.match(mainMarkup,/<button[^>]+data-apply="night"[^>]*>Применить сцену<\/button>/);
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
 assert.match(status.textContent,/Применяем/);
 assert.equal(devices.hidden,true);
 receiveDeviceState('snapshot',{...libraryState,scene:entry.scene});
 receiveDeviceState('status',{scene:entry.scene,pending:false});
 assert.match(status.textContent,/Применяем/,'snapshots and unrelated status cannot confirm even the same scene');
 const reply=(request,result,details)=>vm.runInContext(
  'window.prismestraReceive('+JSON.stringify({kind:'scene_activation',request,status:result,...details})+')',context);
 reply(apply.request+1,'applied',{output:'active_rgb'});
 assert.match(status.textContent,/Применяем/);
 reply(apply.request,'failed',{message:'Устройство недоступно.'});
 assert.match(status.textContent,/Не удалось применить.*Устройство недоступно/);
 assert.equal(devices.hidden,true);
 click('apply');
 const second=commands.at(-1);
 assert.notEqual(second.request,apply.request);
 vm.runInContext('window.prismestraReceive({kind:"error",message:"Ошибка другой команды"})',context);
 assert.equal(find('#native-error').hidden,false);
 assert.match(find('#native-error').innerHTML,/Ошибка другой команды/);
 assert.match(status.textContent,/Применяем/,'an unrelated error cannot settle the activation');
 click('apply');
 assert.equal(commands.at(-1),second,'an unrelated error cannot unlock another activation');
 reply(apply.request,'applied',{output:'active_rgb'});
 assert.match(status.textContent,/Применяем/);
 reply(second.request,'applied',{output:'no_active_rgb'});
 assert.equal(status.textContent,'Сцена «Ночная» применена. Нет активных RGB-выходов для этой сцены.');
 assert.equal(find('#native-error').hidden,false,'the scene result does not dismiss the unrelated error');
 assert.equal(devices.hidden,false);
 click('apply');
 reply(commands.at(-1).request,'applied',{output:'active_rgb'});
 assert.equal(status.textContent,'Сцена «Ночная» применена.');
 receiveDeviceState('status',{native:false});
 assert.equal(devices.hidden,true,'hardware status is not offered in preview');
 click('apply');
 reply(commands.at(-1).request,'applied',{output:'preview_only'});
 assert.equal(status.textContent,'Сцена «Ночная» применена. Режим предпросмотра.');
 assert.equal(viewNow(),'library');
 assert.equal(vm.runInContext('draft.brightness',context),65,'activation does not replace the editor draft');
}
console.log('PASS: scene activation survives an unrelated error, ignores stale replies, and settles only on matching rejection or success');

{
 const controller={id:'argb-controller',label:'Контроллер ARGB',role:'canvas',driver:{type:'openrgb'},leds:1,rate_hz:25,enabled:true};
 const editor={page:'devices',index:0,targets:[controller.label],fields:[],dirty:false};
 const snapshot={...initial,devices:[controller],draftDevices:[controller],editor};
 vm.runInContext('view="devices";pendingView=null',context);
 receiveDeviceState('snapshot',snapshot);
 assert.doesNotMatch(mainMarkup,/id="open-argb"/);
 receiveDeviceState('status',{argbDevice:controller.id});
 assert.match(mainMarkup,/id="open-argb"[^>]*>Каналы ARGB/);
 let reply, save;
 context.window.argbChannel={
  open(device,label,api){
   assert.equal(label,controller.label);
   save=api.save;
   return api.list({device,request:31});
  },
  receive(message){reply=message;},
 };
 events.click({target:{closest:()=>({
  dataset:{action:'open-argb'},hasAttribute:name=>name==='data-mutation',
 })}});
 assert.deepEqual(commands.at(-1),{op:'argb_channels',device:controller.id,request:31});
 save({device:controller.id,request:32,revision:'confirmed-inventory',zone:0,leds:27});
 assert.deepEqual(commands.at(-1),{op:'argb_channel_save',device:controller.id,request:32,revision:'confirmed-inventory',zone:0,leds:27});
 const inventory={kind:'argb_channels',device:controller.id,request:31,status:'ready',writable:false,channels:[]};
 vm.runInContext('window.prismestraReceive('+JSON.stringify(inventory)+')',context);
 assert.deepEqual(JSON.parse(JSON.stringify(reply)),inventory);
 const rejected={kind:'argb_channel_saved',device:controller.id,request:32,revision:'confirmed-inventory',status:'failed'};
 vm.runInContext('window.prismestraReceive('+JSON.stringify(rejected)+')',context);
 assert.deepEqual(JSON.parse(JSON.stringify(reply)),rejected);
 receiveDeviceState('status',{argbDevice:null});
 assert.doesNotMatch(mainMarkup,/id="open-argb"/);
}
console.log('PASS: managed ARGB editor routes typed inventory/save IPC and correlated save errors');

{
 const controller={label:'<Controller 0/0>',channels:[{id:'0',label:'Header A',mode:'manual',leds:0,min:1,max:64},{id:'1',label:'Header B',mode:'manual',leds:0,min:1,max:120}]};
 vm.runInContext('view="devices";pendingView=null',context);
 receiveDeviceState('snapshot',{...initial,devices:[],draftDevices:[],argbDevice:null});
 for(const id of ['channel','template','dots']){
  const field=find(`#argb-${id}`);
  field.replaceChildren=(...children)=>{field.children=children;};
 }
 vm.runInContext(fs.readFileSync(path.join(__dirname,'..','src','ui','windows','web','argb-channel.js'),'utf8'),context);
 receiveDeviceState('status',{unconfiguredArgb:[controller]});
 assert.match(mainMarkup,/&lt;Controller 0\/0&gt;/);
 assert.match(mainMarkup,/id="preview-argb-0"/);
 assert.doesNotMatch(mainMarkup,/data-output-status|id="open-argb"/);
 const before=commands.length;
 events.click({target:{closest:()=>({dataset:{action:'preview-argb',argbIndex:'0'},hasAttribute:()=>false})}});
 assert.equal(find('#argb-dialog').open,true);
 assert.equal(find('#argb-label').textContent,controller.label);
 const channel=find('#argb-channel');
 assert.deepEqual(channel.children.slice(1).map(item=>item.textContent),['Header A','Header B']);
 channel.value='1';
 channel.onchange();
 assert.equal(find('#argb-count').max,120);
 assert.equal(find('#argb-count').value,'');
 assert.equal(find('#argb-save').hidden,true);
 find('#argb-form').onsubmit({preventDefault(){}});
 assert.equal(commands.length,before,'the unbound preview sends no IPC');
 find('#argb-close').onclick();
 receiveDeviceState('status',{unconfiguredArgb:[]});
 assert.doesNotMatch(mainMarkup,/id="preview-argb-0"/);
}
console.log('PASS: an unconfigured ARGB controller opens a local preview without a binding or IPC');

{
 restoreDraft(draftState);
 const submitNew=()=>{
  evaluate('nameDialog("new")');
  find('#scene-name').value='New fixture';
  find('#name-form').onsubmit({preventDefault(){}});
  return commands.at(-1);
 };
 assert.deepEqual(submitNew(),{op:'save_scene',id:null,name:'New fixture',scene});
 evaluate('window.prismestraReceive(payload)');
 assert.equal(evaluate('view'),'library','an unrelated snapshot cannot open the editor');
 evaluate('window.prismestraReceive({kind:"error",message:"Cannot write catalog"})');
 assert.equal(evaluate('view'),'library');
 assert.equal(evaluate('busy()'),false);
 submitNew();
 const firstSave=commands.length;
 const created={id:'created',name:'New fixture',scene};
 context.payload={kind:'snapshot',state:{...draftState,library:[entryA,entryB,created]}};
 evaluate('window.prismestraReceive(payload)');
 assert.equal(evaluate('view'),'editor');
 assert.equal(evaluate('selectedId'),created.id);
 assert.equal(document.activeElement,find('#scene-effect'));
 assert.equal(commands.at(-1).origin.id,created.id);
 assert.equal(commands.slice(firstSave).some(command=>command.op==='apply_scene'),false);
}
console.log('PASS: new scene opens its editor and focuses the effect only after a confirmed save');

{
 const templateState={...draftState,library:[plumDawn]};
 restoreDraft(templateState);
 selectScene(plumDawn.id);
 evaluate('nameDialog("clone")');
 find('#scene-name').value='My plum template';
 find('#name-form').onsubmit({preventDefault(){}});
 assert.deepEqual(commands.at(-1),{op:'save_scene',id:null,name:'My plum template',scene:plumDawn.scene});

 context.payload={kind:'snapshot',state:templateState};
 evaluate('window.prismestraReceive(payload)');
 assert.equal(evaluate('selectedId'),null);
 assert.equal(evaluate('draftName'),plumDawn.name);

 const copy={id:'plum-copy',name:'My plum template',scene:plumDawn.scene};
 const beforeAck=commands.length;
 context.payload={kind:'snapshot',state:{...templateState,library:[plumDawn,copy]}};
 evaluate('window.prismestraReceive(payload)');
 assert.equal(evaluate('view'),'editor');
 assert.equal(evaluate('selectedId'),copy.id);
 assert.equal(document.activeElement,find('#scene-effect'));
 assert.equal(commands.at(-1).origin.id,copy.id);
 assert.equal(commands.slice(beforeAck).some(command=>command.op==='apply_scene'),false);
}
console.log('PASS: copying a built-in scene focuses the effect on the confirmed copy');

const lcdBackground = {
 page:'lcd', index:0, deviceId:navLcd.id, targets:['LCD'], dirty:false,
 fields:[{key:'media',label:'Фон LCD',value:'Выбран · 12345678',choices:[],hint:'',file:true}],
};
vm.runInContext('formStages.clear();view="lcd";pendingView=null',context);
receiveDeviceState('snapshot',navFixture([navLcd],{editor:lcdBackground}));
const backgroundInput=find('#field-0');
backgroundInput.value='';
events.input({target:backgroundInput});
events.click({target:{closest:()=>({dataset:{action:'apply-document'},disabled:false,hasAttribute:()=>true})}});
assert.deepEqual(commands.at(-1),{op:'apply_fields',fields:[{key:'media',value:''}]});
console.log('PASS: clearing the LCD background sends an explicit empty field in Apply');

{
 const library=Array.from({length:100},(_,i)=>({
  id:'search-'+i,name:i===99?'Ёлка 099':`Evening ${i}`,
  scene:{...scene,effect:i===99?'gradient_wave':'breathing'},
 }));
 const effects=[{id:'gradient_wave',name:'Градиентная волна'},{id:'breathing',name:'Дыхание'}];
 vm.runInContext('formStages.clear();view="library";pendingView=null;libraryEffect="";search=""',context);
 receiveDeviceState('snapshot',{...initial,library,effects});
 const beforeSearch=commands.length;
 const query=value=>{const input=find('#search');input.id='search';input.value=value;events.input({target:input});};
 for(const value of ['ГРАДИЕНТНАЯ волна','gradient_wave','елка gradient wave']){
  query(value);
  assert.match(mainMarkup,/data-preview="search-99"/,value);
  assert.doesNotMatch(mainMarkup,/data-preview="search-98"/,value);
 }
 query('evening 98');
 assert.match(mainMarkup,/data-preview="search-98"/);
 query('gradient wave');
 events.change({target:{id:'library-effect',value:'breathing'}});
 assert.match(mainMarkup,/Сцен не найдено/);
 assert.equal(commands.length,beforeSearch,'search and filtering stay local');
}
console.log('PASS: bilingual effect and scene search across 100 saved scenes without IPC');

{
 const assignments={key:'sources',label:'Показатели и подписи',value:'ram=RAM',choices:[],hint:'',file:false};
 const choice={key:'add_source',label:'Добавить показатель',value:'',choices:[{label:'Выберите источник…',value:''},{label:'a',value:'a'}],hint:'',file:false};
 const editor={...lcdBackground,fields:[assignments,choice]};
 vm.runInContext('formStages.clear();view="lcd";pendingView=null',context);
 receiveDeviceState('snapshot',navFixture([navLcd],{editor}));
 const sourceInput=find('#field-0');
 sourceInput.value='ram=Память';
 events.input({target:sourceInput});
 const select=new Node();
 select.tagName='SELECT';
 select.dataset.field='add_source';
 select.value='a';
 events.change({target:select});
 assert.deepEqual(commands.at(-1),{op:'set_field',key:'add_source',value:'a',fields:[{key:'sources',value:'ram=Память'}]});
 const fields=[{...assignments,value:'ram=Память; a=A'},{...choice,choices:[{label:'Все источники добавлены',value:''}]}];
 receiveDeviceState('snapshot',navFixture([navLcd],{editor:{...editor,fields,dirty:true}}));
 assert.equal(find('#field-0').value,'ram=Память; a=A');
 assert.match(mainMarkup,/data-field="add_source"[^>]*>\s*<option value="" selected>Все источники добавлены/);
}
console.log('PASS: LCD source choice preserves staged labels and refreshes the editable assignments');

async function checkOutputStatusCopy() {
 const [selected,neighbor]=['private-output-id','private-neighbor-id'].map(id=>({
  id,label:'Private room',role:'canvas',driver:{type:'open_rgb',endpoint:'192.0.2.42:6742'},
  leds:56,rate_hz:48,enabled:true
 }));
 const privateError='Failed to open C:\\Users\\Fixture\\profile.json with token fixture-secret';
 const snapshot = devices => {
  const index=devices.indexOf(selected);
  receiveDeviceState('snapshot',{
   ...initial,devices,draftDevices:devices,detail:privateError,error:privateError,
   editor:{page:'devices',deviceId:selected.id,index,targets:devices.map(({label})=>label),fields:[],dirty:false},
   deviceActivity:{labels:{[selected.id]:'Кадры отправлены',[neighbor.id]:'Кадры отправлены'},
    framesSent:{[selected.id]:'7',[neighbor.id]:'100'}}
  });
 };
 const writes=[];
 let denied=false;
 context.navigator={clipboard:{writeText:async text=>{
  if(denied)throw new Error(privateError);
  writes.push(text);
 }}};
 const feedback=find('#output-copy-result');
 const click = async id => {
  const button=new Node();
  button.dataset={action:'copy-output-status',copyOutput:id};
  button.focus();
  events.click({target:{closest:()=>button}});
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(document.activeElement,button);
 };
 vm.runInContext('formStages.clear();view="devices";pendingView=null',context);
 snapshot([neighbor,selected]);
 assert.match(mainMarkup,/data-copy-output="private-output-id">Скопировать статус/);
 assert.doesNotMatch(mainMarkup,/data-copy-output="private-neighbor-id"/);
 const beforeCopy=commands.length;
 await click(neighbor.id);
 assert.deepEqual(writes,[],'stale or neighboring action cannot copy');
 await click(selected.id);
 assert.deepEqual(writes,['Транспорт: OpenRGB\nСостояние: Кадры отправлены\nОтправлено кадров: 7']);
 assert.equal(feedback.textContent,'Статус скопирован');
 snapshot([selected,neighbor]);
 receiveDeviceState('status',{deviceActivity:{labels:{[selected.id]:'Кадры отправлены'},
  framesSent:{[selected.id]:'18446744073709551615',[neighbor.id]:'100'}}});
 await click(selected.id);
 assert.match(writes.at(-1),/кадров: 18446744073709551615$/);
 receiveDeviceState('status',{deviceActivity:{labels:{[selected.id]:'Кадры отправлены'},
  framesSent:{[selected.id]:'7\n'+privateError}}});
 await click(selected.id);
 assert.equal(writes.at(-1),'Транспорт: OpenRGB\nСостояние: Кадры отправлены');
 receiveDeviceState('status',{deviceActivity:{labels:{[selected.id]:privateError},
  reasons:{[selected.id]:'endpointLeaseBusy'},framesSent:{[neighbor.id]:'100'}}});
 await click(selected.id);
 assert.equal(writes.at(-1),'Транспорт: OpenRGB\nСостояние: Подключение занято другим выходом Prismestra');
 receiveDeviceState('status',{deviceActivity:{labels:{[selected.id]:privateError}}});
 await click(selected.id);
 assert.equal(writes.at(-1),'Транспорт: OpenRGB\nСостояние: Нет данных о состоянии');
 denied=true;
 await click(selected.id);
 assert.equal(feedback.textContent,'Не удалось скопировать статус');
 assert.equal(commands.length,beforeCopy,'copy issues no host commands');
 receiveDeviceState('snapshot',{...initial,draftDevices:[selected],
  editor:{page:'devices',deviceId:selected.id,index:0,targets:[selected.label],fields:[],dirty:true}});
 assert.doesNotMatch(mainMarkup,/data-copy-output/,'draft-only output has no runtime status to copy');
 console.log('PASS: selected runtime status copies only allowlisted text and its validated counter, with local clipboard feedback');

{
 const canvasPlacement={width:13,height:11,objects:[{id:'strip',label:'Strip <1>',bounds:[1,1,2,1]}]};
 restoreDraft({...draftState,canvasPlacement});
 evaluate('view="editor";pendingView=null;commandPending=false;awaitingCommandAck=false;openPlacement()');
 assert.deepEqual(commands.at(-1),{op:'page',page:'devices'});
 context.payload={kind:'snapshot',state:{...draftState,canvasPlacement,draftMapRevision:'7',editor:{page:'devices',dirty:false}}};
 evaluate('window.prismestraReceive(payload)');
 assert.match(evaluate('renderPlacementObjects()'),/Strip &lt;1&gt;/);
 assert.doesNotMatch(evaluate('renderPlacementControls()'),/id="placement-picker"/);
 canvasPlacement.objects.push({id:'overlap',label:'Strip <1>',bounds:[1,1,2,1]});
 events.change({target:{id:'placement-picker',value:'overlap'}});
 assert.match(evaluate('renderPlacementControls()'),/value="overlap" selected>Strip &lt;1&gt; · overlap/);
 assert.match(evaluate('renderPlacementObjects()'),/aria-label="Strip &lt;1&gt; · strip"/);
 const delta=(x,y)=>JSON.parse(evaluate(`JSON.stringify(placementDelta({x:100,y:100,cellX:10,cellY:20,minX:-1,maxX:3,minY:-2,maxY:1},${x},${y}))`));
 assert.deepEqual(delta(148,185),{dx:3,dy:1,limited:true});
 assert.deepEqual(delta(78,55),{dx:-1,dy:-2,limited:true});
 assert.deepEqual(delta(119,82),{dx:2,dy:-1,limited:false});
 evaluate('movePlacement("strip","7",1,0)');
 assert.deepEqual(commands.at(-1),{op:'move_canvas_device',device:'strip',revision:'7',dx:1,dy:0});
 context.payload.state.editor.dirty=true;
 evaluate('window.prismestraReceive(payload)');
 assert.equal(evaluate('pageHasEdits()'),true);
 evaluate('changeView("library")');
 assert.equal(evaluate('view'),'editor');
 assert.equal(evaluate('send({op:"save"},true)'),false);
}
console.log('PASS: scene placement uses the device draft and guards navigation and profile save until apply or revert');
}
function checkSearchRetryRecovery(testContext, label) {
 const receive=payload=>{
  testContext.payload=payload;
  vm.runInContext('window.prismestraReceive(payload)',testContext);
 };
 const click=action=>events.click({target:{closest:()=>({dataset:{action},hasAttribute:()=>true})}});
 vm.runInContext('formStages.clear();view="library";pendingView=null',testContext);
 receive({kind:'snapshot',state:{...structuredClone(initial),startupDiscoveryRetry:false}});
 receive({kind:'error',code:'startupDiscoveryRetry',message:'Search is busy'});
 const banner=find('#native-error');
 assert.ok(banner.innerHTML.includes(`id="startup-discovery-retry-error">${label}</button>`));
 assert.doesNotMatch(banner.innerHTML,/data-action="retry-document"/);
 const before=commands.length;
 click('retry-startup-discovery');
 assert.equal(commands.length,before,'unavailable discovery does not dispatch');
 receive({kind:'status',state:{startupDiscoveryRetry:true}});
 click('retry-startup-discovery');
 click('retry-startup-discovery');
 assert.deepEqual(commands.slice(before),[{op:'retry_startup_discovery'}]);
 receive({kind:'error',message:'Connection configuration requested recovery'});
 assert.match(banner.innerHTML,/data-action="retry-document"/);
 assert.doesNotMatch(banner.innerHTML,/id="startup-discovery-retry-error"/);
 click('retry-document');
 assert.deepEqual(commands.at(-1),{op:'retry_document'});
}
function checkEnglishApplicationLanguage() {
 checkSearchRetryRecovery(context,'Повторить поиск');
 document.documentElement={lang:'ru'};
 document.activeElement=null;
 const preview=vm.createContext({
  document,window:{addEventListener:(name,fn)=>events[name]=fn,
   ipc:{postMessage:value=>commands.push(JSON.parse(value))},location:{protocol:'http:',search:'?lang=ru'},prismestraLanguage:'en'},
  requestAnimationFrame:fn=>rafQueue.push(fn),CSS:{escape:value=>value},URLSearchParams,
  structuredClone,setTimeout:()=>1,clearTimeout(){},console
 });
 vm.runInContext(source,preview);
 const receive=(kind,state)=> {
  preview.payload={kind,state};
  vm.runInContext('window.prismestraReceive(payload)',preview);
 };
 assert.equal(document.documentElement.lang,'en');
 assert.match(mainMarkup,/Connecting to Prismestra/);
 receive('snapshot',{...initial,startupStatus:'searching',status:'Ищу устройства'});
 assert.match(find('nav').innerHTML,/data-view="devices"[^>]*aria-label="Devices"/);
 assert.match(mainMarkup,/<h1>Scenes<\/h1>/);
 assert.match(find('#player-status').textContent,/Looking for supported devices/);
 events.click({target:{closest:()=>({dataset:{view:'devices'}})}});
 assert.deepEqual(commands.at(-1),{op:'page',page:'devices'});
 receive('snapshot',{...initial,editor:{page:'devices',index:0,targets:[],fields:[]},
  startupStatus:'empty',startupDiscoveryRetry:true,startupEmptySearchResult:'continuing'});
 assert.match(mainMarkup,/<h1>Devices<\/h1>/);
 assert.match(mainMarkup,/data-action="retry-startup-discovery"[^>]*>Find devices/);
 assert.match(find('#startup-empty-search').textContent,/continue in the background/);
 receive('status',{startupStatus:'unconfiguredArgb',startupEmptySearchResult:null,
  status:'Найдены контроллеры ARGB',unconfiguredArgb:[{label:'Empty ARGB',channels:[]}]});
 assert.match(find('#player-status').textContent,/Found ARGB controllers with no LEDs configured/);
 assert.match(mainMarkup,/aria-label="ARGB channels: Empty ARGB">ARGB channels/);
 assert.equal(find('#startup-empty-search').hidden,true);
 const retry={dataset:{action:'retry-startup-discovery'},hasAttribute:()=>true};
 events.click({target:{closest:()=>retry}});
 assert.deepEqual(commands.at(-1),{op:'retry_startup_discovery'});
 preview.payload={kind:'error',code:'startupDiscoveryRetry',message:'Сбой <details>'};
 vm.runInContext('window.prismestraReceive(payload)',preview);
 assert.match(find('#native-error').innerHTML,/Could not start device search.*Technical details.*Сбой &lt;details&gt;/);
 assert.doesNotMatch(find('#native-error').innerHTML,/data-action="retry-document"/);
 receive('status',{startupStatus:'failed',status:'Сбой SDK'});
 assert.match(find('#player-status').textContent,/Automatic setup could not finish/);
 assert.equal(find('#player-status').title,'Сбой SDK');
 receive('status',{startupStatus:null,status:'Сцена сохранена'});
 assert.equal(find('#player-status').textContent,'Scene saved');
 const device={id:'output1',label:'Личный RGB',role:'rgb',enabled:true,driver:{type:'open_rgb'},leds:8,rate_hz:30};
 receive('snapshot',{...initial,devices:[device],deviceActivity:{labels:{output1:'Кадры отправлены'}},
  editor:{page:'devices',index:0,targets:['Личный RGB'],fields:[]}});
 assert.match(mainMarkup,/Личный RGB.*Frames sent/s,'user labels are preserved and output activity is localized');
 events.click({target:{closest:()=>({dataset:{view:'settings'}})}});
 assert.deepEqual(commands.at(-1),{op:'page',page:'settings'});
 receive('snapshot',{...initial,preferredUiLanguage:'en'});
 assert.match(mainMarkup,/Application language.*value="en" lang="en" selected/s);
 const select=find('#ui-language');
 select.id='ui-language';
 select.value='ru';
 const beforeLanguage=commands.length;
 events.change({target:select});
 events.change({target:select});
 assert.deepEqual(commands.slice(beforeLanguage),[{op:'set_ui_language',language:'ru'}]);
 preview.payload={kind:'error',code:'uiLanguage',message:'Write failed'};
 vm.runInContext('window.prismestraReceive(payload)',preview);
 assert.equal(select.value,'en');
 assert.match(find('#native-error').innerHTML,/Could not save the application language/);
 assert.doesNotMatch(find('#native-error').innerHTML,/retry-document/);
 select.value='ru';
 events.change({target:select});
 receive('snapshot',{...initial,preferredUiLanguage:'ru'});
 assert.match(mainMarkup,/value="ru" lang="ru" selected/);
 assert.match(mainMarkup,/Exit Prismestra completely/);
 assert.equal(document.documentElement.lang,'en','the running shell keeps its language until reopen');
 const sample={id:'builtin:blue-hour',builtIn:true,name:'Синий час',scene:initial.scene};
 vm.runInContext('view="library"',preview);
 receive('snapshot',{...initial,library:[sample,{...sample,id:'my-scene',builtIn:false,name:'Моя сцена'}]});
 assert.match(mainMarkup,/Blue hour.*Apply scene.*Моя сцена/s);
 events.click({target:{closest:()=>({dataset:{apply:sample.id},getAttribute:()=>null})}});
 const activate=commands.at(-1);
 assert.equal(activate.op,'apply_scene');
 preview.payload={kind:'scene_activation',request:activate.request,status:'applied',output:'no_active_rgb'};
 vm.runInContext('window.prismestraReceive(payload)',preview);
 assert.match(find('#scene-activation-'+sample.id).textContent,/applied.*No active RGB outputs/);
 checkSearchRetryRecovery(preview,'Retry search');
 console.log('PASS: English native bootstrap, scenes, device status and application language persistence IPC');
 console.log('PASS: rejected discovery retries only discovery in Russian and English; other errors retain document recovery');
}
checkOutputStatusCopy().then(checkEnglishApplicationLanguage).catch(error=>{console.error(error);process.exitCode=1;});
