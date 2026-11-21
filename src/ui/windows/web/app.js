'use strict';
const $ = (selector, root = document) => root.querySelector(selector);
const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const clone = value => structuredClone(value);
const same = (a,b) => JSON.stringify(a) === JSON.stringify(b);
const hex = color => '#' + (Number(color) >>> 0).toString(16).padStart(6,'0').slice(-6);
// Also read by the portable scene layer dialog.
let effects = Object.create(null);
let audioEffects = new Set();
let contextEffects = new Set();
let reactiveEffects = new Map();
let directEffects = new Set();
const effectLabel = id => effects[id] ?? id;
const effectOption = (id, selected) => `<option value="${esc(id)}" ${id===selected?'selected':''}>${esc(effectLabel(id))}</option>`;
const pages = {library:'Сцены',editor:'Редактор',devices:'Устройства',lcd:'LCD',ambient:'Экран',settings:'Настройки',temperatures:'Температуры',connections:'Подключения',sources:'Источники',schedule:'Расписание'};
const settingsPages = ['temperatures','connections','sources','schedule'];
const devicePresetField = 714;
const glyphs = {
 library:'<rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/>',
 editor:'<path d="M7 3v6m0 4v8M17 3v11m0 4v3M4 9h6v4H4zm10 5h6v4h-6z"/>',
 devices:'<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M10 10h4v4h-4zM9 6V3m6 3V3M9 21v-3m6 3v-3M6 9H3m3 6H3m18-6h-3m3 6h-3"/>',
 lcd:'<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M7 10h10M7 14h6"/>',
 ambient:'<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8m-4-4v4M7 10V8h3m4 0h3v2"/>',
 temperatures:'<path d="M9 14.5V5a3 3 0 0 1 6 0v9.5a5 5 0 1 1-6 0ZM12 9v8"/><circle cx="12" cy="17" r="1"/>',
 connections:'<path d="M9 3v5m6-5v5M7 8h10v4a5 5 0 0 1-10 0V8Zm5 9v4"/>',
 sources:'<path d="M13 5h6a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-6M3 12h13m-4-4 4 4-4 4"/>',
 schedule:'<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
 edit:'<path d="m15 4 5 5M4 20l5-1L21 7a2 2 0 0 0-5-5L4 14Z"/>',
 play:'<path d="m9 5 11 7-11 7Z"/>',
 trash:'<path d="M4 7h16M9 7V4h6v3M6 7l1 14h10l1-14M10 11v6m4-6v6"/>',
 plus:'<path d="M12 5v14M5 12h14"/>',
 sun:'<circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2"/>',
};
glyphs.settings='<path d="M4 6h16M4 12h16M4 18h16"/><circle cx="8" cy="6" r="2" fill="var(--ui-sidebar)"/><circle cx="15" cy="12" r="2" fill="var(--ui-sidebar)"/><circle cx="10" cy="18" r="2" fill="var(--ui-sidebar)"/>';
const icon = name => `<svg viewBox="0 0 24 24" aria-hidden="true">${glyphs[name] ?? ''}</svg>`;
let state = null, draft = null, draftOrigin = null, draftName = 'Текущая сцена', selectedId = null, draftSwitchId = null;
let view = 'library', search = '', previewTimer = null, frame = null, commandPending = false, error = '', dialogMode = 'new', dialogReturn = null, savedSceneRequest = null, previewQueued = false, closing = false, nativeAwaitingAck = false, pendingView = null, advancedOpen = false, submittedStages = [], pointerGesture = false, keyGesture = false, composing = false, renderQueued = false, awaitingCommandAck = false, errorIsCommand = false;
const formStages = new Map();
const libraryPageSize = 24;
let libraryLimit = libraryPageSize, libraryEffect = '';
let deviceInspection = null;
let visibleCandidateIds = new Set();
const outputRetries = new Map();
let retryRequest = 0;
const retryScope = () => state?.deviceActivity?.retryScope;
const selectedRetryOutput = () => view==='devices' && state?.editor?.page==='devices'
 ? state.draftDevices?.[state.editor.index]?.id
 : undefined;
function outputRetryTicket(id) {
 if(selectedRetryOutput()!==id)return null;
 const tickets=state?.deviceActivity?.retryTickets;
 const ticket=tickets&&Object.hasOwn(tickets,id)?tickets[id]:null;
 return ticket===outputRetries.get(id)?.ticket?null:ticket;
}
function reconcileRetryActivity(activity) {
 if(!activity)return;
 for(const [id,retry] of outputRetries) {
  const label=activity.labels?.[id];
  if(retry.awaitingActivity && label && label!=='Подключается')retry.awaitingActivity=false;
 }
}
function syncOutputStatuses() {
 document.querySelectorAll('[data-output-status]').forEach(node=>{
  const status=deviceStatus(state?.devices.find(device=>device.id===node.dataset.outputStatus));
  if(node.textContent!==status)node.textContent=status;
 });
}
function syncOutputRetries(keepFocus) {
 document.querySelectorAll('[data-output-retry]').forEach(button=>{
  const id=button.dataset.outputRetry, retry=outputRetries.get(id), ticket=outputRetryTicket(id);
  const status=button.parentElement.querySelector('[data-retry-status]');
  const help=button.parentElement.querySelector('[data-retry-help]');
  const selected=selectedRetryOutput()===id;
  const leaseBusy=selected && state.deviceActivity?.reasons?.[id]==='endpointLeaseBusy' && retry?.status!=='pending';
  const instruction=leaseBusy
   ? 'Подключение было занято другим выходом Prismestra. Дождитесь его отключения. Когда кнопка доступна, нажмите «Повторить подключение».'
   : '';
  if(help.textContent!==instruction)help.textContent=instruction;
  help.hidden=!instruction;
  const preserveFocus=button===document.activeElement||id===keepFocus;
  const message=retry?.status==='pending'?'Подключаемся…':retry?.status==='submitted'&&!preserveFocus?'':retry?.message||'';
  if(status.textContent!==message)status.textContent=message;
  button.setAttribute('aria-disabled',String(busy()||!!pendingView||!ticket||retry?.status==='pending'));
  button.hidden=!selected||(!ticket&&retry?.status!=='pending'&&!preserveFocus);
  button.parentElement.hidden=button.hidden&&!message&&!instruction;
 });
}
function receiveOutputRetry({id,request,scope,status,message}) {
 const retry=outputRetries.get(id);
 if(!retry||scope!==retryScope()||retry.request!==request||retry.status!=='pending')return;
 if(status==='failed')retry.message=message||'Подключение не удалось';
 else if(status==='submitted')retry.message='Кадры отправлены';
 else if(status==='cancelled')retry.message='Подключение отменено';
 else return;
 retry.status=status;
 if(status==='submitted' && state.deviceActivity?.labels?.[id]==='Подключается'){
  retry.awaitingActivity=true;
  syncOutputStatuses();
 }
 syncOutputRetries();
}
let settingsOpener = null, settingsFocusReturn = null;
const busy = () => closing || !state || !!state.pending || commandPending || savedSceneRequest !== null;
const disabled = () => busy() ? 'disabled' : '';
const changed = () => !!draft && !!state && !same(draft,state.scene);
const unsavedDraft = () => !!draft && (!draftOrigin || !same(draft,draftOrigin.scene));
const deviceFocusKey = id => `[data-device-id="${CSS.escape(id)}"]`;
function focusKey(node=document.activeElement) {
 if(!node) return null;
 if(node.id) return '#'+CSS.escape(node.id);
 if(node.dataset?.deviceId!==undefined)return deviceFocusKey(node.dataset.deviceId);
 for(const key of ['view','action','edit','apply','remove','field','param','color','target','group','asset','remove-color','native-field','native-action']) if(node.dataset?.[key] !== undefined) return `[data-${key}="${CSS.escape(node.dataset[key])}"]`;
 return null;
}
function restoreFocus(key, selection) {
 const node = key && $(key);
 if(node && !node.disabled) {node.focus({preventScroll:true}); if(selection && node.setSelectionRange) try{node.setSelectionRange(...selection);}catch{}}
 else if(key) focusHeading();
}
function focusHeading() {const heading=$('h1');if(heading){heading.tabIndex=-1;heading.focus({preventScroll:true});}}
const recoveryOperation = op => ['revert_document','retry_document','classic'].includes(op);
const recoveryAction = action => ['revert-document','retry-document','classic'].includes(action);
function controlDisabled(node) {
 if(closing)return true;
 const code=node.dataset.action;
 if(code==='classic')return !state;
 if(code==='revert-document')return !state || !(busy()||state.editor?.dirty||stagedDirty());
 if(code==='retry-document')return !state || !(busy()||error||state.error);
 if(['insert-palette-color','move-palette-earlier','move-palette-later'].includes(code)){
  if(busy()||!paletteHasOrder(draft))return true;
  if(code==='insert-palette-color')return draft.colors.length>=8;
  return code==='move-palette-earlier'?paletteIndex()===0:paletteIndex()===draft.colors.length-1;
 }
 return busy() || (node.dataset.needsChange==='scene'&&!changed()) || (node.dataset.needsChange==='document'&&!(state?.editor?.dirty||currentGenericStages().length)) || (code==='add-color'&&draft?.colors.length>=8) || (node.dataset.removeColor!==undefined&&draft?.colors.length<=1);
}
const advancedPage = () => ['connections','sources','schedule'].includes(view);
function nativeFormForView() {return advancedPage()?state?.nativeForm:state?.nativeAux;}
const outputPage = page => page==='lcd' || page==='ambient';
const fieldTarget = editor => editor?.deviceId ?? editor?.index;
let renderedOutputEditor = null;
function stageKey(kind,key,editor=state?.editor) {return JSON.stringify(kind==='generic'?[kind,editor?.page,fieldTarget(editor),key]:[kind,view,String(key)]);}
function genericValue(field) {return formStages.get(stageKey('generic',field.key))?.value??String(field.value??'');}
function nativeValue(field) {return formStages.get(stageKey('native',field.id))?.value??String(field.value??'');}
function currentGenericStages() {return (state?.editor?.page===view?state.editor.fields:[]).filter(field=>!field.choices?.length&&formStages.has(stageKey('generic',field.key))).map(field=>({key:field.key,value:genericValue(field)}));}
function currentNativeStages() {return (nativeFormForView()?.page===view?nativeFormForView().fields:[]).filter(field=>field.enabled&&['text','secret'].includes(field.kind)&&formStages.has(stageKey('native',field.id))).map(field=>({id:field.id,value:nativeValue(field)}));}
function stageText(input) {
 if(!input||closing)return false;
 let key,record,baseline;
 if(input.dataset?.nativeField!==undefined&&['text','secret'].includes(input.dataset.nativeKind)){
  const field=nativeFormForView()?.fields.find(field=>String(field.id)===input.dataset.nativeField);if(!field)return false;
  key=stageKey('native',field.id);baseline=field.kind==='secret'?'':String(field.value??'');record={kind:'native',page:view,id:field.id,value:input.value,secret:field.kind==='secret'};
 }else if(input.dataset?.field&&input.tagName==='INPUT'){
  const editor=outputPage(view)?renderedOutputEditor:state?.editor;
  const field=editor?.fields.find(field=>field.key===input.dataset.field);if(!field)return false;
  key=stageKey('generic',field.key,editor);baseline=String(field.value??'');record={kind:'generic',page:editor.page,target:fieldTarget(editor),key:field.key,value:input.value};
 }else return false;
 if(record.value===baseline)formStages.delete(key);else formStages.set(key,record);
 syncControls();return true;
}
function stagedDirty() {return formStages.size>0;}
function currentPageStaged() {return [...formStages.values()].some(record=>record.page===view);}
function detachedOutputEdits() {
 if(!outputPage(view))return false;
 const editor = state?.editor;
 return [...formStages.values()].some(record=>record.kind==='generic' && record.page===view &&
  (editor?.page!==view || fieldTarget(editor)!==record.target));
}
function discardPageFields() {const removed=new Set();for(const [key,record] of formStages)if(record.page===view){formStages.delete(key);removed.add(key);}submittedStages=submittedStages.filter(([key])=>!removed.has(key));render();}
function completeStageAck() {for(const [key,value] of submittedStages)if(formStages.get(key)?.value===value)formStages.delete(key);submittedStages=[];}
function selectTarget(command,input=null) {if(stagedDirty()){if(input)input.value=state.editor.index<state.editor.targets.length?String(state.editor.index):'';showError('Примените или отмените незавершённый ввод перед выбором другого объекта.');return false;}return send(command,true);}
function deferRender() {if(pointerGesture||keyGesture||composing){renderQueued=true;return true;}return false;}
function releaseGesture() {requestAnimationFrame(()=>{if(renderQueued&&!pointerGesture&&!keyGesture&&!composing){renderQueued=false;render();}});}
function nativeControlDisabled(node) {
 const form=nativeFormForView();
 const items=node.dataset.nativeAction!==undefined?form?.actions:form?.fields;
 const id=node.dataset.nativeAction??node.dataset.nativeField;
 const item=items?.find(item=>String(item.id)===id);
 const staged=item?.accepts_staged_text && form.page===view && form.fields.some(field=>
  field.enabled && field.kind==='text' && formStages.has(stageKey('native',field.id)));
 return closing || commandPending || !(item?.enabled || staged);
}
function showError(message,fromCommand=false) {errorIsCommand=fromCommand;error=String(message);const box=$('#native-error');box.hidden=false;box.innerHTML=`<button class="icon-button" data-action="dismiss-error" aria-label="Закрыть сообщение">×</button><p>${esc(error)}</p>${stagedDirty()?'<button class="button" data-action="discard-fields">Отменить незавершённый ввод</button>':''}<button class="button" data-action="retry-document" data-mutation>Повторить</button>`;}
function send(command, mutation=false) {
 if(closing)return false;
 if(outputPage(view)&&['apply_fields','set_field','asset'].includes(command.op)&&
  (detachedOutputEdits() || fieldTarget(renderedOutputEditor)!==fieldTarget(state?.editor)))return false;
 const nativeCommand=['native_field','native_action'].includes(command.op);
 if(nativeCommand){const node={dataset:{[command.op==='native_field'?'nativeField':'nativeAction']:String(command.id)}};if(nativeControlDisabled(node))return false;const items=command.op==='native_field'?nativeFormForView().fields:nativeFormForView().actions;command.id=items.find(item=>String(item.id)===String(command.id)).id;}
 else if(mutation && busy() && !recoveryOperation(command.op)) return false;
 if(['open','save','export'].includes(command.op)&&stagedDirty()){showError('Примените или отмените незавершённый ввод перед сохранением профиля.');return false;}
 const stagedCommand=['apply_fields','set_field','asset','native_field','native_action'].includes(command.op);
 const discardNative=nativeCommand&&['408','1812','1819','1107','1141'].includes(String(command.id));
 if(stagedCommand&&!discardNative){if(nativeCommand){command.fields=currentNativeStages();command.editor_fields=state.pending?[]:currentGenericStages().filter(item=>state.editor.fields.find(field=>field.key===item.key)?.enabled!==false);}else command.fields=currentGenericStages();}
 if(['apply_scene','open','save','export'].includes(command.op))flushPreview(true);
 if(!window.ipc || typeof window.ipc.postMessage !== 'function') {showError('Нет подключения к приложению Prismestra. Откройте интерфейс из нативного приложения.');return false;}
 try {window.ipc.postMessage(JSON.stringify(command));if(mutation||command.op==='page')awaitingCommandAck=true;if(stagedCommand){const keys=discardNative?[...formStages].filter(([,record])=>record.page===view).map(([key])=>key):nativeCommand?[...(command.fields||[]).map(item=>stageKey('native',item.id)),...(command.editor_fields||[]).map(item=>stageKey('generic',item.key))]:(command.fields||[]).map(item=>stageKey('generic',item.key));submittedStages=keys.filter(key=>formStages.has(key)).map(key=>[key,formStages.get(key).value]);if(nativeCommand&&!discardNative){for(const [key] of submittedStages)if(formStages.get(key)?.secret)formStages.delete(key);document.querySelectorAll('[data-native-kind="secret"]').forEach(input=>{if(!formStages.has(stageKey('native',input.dataset.nativeField)))input.value='';});}}if(mutation){commandPending=true;nativeAwaitingAck=nativeCommand||stagedCommand;syncControls();}return true;}catch(e){awaitingCommandAck=false;commandPending=false;showError(e.message);syncControls();return false;}
}
function syncControls(keepRetryFocus) {
 document.querySelectorAll('[data-mutation]').forEach(node=>{
  const disabled=controlDisabled(node);
  // Keep the row focusable across the command acknowledgement and list updates.
  if(node.dataset.deviceId!==undefined)node.setAttribute('aria-disabled',String(disabled));
  else node.disabled=disabled;
 });
 document.querySelectorAll('[data-native-control]').forEach(node=>node.disabled=nativeControlDisabled(node));
 document.querySelectorAll('[data-local-discard]').forEach(node=>{node.hidden=!currentPageStaged();node.disabled=closing||commandPending;});
 $('#master-brightness').disabled=busy()||!draft;
 $('#footer-apply').disabled=busy()||!changed();
 $('#play').disabled=busy();
 const light=!!(state?.groups?.pc||state?.groups?.door);$('#play').textContent=light?'Выключить свет':'Включить свет';
 $('#player-name').textContent=state?effectLabel(state.scene.effect):'Prismestra';
 $('#player-status').textContent=state?.status||'Подключение к приложению…';
 const runtimeDetail=$('#native-runtime-detail');if(runtimeDetail)runtimeDetail.textContent=state?.detail||'';
 const audioSource=$('#scene-audio-source');
 if(audioSource)audioSource.textContent=state?.native
  ? state.audioError&&audioEffects.has(state.scene.effect)?'Системный звук недоступен. Повторяю подключение.'
   : 'Источник — звук, который воспроизводит Windows. Данные появляются после применения при включённом RGB.'
  : 'Системный звук доступен в профиле с локальным RGB-подключением.';
 const reactiveSource=$('#scene-reactive-source');
 if(reactiveSource&&state&&draft){
  const kind=reactiveEffects.get(draft.effect);
  const hints={
   keys:'Источник — нажатия клавиш. Для подсветки используются позиции клавиш полноразмерной ANSI-клавиатуры.',
   camera:'После применения используется первая камера в списке Windows.',
   game:'После применения ожидаются события CS2, Dota 2 или локального адаптера игры.',
   external:draft.effect==='media_ambience'
    ?'Источник — текущий проигрыватель Windows с обложкой или данные локального адаптера.'
    :'После применения ожидаются данные от подключённого локального источника.'
  };
  reactiveSource.textContent=!state.native?'Подключите локальный профиль RGB.':
   state.reactiveError&&reactiveEffects.get(state.scene.effect)===kind?state.reactiveError:
   hints[kind]??'';
 }
 syncOutputStatuses();
 syncOutputRetries(keepRetryFocus);
 $('#save-status').textContent=busy()&&state?'Выполняется…':state?.dirty?'Профиль изменён':'Локальный профиль';
 if(draft){$('#master-brightness').value=draft.brightness;$('#master-value').textContent=draft.brightness+'%';}
 const status=$('.scene-status');if(status)status.textContent=changed()?'Изменения ещё не применены':'Эта версия применена';
 const apply=$('[data-action="apply-scene"]');if(apply)apply.textContent=changed()?'Применить':'Применено';
 if(state){const art=$('#player-art'),ctx=art.getContext('2d');art.setAttribute('aria-label','Палитра применённой сцены');state.scene.colors.forEach((c,i)=>{ctx.fillStyle=hex(c);ctx.fillRect(i*art.width/state.scene.colors.length,0,Math.ceil(art.width/state.scene.colors.length),art.height);});}
 if(state?.error)showError(state.error,true);
}
function palette(scene) {return `<div class="palette-art" aria-label="Палитра сцены">${scene.colors.map(c=>`<span style="background:${hex(c)}"></span>`).join('')}</div>`;}
function heading(title,description,actions='') {
 const sceneFileAction=window.sceneFile&&draft&&['library','editor'].includes(view)
  ?action('Файл сцены','scene-file',false,'id="scene-file-launch"'):'';
 return `<div class="page-heading"><div><h1>${esc(title)}</h1><p>${esc(description)}</p></div><div class="heading-actions">${actions}${sceneFileAction}</div></div>`;
}
function action(label,code,primary=false,extra='') {return `<button class="button ${primary?'primary':''}" data-action="${code}" data-mutation ${disabled()} ${extra}>${label}</button>`;}
function renderLibrary() {
 const query=search.trim().toLocaleLowerCase('ru');
 const available=new Set(state.library.map(item=>item.scene.effect));
 const list=state.library.filter(item=>(!libraryEffect||item.scene.effect===libraryEffect)
  && item.name.toLocaleLowerCase('ru').includes(query));
 const effectIds=[...Object.keys(effects).filter(id=>available.has(id)),...Array.from(available).filter(id=>!Object.hasOwn(effects,id))];
 const options=effectIds.map(id=>effectOption(id,libraryEffect)).join('');
 const cards=list.slice(0,libraryLimit).map(item=>`<article class="scene-card ${same(item.scene,state.scene)?'selected':''}">
   <div class="art-wrap"><button class="scene-preview" data-edit="${esc(item.id)}" aria-label="Открыть: ${esc(item.name)}" data-mutation ${disabled()}>
    ${item.preview?`<canvas data-library-preview="${esc(item.id)}" aria-label="Предпросмотр: ${esc(item.name)}"></canvas>`:palette(item.scene)}
   </button></div>
   <div class="scene-body"><div class="scene-title"><h2>${esc(item.name)}</h2><small class="muted">${item.builtIn?'Шаблон · ':''}${esc(effectLabel(item.scene.effect))}</small></div>
    <div class="card-actions"><button class="button" data-apply="${esc(item.id)}" aria-label="Применить: ${esc(item.name)}" title="Применить" data-mutation ${disabled()}>${icon('play')}</button>
     <button class="button" data-edit="${esc(item.id)}" aria-label="Редактировать: ${esc(item.name)}" data-mutation ${disabled()}>${icon('edit')}</button>
     ${item.builtIn?'':`<button class="icon-button" data-remove="${esc(item.id)}" aria-label="Удалить: ${esc(item.name)}" data-mutation ${disabled()}>${icon('trash')}</button>`}
    </div>
   </div>
  </article>`).join('');
 return heading('Сцены','Сцены сохраняют параметры RGB. Устройства и LCD сохраняются в профиле.',action(icon('plus')+' Новая сцена','new',true))+`
  <div class="toolbar">
   <select id="library-effect" aria-label="Тип эффекта"><option value="">Все эффекты</option>${options}</select>
   <output class="muted" aria-live="polite">Показано ${Math.min(libraryLimit,list.length)} из ${list.length}</output>
   <label class="search"><input id="search" placeholder="Найти сцену" aria-label="Найти сцену" value="${esc(search)}"></label>
  </div>
  <div class="library-grid">${cards||'<div class="empty"><h2>Сцен не найдено</h2><p>Измените запрос или выберите другой тип эффекта.</p></div>'}</div>
  ${list.length>libraryLimit?'<button class="button" id="library-more">Показать ещё</button>':''}`;
}
function range(key,label,min,max,step=1,suffix='') {
 const value=key.startsWith('noise.')?draft.noise[key.slice(6)]:draft[key];
 return `<div class="control-block"><label class="control-label" for="param-${key}"><span>${label}</span><output id="value-${key}">${value}${suffix}</output></label><input id="param-${key}" type="range" data-param="${key}" data-suffix="${suffix}" min="${min}" max="${max}" step="${step}" value="${value}" data-mutation ${disabled()}></div>`;
}
function paletteHasOrder(scene) {
 return scene && scene.effect!=='rainbow_wave' && (scene.effect!=='noise_map'||scene.noise_palette==='custom');
}
function patternScale() {
 if(audioEffects.has(draft.effect)||contextEffects.has(draft.effect))return '';
 const source=reactiveEffects.get(draft.effect);
 if(source&&source!=='keys')return '';
 if(draft.effect==='noise_map'||draft.effect==='breathing')return '';
 return range('rainbow_width',draft.effect==='rainbow_wave'?'Ширина волны':'Масштаб рисунка',1,50);
}
function sceneSourceControls() {
 if(audioEffects.has(draft.effect))return '<p class="muted" id="scene-audio-source"></p>';
 const source=reactiveEffects.get(draft.effect);
 if(source){
  const exportButtons=state?.native?source==='game'
   ?action('Сохранить настройку CS2','export-cs2')+action('Сохранить настройку Dota 2','export-dota2')+action('Сохранить адрес и ключ','export-event-source')
   :source==='external'?action('Сохранить адрес и ключ','export-event-source'):'':'';
  const cadence=['game_resources','game_round','weather_scene'].includes(draft.effect)?range('speed','Скорость',1,100):'';
  return '<p class="muted" id="scene-reactive-source"></p>'+exportButtons+cadence;
 }
 if(draft.effect==='day_night')return '<p class="muted">Источник — местное время с часовым поясом из расписания.</p>';
 if(draft.effect==='thermal_tint'||draft.effect==='thermal_gauge')return '<p class="muted">Источник — самая высокая свежая температура из настроенных источников.</p>';
 const speed=range('speed','Скорость',draft.effect==='breathing'?10:1,draft.effect==='breathing'?200:100);
 return draft.effect==='cpu_pulse'?'<p class="muted">Источник — загрузка CPU в подключённом профиле.</p>'+speed:speed;
}
let palettePosition=0;
function paletteIndex() {return Math.min(palettePosition,draft.colors.length-1);}
function palettePositionLabel(color,index) {return `${index+1} · ${hex(color)}`;}
function paletteOrderMarkup() {
 palettePosition=paletteIndex();
 if(!paletteHasOrder(draft))return '';
 const options=draft.colors.map((color,i)=>
  `<option value="${i}" ${i===palettePosition?'selected':''}>${palettePositionLabel(color,i)}</option>`
 ).join('');
 return `<div class="native-palette-order">
   <div class="native-field">
     <label for="palette-position">Позиция цвета</label>
     <select id="palette-position" data-mutation ${disabled()}>${options}</select>
   </div>
   <div class="native-palette-order-actions" role="group" aria-label="Порядок цветов">
     ${action('Вставить после','insert-palette-color',false,'title="Вставить копию выбранного цвета"')}
     ${action('← Раньше','move-palette-earlier')}
     ${action('Позже →','move-palette-later')}
   </div>
 </div>`;
}
function reorderPalette(actionCode) {
 if(!paletteHasOrder(draft)||busy())return;
 const index=paletteIndex();
 if(actionCode==='insert-palette-color'){
  if(draft.colors.length>=8)return;
  palettePosition=index+1;
  draft.colors.splice(palettePosition,0,draft.colors[index]);
  render();
  $(`#palette-color-${palettePosition}`)?.focus();
 }else{
  const direction=actionCode==='move-palette-earlier'?-1:1;
  const target=index+direction;
  if(target<0||target>=draft.colors.length)return;
  [draft.colors[index],draft.colors[target]]=[draft.colors[target],draft.colors[index]];
  palettePosition=target;
  if(target+direction<0||target+direction>=draft.colors.length)$('#palette-position')?.focus();
  render();
 }
 requestPreview();
}
function renderScene() {
 const effectIds=Object.keys(effects);
 if(!Object.hasOwn(effects,draft.effect))effectIds.unshift(draft.effect);
 return heading(draftName,'Изменения остаются в черновике до нажатия «Применить».',action('Сохранить сцену','save-scene')+action('Создать копию','clone')+(selectedId?action('Переименовать','rename'):'')+action('Применить','apply-scene',true,'data-needs-change="scene"'))+`<p class="scene-status"></p><div class="editor-layout"><section class="preview-panel" aria-label="Предпросмотр"><div class="stage"><canvas id="stage" aria-label="Предпросмотр сцены из движка Prismestra"></canvas><div class="stage-bar"><span class="stage-chip">Предпросмотр</span></div><div class="stage-caption">Кадр движка · черновик</div></div><p class="preview-description">Предпросмотр не меняет подсветку. Выходы RGB и LCD включаются отдельно.</p><div class="native-field"><label for="scene-effect">Эффект</label><select id="scene-effect" data-mutation ${disabled()}>${effectIds.map(id=>effectOption(id,draft.effect)).join('')}</select></div><div class="parameter-panel" style="margin-top:20px"><h2>Выходы</h2><label class="native-group"><input type="checkbox" data-group="pc" ${state.groups.pc?'checked':''} data-mutation ${disabled()}> Основной свет · RGB и LCD</label><label class="native-group"><input type="checkbox" data-group="door" ${state.groups.door?'checked':''} data-mutation ${disabled()}> Дополнительный свет</label><button class="button" data-view="devices">Устройства и свойства</button></div></section><aside class="parameter-panel" id="scene-parameters" aria-label="Параметры сцены">${sceneParameters()}</aside></div>`;
}
function sceneParameters() {
 const noise=draft.effect==='noise_map';
 return `${directEffects.has(draft.effect)?'':`<div class="control-block"><div class="control-label">Палитра <small>${draft.colors.length} / 8</small></div><div class="native-palette">${draft.colors.map((c,i)=>`<div class="native-palette-item"><label class="color-control"><input type="color" id="palette-color-${i}" data-color="${i}" value="${hex(c)}" aria-label="Цвет ${i+1}" data-mutation ${disabled()}></label><button class="icon-button" data-remove-color="${i}" data-mutation aria-label="Удалить цвет ${i+1}" ${draft.colors.length===1?'disabled':disabled()}>×</button></div>`).join('')}<button class="icon-button" data-action="add-color" aria-label="Добавить цвет" data-mutation ${draft.colors.length>=8?'disabled':disabled()}>${icon('plus')}</button></div>${paletteOrderMarkup()}</div>`}${sceneSourceControls()}${range('brightness','Яркость',0,100,1,'%')}<details id="advanced-scene" ${advancedOpen?'open':''}><summary>Дополнительно</summary>${range('fps','Частота кадров',1,144,1,' fps')}${range('temperature','Температура',-100,100)}${range('tint','Оттенок',-100,100)}${patternScale()}${noise?`<div class="native-field control-block"><label for="noise-palette">Палитра шума</label><select id="noise-palette" data-scene-select="noise_palette" data-mutation ${disabled()}>${Object.entries({custom:'Своя палитра',rainbow:'Радуга',inverse_rainbow:'Обратная радуга'}).map(([v,t])=>`<option value="${v}" ${draft.noise_palette===v?'selected':''}>${t}</option>`).join('')}</select></div>${range('noise.frequency','Частота шума',0.0001,0.5,0.0001)}${range('noise.amplitude','Амплитуда',0.0001,5,0.0001)}${range('noise.lacunarity','Лакунарность',0.0001,5,0.0001)}${range('noise.persistence','Затухание',0.0001,5,0.0001)}${range('noise.octaves','Октавы',1,20)}${range('noise.motion','Направление движения',0,3)}${range('noise.motion_speed','Скорость движения',0,99)}`:''}</details>`;
}
function nativeProperties() {
 const editor=state.editor;
 if(!editor || editor.page!==view)return state.nativeAux?.page===view?renderNativeForm(state.nativeAux):'<div class="native-wait">Настройки загружаются…</div>';
 const repair=state.devices.some(device=>device.id===editor.deviceId)
  ? `<button class="button" data-view="connections" data-connection-device="${esc(editor.deviceId)}" data-mutation ${disabled()}>Открыть подключение устройства</button>` : '';
 return `<section class="native-properties"><div class="native-inline"><label for="native-target">Объект</label><select id="native-target" class="native-target" data-target data-mutation ${disabled()}>${view==='devices'&&editor.index>=editor.targets.length?'<option value="" disabled selected>Выберите устройство…</option>':''}${editor.targets.map((name,i)=>`<option value="${i}" ${i===editor.index?'selected':''}>${esc(name)}</option>`).join('')}</select>${repair}</div><div class="parameter-panel"><p class="parameter-note" id="native-runtime-detail"${view==='devices'?'':' role="status"'}>${esc(state.detail||'')}</p><div class="native-form">${editor.fields.map((field,i)=>`<div class="native-field"><label for="field-${i}">${esc(field.label)}</label>${field.choices?.length?`<select id="field-${i}" data-field="${esc(field.key)}" data-mutation ${disabled()}>${field.choices.map(choice=>`<option value="${esc(choice.value)}" ${String(choice.value)===String(field.value)?'selected':''}>${esc(choice.label)}</option>`).join('')}</select>`:`<div class="native-field-file"><input id="field-${i}" data-field="${esc(field.key)}" value="${esc(genericValue(field))}" data-mutation ${disabled()}>${field.file?`<button class="button" data-asset="${esc(field.key)}" data-mutation ${disabled()}>Выбрать…</button>`:''}</div>`}${field.hint?`<small>${esc(field.hint)}</small>`:''}</div>`).join('')||'<p class="muted">Параметров объекта нет.</p>'}</div><div class="native-property-actions">${action('Применить','apply-document',true,'data-needs-change="document"')}${action('Отменить изменения','revert-document')}${action('Повторить','retry-document')}</div></div></section>${state.nativeAux?.page===view?renderNativeForm(state.nativeAux):''}`;
}
function deviceStatus(device) {
 if(!device)return '';
 const labels=state.deviceActivity?.labels;
 if(labels?.[device.id]==='Подключается' && outputRetries.get(device.id)?.awaitingActivity)return 'Кадры отправлены';
 if(labels&&Object.hasOwn(labels,device.id))return labels[device.id];
 return !device.enabled?'Выключено':state.native?'Недоступно':'Предпросмотр';
}
function syncDeviceAnnouncement() {
 if(view!=='devices')deviceInspection=null;
 const region=$('#device-status-announcement');
 const focus=document.activeElement;
 const selected=state?.editor?.page==='devices' && state.draftDevices?.[state.editor.index]?.id;
 const id=deviceInspection?.id;
 const device=id && selected===id &&
  (focus?.dataset?.deviceId===id || focus?.closest?.('#device-properties')) &&
  state.devices.find(device=>device.id===id);
 if(!device){
  if(deviceInspection)deviceInspection.status=null;
  if(region.textContent)region.textContent='';
  return;
 }
 const status=deviceStatus(device);
 if(deviceInspection.status!==null && deviceInspection.status!==status){
  region.textContent=`${device.label}. ${status}.`;
 }
 deviceInspection.status=status;
}
function announceDeviceDiscovery(candidates) {
 const newCandidates=candidates.filter(candidate=>!visibleCandidateIds.has(candidate.id));
 visibleCandidateIds=new Set(candidates.map(candidate=>candidate.id));
 const region=$('#device-discovery-announcement');
 if(!candidates.length){
  if(region.textContent)region.textContent='';
 } else if(newCandidates.length){
  region.textContent=newCandidates.length===1
   ? `Найдено устройство: ${newCandidates[0].label}.`
   : `Найдено устройств: ${newCandidates.length}.`;
 }
}
function pageButton(page, label=pages[page]) {
 return `<button class="button" data-view="${page}">${label}</button>`;
}

function outputPageAvailable(page) {
 return !outputPage(page) || !!state?.devices.some(device=>device.role===page);
}

function renderDevices(candidates) {
 const devices=state.devices.map(d=>{
  const statusId=`device-status-${encodeURIComponent(d.id)}`;
  return `<article class="device">
  <div class="device-symbol">${icon('devices')}</div>
  <div class="device-info"><h2>${esc(d.label)}</h2><p>${esc(d.driver?.type)} · ${esc(d.role)}</p></div>
  <div class="device-meta"><span>${esc(d.leds)} LED · ${esc(d.rate_hz)} Гц</span><small id="${statusId}" data-output-status="${esc(d.id)}">${esc(deviceStatus(d))}</small></div>
  <button class="icon-button" data-device-id="${esc(d.id)}" data-mutation aria-disabled="${busy()}" aria-label="Свойства: ${esc(d.label)}" aria-describedby="${statusId}">${icon('edit')}</button>
  <div class="device-retry" hidden><span id="${statusId}-retry-help" data-retry-help hidden></span><button class="button" id="${statusId}-retry" data-output-retry="${esc(d.id)}" aria-describedby="${statusId}-retry-help ${statusId}-retry-status" aria-disabled="true" hidden>Повторить подключение</button><span id="${statusId}-retry-status" data-retry-status role="status"></span></div>
 </article>`;
 });
 const found=candidates.map(candidate=>`<article class="device" data-candidate-id="${esc(candidate.id)}">
  <div class="device-symbol">${icon('devices')}</div>
  <div class="device-info"><h2>${esc(candidate.label)}</h2><p>${esc(candidate.role)}</p></div>
  <div class="device-meta"><small>Найдено</small></div>
 </article>`);
 const entries=[...devices,...found].join('')||'<div class="empty"><h2>В профиле нет устройств</h2><p>Добавьте устройство в свойствах профиля.</p></div>';
 const add = state.nativeAux?.page==='devices' && state.nativeAux.fields.some(field=>field.id===devicePresetField && field.enabled)
  ? action('Добавить устройство','focus-device-setup',true)
  : '';
 const shortcuts = add + pageButton('connections')
  + (outputPageAvailable('lcd') ? pageButton('lcd','Настроить LCD') : '')
  + (outputPageAvailable('ambient') ? pageButton('ambient','Настроить экран') : '');
 return heading('Устройства','Изменения действуют после «Применить». Профиль сохраняется отдельно.',shortcuts)
  + `<div class="device-list">${entries}</div><div id="device-properties" style="margin-top:24px">${nativeProperties()}</div>`;
}
function renderNativeForm(form=nativeFormForView()) {
 if(!form||form.page!==view)return '<div class="native-wait" role="status">Загрузка настроек…</div>';
 return `<section class="native-properties"><div class="parameter-panel"><div class="native-form">${form.fields.map((field,index)=>{
  const id='native-field-'+index,common=`id="${id}" data-native-field="${esc(field.id)}" data-native-kind="${esc(field.kind)}" data-native-control ${nativeControlDisabled({dataset:{nativeField:String(field.id)}})?'disabled':''}`;
  if(field.kind==='checkbox')return `<label class="native-group" for="${id}"><input type="checkbox" ${common} ${field.value==='true'?'checked':''}>${esc(field.label)}</label>`;
  const control=field.kind==='select'?`<select ${common}>${!field.choices.some(choice=>String(choice.value)===String(field.value))?'<option value="" disabled selected>Выберите…</option>':''}${field.choices.map(choice=>`<option value="${esc(choice.value)}" ${String(choice.value)===field.value?'selected':''}>${esc(choice.label)}</option>`).join('')}</select>`:`<input ${common} type="${field.kind==='secret'?'password':'text'}" autocomplete="off" value="${field.kind==='secret'?'':esc(nativeValue(field))}">`;
  return `<div class="native-field"><label for="${id}">${esc(field.label)}</label>${control}</div>`;
 }).join('')}</div><div class="native-property-actions">${form.actions.map(item=>`<button class="button" data-native-action="${esc(item.id)}" data-native-control ${nativeControlDisabled({dataset:{nativeAction:String(item.id)}})?'disabled':''}>${esc(item.label)}</button>`).join('')}<button class="button" data-action="discard-page-fields" data-local-discard ${currentPageStaged()?'':'hidden'}>Отменить незавершённый ввод</button></div>${form.notes.map(note=>`<p class="parameter-note">${esc(note)}</p>`).join('')}</div></section>`;
}
function renderSettings() {
 const schedule = state.schedule?.enabled
  ? (state.schedule.paused ? 'Расписание на паузе' : 'Расписание активно')
  : 'Расписание выключено';
 return heading('Настройки','Настройки локального профиля.')
  + `<div class="settings-sections">
    <section class="parameter-panel"><h2>Диагностика</h2><div class="settings-links">${pageButton('temperatures')}${pageButton('connections')}${pageButton('sources','Источники температур')}</div></section>
    <section class="parameter-panel"><h2>Расписание</h2><p class="parameter-note">${schedule}</p><div class="settings-links">${pageButton('schedule','Управлять расписанием')}</div></section>
  </div>`;
}

function renderProfilePage() {
 if (view==='settings') return renderSettings();
 if (outputPage(view)) {
  if(detachedOutputEdits()){
   return heading(pages[view],'Выход больше не выбран. Незавершённый ввод сохранён для этого выхода.')
    + `<div class="native-property-actions">${action('Отменить незавершённый ввод','discard-page-fields')}</div>`;
  }
  const available = outputPageAvailable(view);
  const actions = (available ? pageButton('connections','Проверить подключение') : '')
   + (available && view==='lcd' ? pageButton('temperatures') : '') + pageButton('devices','К устройствам');
  return heading(pages[view],available?'Настройки локального профиля.':'В применённом профиле нет такого выхода.',actions)
   + nativeProperties();
 }
 return heading(pages[view],'Настройки локального профиля.',pageButton('settings','К настройкам'))
  + (advancedPage()?renderNativeForm():nativeProperties());
}

function pageHasEdits() {
 return stagedDirty() || (state?.nativeForm?.page===view&&state.nativeForm.dirty)
  || (state?.editor?.page===view&&state.editor.dirty);
}

function render() {
 if(deferRender())return;renderQueued=false;
 if(!busy()&&!pendingView&&!pageHasEdits()&&!outputPageAvailable(view)){
  changeView('devices');
  return;
 }
 const key=focusKey(),node=document.activeElement,selection=typeof node?.selectionStart==='number'?[node.selectionStart,node.selectionEnd]:null;
 const headingFocused = node === $('#main h1');
 const returnKey = view==='settings' && headingFocused ? settingsFocusReturn : null;
 settingsFocusReturn = null;
 const hiddenNavFocused = node?.closest?.('nav') && !outputPageAvailable(node.dataset.view);
 renderedOutputEditor = outputPage(view)&&state?.editor?.page===view?state.editor:null;
 const selectedPage = settingsPages.includes(view)?'settings':view;
 const appliedIds = new Set(view==='devices' ? state?.devices.map(device=>device.id) : []);
 const candidates = view==='devices' ? (state?.deviceCandidates??[]).filter(candidate=>!appliedIds.has(candidate.id)) : [];
 $('nav').innerHTML=Object.entries(pages).filter(([key])=>!settingsPages.includes(key)&&outputPageAvailable(key)).map(([key,title])=>`<button data-view="${key}" class="${selectedPage===key?'active':''}" aria-label="${title}" title="${title}" ${selectedPage===key?'aria-current="page"':''}>${icon(key)}<span>${title}</span></button>`).join('');
 if(!state)$('#main').innerHTML='<div class="native-wait"><h1>Подключение к Prismestra…</h1><p>Ожидание локального профиля.</p></div>';
 else {$('#main').innerHTML=view==='library'?renderLibrary():view==='editor'?renderScene():view==='devices'?renderDevices(candidates):renderProfilePage();$('.project-context').innerHTML=`<span>Локальный профиль</span><small title="${esc(state.profilePath||'')}">${esc(state.profilePath?.split(/[\\/]/).pop()||'Не сохранён')}</small>`;}
 document.querySelectorAll('[data-native-kind="secret"]').forEach(input=>{const staged=formStages.get(stageKey('native',input.dataset.nativeField));if(staged)input.value=staged.value;});
 if(view==='devices')window.outputMap?.mount(state,send,currentGenericStages);
 syncControls(node?.dataset?.outputRetry);paintPreview();paintLibraryPreviews();
 if(returnKey)restoreFocus(returnKey);
 else if(headingFocused || (node?.dataset?.field && detachedOutputEdits()))focusHeading();
 else if(hiddenNavFocused)$('nav [data-view="devices"]')?.focus({preventScroll:true});
 else restoreFocus(key,selection);
 announceDeviceDiscovery(candidates);
 syncDeviceAnnouncement();
}
function flushPreview(force=false) {
 clearTimeout(previewTimer);previewTimer=null;
 if(!draft||(!previewQueued&&!force))return;
 previewQueued=true;
 if(send({op:'preview_scene',scene:draft,origin:draftOrigin}))previewQueued=false;
}
function requestPreview() {if(closing)return;previewQueued=true;clearTimeout(previewTimer);previewTimer=setTimeout(()=>flushPreview(),90);}
function editDraft() {syncControls();requestPreview();}
function paintPixels(canvas,pixelsFrame) {
 if(!canvas || !pixelsFrame || !Number.isInteger(pixelsFrame.width)||!Number.isInteger(pixelsFrame.height)||pixelsFrame.width<1||pixelsFrame.height<1||pixelsFrame.width*pixelsFrame.height>1048576||!Array.isArray(pixelsFrame.colors)||pixelsFrame.colors.length!==pixelsFrame.width*pixelsFrame.height)return;
 canvas.width=pixelsFrame.width;canvas.height=pixelsFrame.height;
 const ctx=canvas.getContext('2d'),pixels=ctx.createImageData(pixelsFrame.width,pixelsFrame.height);
 pixelsFrame.colors.forEach((c,i)=>{pixels.data[i*4]=(c>>>16)&255;pixels.data[i*4+1]=(c>>>8)&255;pixels.data[i*4+2]=c&255;pixels.data[i*4+3]=255;});ctx.putImageData(pixels,0,0);
}
function paintPreview() {paintPixels($('#stage'),frame);}
function paintLibraryPreviews() {
 document.querySelectorAll('[data-library-preview]').forEach(canvas=>paintPixels(canvas,state?.library.find(item=>String(item.id)===canvas.dataset.libraryPreview)?.preview));
}
function changeView(next, source) {
 if(!Object.hasOwn(pages,next)||!outputPageAvailable(next)||next===view||closing||pendingView)return;
 if(pageHasEdits()){showError('Примените или отмените изменения перед переходом в другой раздел.');return;}
 const previous=view;
 const opener = previous==='settings' && settingsPages.includes(next) && source?.closest('.settings-links')
  ? '#main .settings-links '+focusKey(source)
  : null;
 view=next;render();focusHeading();
 if(!['library','editor'].includes(next)){
  pendingView={previous,next,opener};
  if(!send({op:'page',page:next,device:source?.dataset.connectionDevice})){view=previous;pendingView=null;render();focusHeading();}
 } else settingsOpener=null;
 if(next==='editor')requestPreview();
}
function setDraftOrigin(origin) {
 draftOrigin=origin?{id:origin.id,name:origin.name,scene:clone(origin.scene)}:null;
 selectedId=origin?.id??null;
 draftName=origin?origin.name??'Текущая сцена':'Восстановленный черновик';
}
function followActiveScene() {
 if(!closing&&draftOrigin?.name===null&&!unsavedDraft()&&!same(draft,state.scene)){
  draft=clone(state.scene);
  setDraftOrigin({id:null,name:null,scene:state.scene});
  requestPreview();
 }
}
function checkDraftOrigin() {
 if(!selectedId)return;
 const entry=state.library.find(item=>item.id===selectedId);
 if(entry?.name===draftOrigin.name&&same(entry.scene,draft))setDraftOrigin(entry);
 else if(entry?.name!==draftOrigin.name||!same(entry.scene,draftOrigin.scene))setDraftOrigin({...draftOrigin,id:null});
}
function loadEntry(entry) {
 draft=clone(entry.scene);
 setDraftOrigin({id:entry.builtIn?null:entry.id,name:entry.name,scene:entry.scene});
 if(view==='editor')render();else changeView('editor');
 requestPreview();
 flushPreview();
}
function openEntry(entry) {
 if(entry.id===selectedId){changeView('editor');return;}
 if(!unsavedDraft()){loadEntry(entry);return;}
 draftSwitchId=entry.id;
 $('#draft-switch-description').textContent='Сохранить изменения в сцене «'+draftName+'» перед открытием «'+entry.name+'»?';
 $('#draft-switch').showModal();
 $('#draft-stay').focus();
}
function closeDraftSwitch() {
 const id=draftSwitchId;
 draftSwitchId=null;
 $('#draft-switch').close();
 return id;
}
function stayInDraft() {
 closeDraftSwitch();
 changeView('editor');
}
function saveDraft(name,id=null,nextId=null) {
 const scene=clone(draft);
 if(!send({op:'save_scene',id,name,scene},true))return false;
 savedSceneRequest={id,name,scene,nextId,ids:state.library.map(item=>item.id)};
 return true;
}
function nameDialog(mode) {
 if(!draft||busy())return;
 dialogMode=mode;dialogReturn=focusKey();$('#dialog-title').textContent={new:'Новая сцена',clone:'Создать копию',rename:'Переименовать сцену',save:'Сохранить сцену','save-switch':'Сохранить сцену'}[mode];
 $('#scene-name').value=mode==='new'?'':mode==='clone'?(draftName+' · копия').slice(0,48):draftName;$('#scene-name').setCustomValidity('');$('#name-dialog').showModal();$('#scene-name').focus();$('#scene-name').select();
}
window.prismestraReceive = message => {
 try {
  if(typeof message==='string')message=JSON.parse(message);
  if(message.kind==='output_preview'){window.outputMap?.receive(message);return;}
  if(message.kind==='output_retry'){receiveOutputRetry(message);return;}
  if(message.kind==='flush'){
   if(!Number.isSafeInteger(message.id)||message.id<0)throw Error('Некорректный запрос закрытия');
   if(window.sceneFile?.blockClose()){
    window.ipc.postMessage(JSON.stringify({op:'close_ready',id:message.id,scene:null,form_dirty:true}));return;
   }
   stageText(document.activeElement);closing=true;clearTimeout(previewTimer);previewTimer=null;previewQueued=false;syncControls();
   window.ipc.postMessage(JSON.stringify({op:'close_ready',id:message.id,scene:draft,origin:draftOrigin,form_dirty:stagedDirty()}));return;
  }
  if(message.kind==='close_cancelled'){closing=false;syncControls();return;}
  if(message.kind==='draft_error'){showError(message.message,true);syncControls();return;}
  if(message.kind==='error'){awaitingCommandAck=false;closing=false;submittedStages=[];savedSceneRequest=null;commandPending=false;nativeAwaitingAck=false;if(pendingView){view=pendingView.previous;pendingView=null;render();focusHeading();}showError(message.message,true);syncControls();return;}
  if(['scene_file','scene_file_frame','scene_file_lcd_frame'].includes(message.kind)){window.sceneFile?.receive(message);return;}
  if(message.kind==='snapshot') {
   const acknowledged=awaitingCommandAck;awaitingCommandAck=false;completeStageAck();
   if(retryScope()!==message.state.deviceActivity?.retryScope)outputRetries.clear();
   state=message.state;
   if(Array.isArray(state.effects)){
    effects=Object.create(null);
    audioEffects=new Set(state.effects.filter(effect=>effect.audio).map(effect=>effect.id));
    contextEffects=new Set(state.effects.filter(effect=>effect.context).map(effect=>effect.id));
    reactiveEffects=new Map(state.effects.filter(effect=>effect.reactive).map(effect=>[effect.id,effect.reactive]));
    directEffects=new Set(state.effects.filter(effect=>effect.directColor).map(effect=>effect.id));
    for(const {id,name} of state.effects)effects[id]=name;
   }
   reconcileRetryActivity(message.state.deviceActivity);
   window.outputMap?.update(state);
   if(acknowledged&&errorIsCommand&&!state.error){error='';errorIsCommand=false;$('#native-error').hidden=true;$('#native-error').innerHTML='';}commandPending=false;nativeAwaitingAck=false;
   if(pendingView){
    if(pendingView.next==='settings')settingsFocusReturn=settingsOpener;
    settingsOpener=pendingView.opener??null;
    pendingView=null;
   }
   if(!draft){
    draft=clone(state.draftScene??state.scene);
    setDraftOrigin(state.draftScene?state.draftOrigin??null:{id:null,name:null,scene:state.scene});
   }
   if(savedSceneRequest){
    const {id,name,scene,ids,nextId}=savedSceneRequest;
    const saved=state.library.find(item=>(id?item.id===id:!ids.includes(item.id))&&item.name===name&&same(item.scene,scene));
    if(saved){
     savedSceneRequest=null;
     setDraftOrigin({id:saved.id,name:saved.name,scene:saved.scene});
     requestPreview();
     const next=state.library.find(item=>item.id===nextId);
     if(next){loadEntry(next);return;}
    }
   }
   checkDraftOrigin();
   followActiveScene();
   render();if(!busy())flushPreview();
  } else if(message.kind==='status'&&state) {
   const foundChanged=view==='devices'&&Object.hasOwn(message.state,'deviceCandidates')&&!same(state.deviceCandidates,message.state.deviceCandidates);
   const nativeFormChanged=(Object.hasOwn(message.state,'nativeForm')&&!same(state.nativeForm,message.state.nativeForm))||(Object.hasOwn(message.state,'nativeAux')&&!same(state.nativeAux,message.state.nativeAux));
   if(message.state.deviceActivity&&retryScope()!==message.state.deviceActivity.retryScope)outputRetries.clear();
   Object.assign(state,message.state);
   reconcileRetryActivity(message.state.deviceActivity);
   if(state.pending||Object.hasOwn(message.state,'mapConnection'))window.outputMap?.update(state);
   followActiveScene();
   if(!state.pending&&!nativeAwaitingAck&&!savedSceneRequest)commandPending=false;
   if(message.state.preview)frame=message.state.preview;
   if(nativeFormChanged||foundChanged||(!busy()&&!outputPageAvailable(view)))render();else {syncControls();syncDeviceAnnouncement();paintPreview();}if(!busy())flushPreview();
  }
 }catch(e){awaitingCommandAck=false;closing=false;commandPending=false;showError('Ошибка ответа приложения: '+e.message);syncControls();}
};
document.addEventListener('click',event=>{
 const b=event.target.closest('button');if(!b||b.disabled||(busy()&&b.hasAttribute('data-mutation')&&!recoveryAction(b.dataset.action)))return;
 if(b.id==='library-more'){
  const first=$('.library-grid').children.length;
  libraryLimit+=libraryPageSize;
  render();
  $('.library-grid').children[first]?.querySelector('.scene-preview')?.focus();
  return;
 }
 if(b.dataset.outputRetry!==undefined){
  const id=b.dataset.outputRetry, ticket=outputRetryTicket(id);
  if(view!=='devices'||busy()||pendingView||!ticket||outputRetries.get(id)?.status==='pending')return;
  const request=++retryRequest;
  outputRetries.set(id,{request,ticket,status:'pending'});
  if(!send({op:'retry_output',id,request,ticket}))outputRetries.delete(id);
  syncOutputRetries();
  return;
 }
 if(b.dataset.nativeAction!==undefined)return void send({op:'native_action',id:b.dataset.nativeAction},true);
 if(b.dataset.view)return changeView(b.dataset.view,b);
 if(b.dataset.edit){const entry=state.library.find(e=>e.id===b.dataset.edit);if(entry)openEntry(entry);return;}
 if(b.dataset.apply){const entry=state.library.find(e=>e.id===b.dataset.apply);if(entry)send({op:'apply_scene',scene:clone(entry.scene)},true);return;}
 if(b.dataset.remove){const entry=state.library.find(e=>e.id===b.dataset.remove);if(entry&&!entry.builtIn){dialogReturn=focusKey();$('#remove-description').textContent=`Удалить сцену «${entry.name}» из профиля? Запущенный эффект продолжит работать.`;$('#remove-dialog').dataset.id=entry.id;$('#remove-dialog').showModal();$('#cancel-remove').focus();}return;}
 if(b.dataset.removeColor!==undefined){
  if(draft.colors.length>1){
   const index=+b.dataset.removeColor;
   if(index<paletteIndex())palettePosition=paletteIndex()-1;
   draft.colors.splice(index,1);
   palettePosition=paletteIndex();
  }
  render();requestPreview();return;
 }
 if(b.dataset.asset)return void send({op:'asset',key:b.dataset.asset},true);
 if(b.dataset.deviceId!==undefined){
  if(!pendingView && !renderQueued && view==='devices' && state?.editor?.page==='devices'){
   const id=b.dataset.deviceId;
   if(selectTarget({op:'select_device',id})){
    deviceInspection={id,status:null};
    syncDeviceAnnouncement();
   }
  }
  return;
 }
 switch(b.dataset.action){
 case 'focus-device-setup':$(`[data-native-field="${devicePresetField}"]`)?.focus();break;
 case 'new':case 'clone':case 'rename':return nameDialog(b.dataset.action);
 case 'save-scene':return nameDialog('save');
 case 'apply-scene':send({op:'apply_scene',scene:clone(draft)},true);break;
 case 'insert-palette-color':case 'move-palette-earlier':case 'move-palette-later':return reorderPalette(b.dataset.action);
 case 'add-color':if(draft.colors.length<8)draft.colors.push(draft.colors.at(-1));render();requestPreview();break;
 case 'apply-document':send({op:'apply_fields'},true);break;
 case 'revert-document':formStages.clear();submittedStages=[];if(state.editor?.dirty||state.pending||commandPending)send({op:'revert_document'},true);else render();break;
 case 'discard-page-fields':discardPageFields();break;
 case 'discard-fields':discardPageFields();showError('Незавершённый ввод отменён.');break;
 case 'retry-document':send({op:'retry_document'},true);break;
 case 'classic':send({op:'classic',page:view},true);break;
 case 'dismiss-error':error='';errorIsCommand=false;$('#native-error').hidden=true;if(state)state.error=null;break;
 case 'scene-file':return window.sceneFile?.open(draft,send,paintPixels);
 case 'open':case 'save':case 'export':send({op:b.dataset.action},true);break;
 case 'export-cs2':case 'export-dota2':case 'export-event-source':
  if(state?.native)send({op:'export_event_source',format:({'export-cs2':'cs2','export-dota2':'dota2','export-event-source':'connection'})[b.dataset.action]},true);
  break;
 }
});
document.addEventListener('input',event=>{
 const input=event.target;if(stageText(input))return;if(busy()||!draft)return;
 if(input.id==='search'){search=input.value;libraryLimit=libraryPageSize;render();return;}
 if(input.dataset.param){const n=Number(input.value);if(!Number.isFinite(n))return;const key=input.dataset.param;if(key.startsWith('noise.'))draft.noise[key.slice(6)]=n;else draft[key]=n;document.getElementById('value-'+key).textContent=n+input.dataset.suffix;editDraft();}
 if(input.dataset.color!==undefined){
  const index=+input.dataset.color;
  draft.colors[index]=parseInt(input.value.slice(1),16);
  const option=$('#palette-position')?.options?.[index];
  if(option)option.textContent=palettePositionLabel(draft.colors[index],index);
  editDraft();
 }
 if(input.id==='master-brightness'){draft.brightness=+input.value;const slider=$('[data-param="brightness"]');if(slider){slider.value=draft.brightness;document.getElementById('value-brightness').textContent=draft.brightness+'%';}editDraft();}
});
document.addEventListener('change',event=>{
 const input=event.target;
 if(input.id==='library-effect'){libraryEffect=input.value;libraryLimit=libraryPageSize;render();return;}
 if(input.dataset.nativeField!==undefined){if(['text','secret'].includes(input.dataset.nativeKind)){stageText(input);return;}if(nativeControlDisabled(input))return;send({op:'native_field',id:input.dataset.nativeField,value:input.dataset.nativeKind==='checkbox'?String(input.checked):input.value},true);return;}
 if(busy())return;
 if(input.id==='scene-effect'&&view==='editor'){
  if(!Object.hasOwn(effects,input.value)||draft.effect===input.value)return;
  draft.effect=input.value;
  draft.speed=Math.min(draft.effect==='breathing'?200:100,Math.max(draft.effect==='breathing'?10:1,draft.speed));
  const advanced=$('#advanced-scene');
  if(advanced)advancedOpen=advanced.open;
  $('#scene-parameters').innerHTML=sceneParameters();
  editDraft();
  return;
 }
 if(input.dataset.group){send({op:'groups',...state.groups,[input.dataset.group]:input.checked},true);return;}
 if(input.hasAttribute('data-target')){
  if(input.value!=='' && selectTarget({op:'select_item',index:+input.value},input)){
   deviceInspection=null;
   syncDeviceAnnouncement();
  }
  return;
 }
 if(input.dataset.field){if(input.tagName==='INPUT'){stageText(input);return;}return void send({op:'set_field',key:input.dataset.field,value:input.value},true);}
 if(input.id==='palette-position'){
  const index=Number(input.value);
  if(Number.isInteger(index)&&index>=0&&index<draft.colors.length){palettePosition=index;syncControls();}
  return;
 }
 if(input.dataset.sceneSelect){draft[input.dataset.sceneSelect]=input.value;if(input.dataset.sceneSelect==='noise_palette')render();editDraft();}
});
$('#play').onclick=()=>{if(!state)return;const on=!(state.groups.pc||state.groups.door);send({op:'groups',pc:on,door:on},true);};
$('#footer-apply').onclick=()=>{if(draft)send({op:'apply_scene',scene:clone(draft)},true);};
$('#open-active').onclick=()=>changeView('editor');
$('.brand').onclick=event=>{event.preventDefault();changeView('library');};
$('#sun-icon').innerHTML=icon('sun');
$('#draft-stay').onclick=stayInDraft;
$('#draft-switch').addEventListener('cancel',event=>{event.preventDefault();stayInDraft();});
$('#draft-discard').onclick=()=>{
 if(busy())return;
 const id=closeDraftSwitch(),entry=state.library.find(item=>item.id===id);
 if(entry)loadEntry(entry);
};
$('#draft-save').onclick=()=>{
 if(busy())return;
 if(selectedId){saveDraft(draftName,selectedId,closeDraftSwitch());return;}
 $('#draft-switch').close();
 nameDialog('save-switch');
};
$('#name-form').onsubmit=event=>{
 event.preventDefault();if(busy())return;
 const name=$('#scene-name').value.trim();if(!name||name.length>48||/[\u0000-\u001f\u007f]/.test(name)){$('#scene-name').setCustomValidity('Введите название до 48 символов без управляющих знаков');$('#scene-name').reportValidity();return;}
 const id=['save','rename'].includes(dialogMode)?selectedId:null;
 if(saveDraft(name,id,dialogMode==='save-switch'?draftSwitchId:null)){draftSwitchId=null;$('#name-dialog').close();restoreFocus(dialogReturn);}
};
$('#scene-name').oninput=()=>$('#scene-name').setCustomValidity('');
$('#cancel-name').onclick=()=>{$('#name-dialog').close();draftSwitchId=null;restoreFocus(dialogReturn);};
$('#name-dialog').addEventListener('cancel',()=>{draftSwitchId=null;restoreFocus(dialogReturn);});
const removal=document.createElement('dialog');removal.id='remove-dialog';removal.setAttribute('aria-labelledby','remove-title');removal.innerHTML='<form id="remove-form"><h2 id="remove-title">Удалить сцену</h2><p id="remove-description"></p><div class="dialog-actions"><button type="button" class="button" id="cancel-remove">Отмена</button><button type="submit" class="button primary" data-mutation>Удалить</button></div></form>';document.body.append(removal);
$('#cancel-remove').onclick=()=>{removal.close();restoreFocus(dialogReturn);};removal.addEventListener('cancel',()=>restoreFocus(dialogReturn));$('#remove-form').onsubmit=event=>{event.preventDefault();if(send({op:'delete_scene',id:removal.dataset.id},true)){removal.close();restoreFocus(dialogReturn);}};
const projectActions=document.createElement('div');
projectActions.className='native-project-actions';
const exportDescription='Профиль и используемые медиа. «Мои сцены» хранятся отдельно.';
projectActions.innerHTML=action('Открыть','open')+action('Сохранить','save')+
 action('Экспорт профиля','export',false,`title="${esc(exportDescription)}" aria-describedby="profile-export-scope"`)+
 `<span id="profile-export-scope" class="sr-only">${esc(exportDescription)}</span>`;
$('#save-status').before(projectActions);
document.addEventListener('pointerdown',()=>{pointerGesture=true;},true);
document.addEventListener('pointerup',()=>{pointerGesture=false;releaseGesture();},true);
document.addEventListener('pointercancel',()=>{pointerGesture=false;releaseGesture();},true);
document.addEventListener('focusin',()=>{syncDeviceAnnouncement();syncOutputRetries();});
document.addEventListener('keydown',event=>{
 if((event.key===' '||event.key==='Spacebar')&&event.target.closest('button'))keyGesture=true;
 if(event.key!=='Escape'||event.defaultPrevented||event.isComposing||!deviceInspection||
  view!=='devices'||!event.target.closest('#device-properties'))return;
 event.preventDefault();
 restoreFocus(deviceFocusKey(deviceInspection.id));
},true);
document.addEventListener('keyup',()=>{keyGesture=false;releaseGesture();},true);
document.addEventListener('compositionstart',()=>{composing=true;},true);
document.addEventListener('compositionend',()=>{composing=false;releaseGesture();},true);
window.addEventListener('blur',()=>{pointerGesture=false;keyGesture=false;composing=false;releaseGesture();});
document.addEventListener('toggle',event=>{if(event.target.id==='advanced-scene')advancedOpen=event.target.open;},true);
window.addEventListener('pagehide',()=>flushPreview(true));
window.addEventListener('beforeunload',()=>flushPreview(true));
render();send({op:'snapshot'});
