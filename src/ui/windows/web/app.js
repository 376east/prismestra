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
const titleFromId = id => id.replace(/[_-]/g,' ').replace(/^./,letter=>letter.toUpperCase())
 .replace(/\b(cpu|lcd|rgb|led|ansi|cs2|wled)\b/gi,word=>word.toUpperCase());
const effectLabel = id => effects[id] ?? id;
const sceneLabel = entry => english && entry.builtIn && entry.id.startsWith('builtin:')
 ? titleFromId(entry.id.slice('builtin:'.length)) : entry.name;
const effectOption = (id, selected) => `<option value="${esc(id)}" ${id===selected?'selected':''}>${esc(effectLabel(id))}</option>`;
const fixtureLanguage=window.location?.protocol==='file:'?new URLSearchParams(window.location.search).get('lang'):null;
const uiLanguage=(window.prismestraLanguage??fixtureLanguage)==='en'?'en':'ru';
const english=uiLanguage==='en';
const ui=(ru,en)=>english?en:ru;
const pages = {library:ui('Сцены','Scenes'),editor:ui('Редактор','Editor'),devices:ui('Устройства','Devices'),
 lcd:'LCD',ambient:ui('Экран','Screen lighting'),settings:ui('Настройки','Settings'),
 temperatures:ui('Температуры','Temperatures'),connections:ui('Подключения','Connections'),
 sources:ui('Источники','Sources'),schedule:ui('Расписание','Schedule')};
const englishStartupStatus = new Map([
 ['restoring','Restoring the saved scene and connections…'],
 ['restored','Saved connections restored. Check output status in Devices.'],
 ['searching','Looking for supported devices. Any devices found will be set up automatically.'],
 ['searchingMore','Looking for more supported devices…'],
 ['empty','No supported devices found yet.'],
 ['unconfiguredArgb','Found ARGB controllers with no LEDs configured. Open ARGB channels on the Devices page.'],
 ['noNewDevices','No new devices found. Check Connections for details.'],
 ['applying','Devices found. Adding them to the shared scene…'],
 ['applied','Devices added to the profile. Check their output status in Devices.'],
 ['complete','Automatic discovery finished.'],
 ['cancelled','Automatic setup stopped. Your edits are still in the editor.'],
 ['failed','Automatic setup could not finish. Check Connections or try searching again when available.'],
]);
const englishRuntimeStatus=new Map([
 ['Готово','Ready'],['Профиль сохранён.','Profile saved.'],
 ['Сцена сохранена','Scene saved'],['Сцена сохранена.','Scene saved.'],
 ['Параметр изменён. Нажмите «Применить».','A setting changed. Select Apply.'],
 ['Проверка и применение параметров…','Checking and applying settings…'],
 ['Изменения параметров отменены.','Changes discarded.'],
 ['Изменения расписания отменены.','Schedule changes discarded.'],
 ['Повторная проверка подключений…','Checking connections again…'],
 ['Открытие профиля: ожидаю завершения текущих операций…','Waiting for active operations before opening the profile…'],
 ['Файл добавлен в профиль. Нажмите «Применить».','File added to the profile. Select Apply.'],
]);
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
 trash:'<path d="M4 7h16M9 7V4h6v3M6 7l1 14h10l1-14M10 11v6m4-6v6"/>',
 plus:'<path d="M12 5v14M5 12h14"/>',
 sun:'<circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2"/>',
};
glyphs.settings='<path d="M4 6h16M4 12h16M4 18h16"/><circle cx="8" cy="6" r="2" fill="var(--ui-sidebar)"/><circle cx="15" cy="12" r="2" fill="var(--ui-sidebar)"/><circle cx="10" cy="18" r="2" fill="var(--ui-sidebar)"/>';
const icon = name => `<svg viewBox="0 0 24 24" aria-hidden="true">${glyphs[name] ?? ''}</svg>`;
let state = null, draft = null, draftOrigin = null, draftName = ui('Текущая сцена','Current scene'), selectedId = null, draftSwitchId = null;
let view = 'library', search = '', previewTimer = null, frame = null, commandPending = false, error = '', dialogMode = 'new', dialogReturn = null, savedSceneRequest = null, previewQueued = false, closing = false, nativeAwaitingAck = false, pendingView = null, advancedOpen = false, submittedStages = [], pointerGesture = false, keyGesture = false, composing = false, renderQueued = false, awaitingCommandAck = false, errorIsCommand = false;
const formStages = new Map();
const libraryPageSize = 24;
let libraryLimit = libraryPageSize, libraryEffect = '';
let sceneActivation = null, sceneActivationRequest = 0, editorSceneResult = null;
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
 const retry=outputRetries.get(id);
 return retry&&ticket===retry.ticket&&!retry.republished?null:ticket;
}
function reconcileRetryActivity(activity) {
 if(!activity)return;
 for(const [id,retry] of outputRetries) {
  if(retry.status==='failed'){
   const tickets=activity.retryTickets;
   retry.republished=!!tickets&&Object.hasOwn(tickets,id)&&tickets[id]===retry.ticket;
  }
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
   ? ui('Подключение было занято другим выходом Prismestra. Дождитесь его отключения. Когда кнопка доступна, нажмите «Повторить подключение».','Another Prismestra output was using this connection. Wait for it to disconnect, then select Retry connection when available.')
   : '';
  if(help.textContent!==instruction)help.textContent=instruction;
  help.hidden=!instruction;
  const preserveFocus=button===document.activeElement||id===keepFocus;
  const message=retry?.status==='pending'?ui('Подключаемся…','Connecting…'):retry?.status==='submitted'&&!preserveFocus?'':retry?.message||'';
  if(status.textContent!==message)status.textContent=message;
  button.setAttribute('aria-disabled',String(busy()||!!pendingView||!ticket||retry?.status==='pending'));
  button.hidden=!selected||(!ticket&&retry?.status!=='pending'&&!preserveFocus);
  button.parentElement.hidden=button.hidden&&!message&&!instruction;
 });
}
function receiveOutputRetry({id,request,scope,status,message}) {
 const retry=outputRetries.get(id);
 if(!retry||scope!==retryScope()||retry.request!==request||retry.status!=='pending')return;
 if(status==='failed')retry.message=ui(message||'Подключение не удалось','Connection failed');
 else if(status==='submitted')retry.message=ui('Кадры отправлены','Frames sent');
 else if(status==='cancelled')retry.message=ui('Подключение отменено','Connection cancelled');
 else return;
 retry.status=status;
 if(status==='submitted' && state.deviceActivity?.labels?.[id]==='Подключается'){
  retry.awaitingActivity=true;
  syncOutputStatuses();
 }
 syncOutputRetries();
}
let settingsOpener = null, settingsFocusReturn = null;
const busy = () => closing || !state || !!state.pending || commandPending || sceneActivation?.pending || savedSceneRequest !== null;
const disabled = () => busy() ? 'disabled' : '';
const changed = () => !!draft && !!state && !same(draft,state.scene);
const sourceNeedsApply = () => !!state?.native && !!draft
 && ['camera','keys'].includes(reactiveEffects.get(draft.effect))
 && (draft.effect!==state.scene.effect || !state.captureAuthorized);
const savedDraft = () => selectedId && state?.library.find(item=>item.id===selectedId && same(item.scene,draft));
const applyNeeded = () => changed() || sourceNeedsApply() || Boolean(state?.native && savedDraft()?.aliceLamp);
let placementOpen=false, placementDrag=null, placementSelection=null;
let placementLimited=false;
const placementHelp=()=>placementLimited
 ? ui('Край холста. Все LED устройства должны оставаться внутри.','Canvas edge. Keep every LED on the canvas.')
 : ui('Перетащите устройство или выберите его клавишей Tab и сдвиньте стрелками.','Drag the device, or select it with Tab and move it with the arrow keys.');
const placementDirty=()=>view==='editor' && placementOpen && state?.editor?.page==='devices' && state.editor.dirty;
const unsavedDraft = () => !!draft && (!draftOrigin || !same(draft,draftOrigin.scene));
const deviceFocusKey = id => `[data-device-id="${CSS.escape(id)}"]`;
function focusKey(node=document.activeElement) {
 if(!node) return null;
 if(node.id) return '#'+CSS.escape(node.id);
 if(node.dataset?.deviceId!==undefined)return deviceFocusKey(node.dataset.deviceId);
 for(const key of ['view','action','preview','edit','apply','remove','field','param','color','target','group','asset','remove-color','native-field','native-action']) if(node.dataset?.[key] !== undefined) return `[data-${key}="${CSS.escape(node.dataset[key])}"]`;
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
 if(code==='apply-scene'&&placementDirty())return true;
 if(code==='retry-startup-discovery')return !state?.startupDiscoveryRetry||busy()||stagedDirty();
 if(code==='revert-document')return !state || !(busy()||state.editor?.dirty||stagedDirty());
 if(code==='retry-document')return !state || !(busy()||error||state.error);
 if(['insert-palette-color','move-palette-earlier','move-palette-later'].includes(code)){
  if(busy()||!paletteHasOrder(draft))return true;
  if(code==='insert-palette-color')return draft.colors.length>=8;
  return code==='move-palette-earlier'?paletteIndex()===0:paletteIndex()===draft.colors.length-1;
 }
 return busy() || (node.dataset.needsChange==='scene'&&!applyNeeded()) || (node.dataset.needsChange==='document'&&!(state?.editor?.dirty||currentGenericStages().length)) || (code==='add-color'&&draft?.colors.length>=8) || (node.dataset.removeColor!==undefined&&draft?.colors.length<=1);
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
function selectTarget(command,input=null) {if(stagedDirty()){if(input)input.value=state.editor.index<state.editor.targets.length?String(state.editor.index):'';showError(ui('Примените или отмените незавершённый ввод перед выбором другого объекта.','Apply or discard unfinished input before selecting another item.'));return false;}return send(command,true);}
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
function showError(message,fromCommand=false,{detail='',retrySearch=false,allowRetry=true}={}) {
 errorIsCommand=fromCommand;error=String(message);const box=$('#native-error');box.hidden=false;
 const recovery=retrySearch
  ?action(ui('Повторить поиск','Retry search'),'retry-startup-discovery',false,'id="startup-discovery-retry-error"')
  :allowRetry?'<button class="button" data-action="retry-document" data-mutation>'+ui('Повторить','Retry')+'</button>':'';
 const details=detail?`<details><summary>${ui('Технические подробности','Technical details')}</summary><p>${esc(detail)}</p></details>`:'';
 box.innerHTML=`<button class="icon-button" data-action="dismiss-error" aria-label="${ui('Закрыть сообщение','Dismiss message')}">×</button><p>${esc(error)}</p>${stagedDirty()?`<button class="button" data-action="discard-fields">${ui('Отменить незавершённый ввод','Discard unfinished input')}</button>`:''}${recovery}${details}`;
}
function showRuntimeError(message,fromCommand=false) {
 if(english)showError('Prismestra could not complete the operation.',fromCommand,{detail:message});
 else showError(message,fromCommand);
}
function send(command, mutation=false) {
 if(closing)return false;
 if(outputPage(view)&&['apply_fields','set_field','asset'].includes(command.op)&&
  (detachedOutputEdits() || fieldTarget(renderedOutputEditor)!==fieldTarget(state?.editor)))return false;
 const nativeCommand=['native_field','native_action'].includes(command.op);
 if(nativeCommand){const node={dataset:{[command.op==='native_field'?'nativeField':'nativeAction']:String(command.id)}};if(nativeControlDisabled(node))return false;const items=command.op==='native_field'?nativeFormForView().fields:nativeFormForView().actions;command.id=items.find(item=>String(item.id)===String(command.id)).id;}
 else if(mutation && busy() && !recoveryOperation(command.op)) return false;
 if(['open','save','export'].includes(command.op)&&(stagedDirty()||placementDirty())){showError(ui('Примените или отмените незавершённый ввод перед сохранением профиля.','Apply or discard unfinished input before saving the profile.'));return false;}
 const stagedCommand=['apply_fields','set_field','asset','native_field','native_action'].includes(command.op);
 const discardNative=nativeCommand&&['408','1812','1819','1107','1141'].includes(String(command.id));
 if(stagedCommand&&!discardNative){if(nativeCommand){command.fields=currentNativeStages();command.editor_fields=state.pending?[]:currentGenericStages().filter(item=>state.editor.fields.find(field=>field.key===item.key)?.enabled!==false);}else command.fields=currentGenericStages();}
 if(['apply_scene','open','save','export'].includes(command.op))flushPreview(true);
 if(!window.ipc || typeof window.ipc.postMessage !== 'function') {showError(ui('Нет подключения к приложению Prismestra. Откройте интерфейс из нативного приложения.','Prismestra is disconnected. Open the editor from the application.'));return false;}
 try {window.ipc.postMessage(JSON.stringify(command));if(mutation||command.op==='page')awaitingCommandAck=true;if(stagedCommand){const keys=discardNative?[...formStages].filter(([,record])=>record.page===view).map(([key])=>key):nativeCommand?[...(command.fields||[]).map(item=>stageKey('native',item.id)),...(command.editor_fields||[]).map(item=>stageKey('generic',item.key))]:(command.fields||[]).map(item=>stageKey('generic',item.key));submittedStages=keys.filter(key=>formStages.has(key)).map(key=>[key,formStages.get(key).value]);if(nativeCommand&&!discardNative){for(const [key] of submittedStages)if(formStages.get(key)?.secret)formStages.delete(key);document.querySelectorAll('[data-native-kind="secret"]').forEach(input=>{if(!formStages.has(stageKey('native',input.dataset.nativeField)))input.value='';});}}if(mutation){commandPending=true;nativeAwaitingAck=nativeCommand||stagedCommand;syncControls();}return true;}catch(e){awaitingCommandAck=false;commandPending=false;showError(e.message);syncControls();return false;}
}
function syncControls(keepRetryFocus) {
 document.querySelectorAll('[data-mutation]').forEach(node=>{
  const disabled=controlDisabled(node);
  // Keep the row focusable across the command acknowledgement and list updates.
  if(node.dataset.deviceId!==undefined||node.dataset.apply!==undefined||node.id==='ui-language')node.setAttribute('aria-disabled',String(disabled));
  else node.disabled=disabled;
 });
 document.querySelectorAll('[data-native-control]').forEach(node=>node.disabled=nativeControlDisabled(node));
 const copyOutput=$('[data-copy-output]');if(copyOutput)copyOutput.disabled=closing||!!pendingView||commandPending;
 document.querySelectorAll('[data-local-discard]').forEach(node=>{node.hidden=!currentPageStaged();node.disabled=closing||commandPending;});
 $('#master-brightness').disabled=busy()||!draft;
 $('#footer-apply').disabled=busy()||placementDirty()||!applyNeeded();
 $('#play').disabled=busy();
 const light=!!(state?.groups?.pc||state?.groups?.door);$('#play').textContent=light?ui('Выключить свет','Turn lights off'):ui('Включить свет','Turn lights on');
 $('#player-name').textContent=state?effectLabel(state.scene.effect):'Prismestra';
 const startupStatus=english?englishStartupStatus.get(state?.startupStatus):null;
 const playerStatus=$('#player-status');
 const runtimeStatus=state?.status==='Сцена изменена и ещё не сохранена.'?'':state?.status;
 playerStatus.textContent=startupStatus||(runtimeStatus&&ui(runtimeStatus,
  englishRuntimeStatus.get(runtimeStatus)??(state.pending?'Working…':'See profile status in Devices')))
  ||(state?'':ui('Подключение к приложению…','Connecting to the app…'));
 playerStatus.title=english||startupStatus?runtimeStatus||'':'';
 const runtimeDetail=$('#native-runtime-detail');if(runtimeDetail)runtimeDetail.textContent=state?.detail||'';
 const diagnostics=$('#native-runtime-diagnostics');if(diagnostics)diagnostics.hidden=!state?.detail;
 if(sceneActivation?.id)syncSceneActivation(sceneActivation.id,sceneActivation);
 const editorLamp=$('#editor-lamp-status');
 if(editorLamp){
  const message=sceneActivation?.editor && sceneActivation.id===selectedId &&
   same(sceneActivation.scene,draft) ? sceneActivation.lampMessage||'' : '';
  if(editorLamp.textContent!==message)editorLamp.textContent=message;
 }
 const audioSource=$('#scene-audio-source');
 if(audioSource)audioSource.textContent=state?.native
  ? state.audioError&&audioEffects.has(state.scene.effect)?ui('Системный звук недоступен. Повторяю подключение.','System audio is unavailable. Reconnecting.')
   : ui('Источник — звук, который воспроизводит Windows. Данные появляются после применения при включённом RGB.','Uses audio playing in Windows after applying the scene with RGB enabled.')
  : ui('Системный звук доступен в профиле с локальным RGB-подключением.','System audio is available with a local RGB connection.');
 const reactiveSource=$('#scene-reactive-source');
 if(reactiveSource&&state&&draft){
  const kind=reactiveEffects.get(draft.effect);
  const hints={
   keys:ui('Источник — нажатия клавиш. Для подсветки используются позиции клавиш полноразмерной ANSI-клавиатуры.','Uses keystrokes to light up positions on a full-size ANSI keyboard.'),
   camera:ui('После применения используется первая камера в списке Windows.','Uses the first camera listed in Windows after applying the scene.'),
   game:ui('После применения ожидаются события CS2, Dota 2 или локального адаптера игры.','Waits for events from CS2, Dota 2, or a local game adapter after applying the scene.'),
   external:draft.effect==='media_ambience'
    ?ui('Источник — текущий проигрыватель Windows с обложкой или данные локального адаптера.','Uses the active Windows media player with cover art, or a local adapter.')
    :ui('После применения ожидаются данные от подключённого локального источника.','Waits for data from the connected local source after applying the scene.')
  };
  reactiveSource.textContent=!state.native?ui('Подключите локальный профиль RGB.','Connect a local RGB profile.'):
   state.reactiveError&&reactiveEffects.get(state.scene.effect)===kind?state.reactiveError:
   hints[kind]??'';
 }
 syncOutputStatuses();
 syncOutputRetries(keepRetryFocus);
 const discoveryHelp=$('#startup-discovery-help');
 if(discoveryHelp)discoveryHelp.hidden=state?.startupDiscoveryUnknownController!==true;
 const emptySearch=$('#startup-empty-search');
 if(emptySearch){
  const result=!state?.devices.length&&!state?.deviceCandidates?.length ? state?.startupEmptySearchResult : null;
  const message=result==='continuing'?ui('Поддерживаемые устройства пока не найдены. Автопоиск продолжится в фоне.','No supported devices found yet. Automatic discovery will continue in the background.')
   :result==='complete'?ui('Автопоиск завершён. Поддерживаемые устройства не найдены.','Automatic discovery finished. No supported devices found.'):'';
  emptySearch.hidden=!message;
  if(emptySearch.textContent!==message)emptySearch.textContent=message;
 }
 $('#save-status').textContent=busy()&&state?ui('Выполняется…','Working…'):state?.dirty?ui('Профиль изменён','Profile modified'):ui('Локальный профиль','Local profile');
 if(draft){$('#master-brightness').value=draft.brightness;$('#master-value').textContent=draft.brightness+'%';}
 if(editorSceneResult&&!same(editorSceneResult.scene,draft))editorSceneResult=null;
 const status=$('.scene-status');
 if(status){
  const message=editorSceneResult?.message??(changed()?ui('Изменения ещё не применены','Changes not yet applied'):sourceNeedsApply()?ui('Для источника нужно применить сцену','Apply the scene to use this source'):'');
  if(status.textContent!==message)status.textContent=message;
 }
 if(state){const art=$('#player-art'),ctx=art.getContext('2d');art.setAttribute('aria-label',ui('Палитра применённой сцены','Applied scene palette'));state.scene.colors.forEach((c,i)=>{ctx.fillStyle=hex(c);ctx.fillRect(i*art.width/state.scene.colors.length,0,Math.ceil(art.width/state.scene.colors.length),art.height);});}
 if(state?.error)showRuntimeError(state.error,true);
}
function palette(scene) {return `<div class="palette-art" aria-label="${ui('Палитра сцены','Scene palette')}">${scene.colors.map(c=>`<span style="background:${hex(c)}"></span>`).join('')}</div>`;}
function heading(title,description,actions='') {
 const sceneFileAction=window.sceneFile&&draft&&['library','editor'].includes(view)
  ?action(ui('Файл сцены','Scene file'),'scene-file',false,'id="scene-file-launch"'):'';
 return `<div class="page-heading"><div><h1>${esc(title)}</h1><p>${esc(description)}</p></div><div class="heading-actions">${actions}${sceneFileAction}</div></div>`;
}
function action(label,code,primary=false,extra='') {return `<button class="button ${primary?'primary':''}" data-action="${code}" data-mutation ${disabled()} ${extra}>${label}</button>`;}
function searchText(text) {
 return text.normalize('NFC').toLowerCase().replaceAll('ё','е');
}
function renderLibrary() {
 const lcdOnly=state.devices.length>0&&state.devices.every(device=>device.role==='lcd');
 const words=searchText(search).match(/[\p{L}\p{N}]+/gu)??[];
 const available=new Set(state.library.map(item=>item.scene.effect));
 const list=state.library.filter(item=>{
  const effect=item.scene.effect;
  if(libraryEffect&&effect!==libraryEffect)return false;
  if(!words.length)return true;
  const searchable=searchText(`${sceneLabel(item)}\n${item.name}\n${effectLabel(effect)}\n${effect}`);
  return words.every(word=>searchable.includes(word));
 });
 const effectIds=[...Object.keys(effects).filter(id=>available.has(id)),...Array.from(available).filter(id=>!Object.hasOwn(effects,id))];
 const options=effectIds.map(id=>effectOption(id,libraryEffect)).join('');
 const cards=list.slice(0,libraryLimit).map(item=>`<article class="scene-card ${same(item.scene,state.scene)?'selected':''}">
   <div class="art-wrap"><button class="scene-preview" data-preview="${esc(item.id)}" aria-label="${ui('Предпросмотр','Preview')}: ${esc(sceneLabel(item))}">
    ${item.preview?`<canvas data-library-preview="${esc(item.id)}" aria-hidden="true"></canvas>`:palette(item.scene)}
    <span class="scene-preview-label">${ui('Предпросмотр','Preview')}</span>
   </button></div>
   <div class="scene-body"><div class="scene-title"><h2>${esc(sceneLabel(item))}</h2><small class="muted">${item.builtIn?ui('Шаблон · ','Preset · '):''}${esc(effectLabel(item.scene.effect))}${item.audioDemo?ui(' · Тестовый сигнал',' · Test signal'):''}</small></div>
    <div class="card-actions"><button class="button primary" data-apply="${esc(item.id)}" aria-label="${ui('Применить сцену','Apply scene')}: ${esc(sceneLabel(item))}" aria-disabled="${!!busy()}" data-mutation>${ui('Применить сцену','Apply scene')}</button>
     <button class="icon-button" data-edit="${esc(item.id)}" aria-label="${ui('Редактировать','Edit')}: ${esc(sceneLabel(item))}" title="${ui('Редактировать','Edit')}" data-mutation ${disabled()}>${icon('edit')}</button>
     ${item.builtIn?'':`<button class="icon-button" data-remove="${esc(item.id)}" aria-label="${ui('Удалить','Delete')}: ${esc(sceneLabel(item))}" data-mutation ${disabled()}>${icon('trash')}</button>`}
    </div>
    ${item.builtIn||(!state.native&&!item.aliceLampStale)?'':`<button class="button" data-alice-scene="${esc(item.id)}" data-mutation ${disabled()}>${item.aliceLampStale?ui('Привязка лампы устарела','Lamp binding needs review'):item.aliceLamp?ui('Лампа · ','Lamp · ')+esc(item.aliceLamp.label):ui('Добавить лампу','Add a lamp')}</button>`}
    <div id="scene-activation-${esc(item.id)}" class="scene-activation-status" role="status" aria-atomic="true"></div>
    <button id="scene-devices-${esc(item.id)}" class="button scene-devices" data-view="devices" hidden>${ui('Проверить устройства','Check devices')}</button>
   </div>
  </article>`).join('');
 const description=lcdOnly
  ? ui('В профиле только LCD. Фон и показания температуры настраиваются отдельно от RGB-сцен.','This profile has only LCD outputs. Set the background and temperatures separately from RGB scenes.')
  : ui('Сцены сохраняют параметры RGB. Устройства и LCD сохраняются в профиле.','Scenes store RGB settings. Devices and LCDs belong to the profile.');
 const outputAction=lcdOnly?pageButton('lcd',ui('Настроить LCD','Set up LCD'),true):startupDiscoveryAction();
 return heading(pages.library,description,outputAction+action(icon('plus')+' '+ui('Новая сцена','New scene'),'new',!lcdOnly&&!state.startupDiscoveryRetry))+startupSearchSummary+`
  <div class="toolbar">
   <select id="library-effect" aria-label="${ui('Тип эффекта','Effect type')}"><option value="">${ui('Все эффекты','All effects')}</option>${options}</select>
   <output class="muted" aria-live="polite">${ui('Показано','Showing')} ${Math.min(libraryLimit,list.length)} ${ui('из','of')} ${list.length}</output>
   <label class="search"><input id="search" placeholder="${ui('Название сцены или эффект','Scene name or effect')}" aria-label="${ui('Поиск по названию сцены или эффекту','Search by scene name or effect')}" value="${esc(search)}"></label>
  </div>
  <div class="library-grid">${cards||`<div class="empty"><h2>${ui('Сцен не найдено','No scenes found')}</h2><p>${ui('Измените запрос или выберите другой тип эффекта.','Change your search or select another effect type.')}</p></div>`}</div>
  ${list.length>libraryLimit?`<button class="button" id="library-more">${ui('Показать ещё','Show more')}</button>`:''}`;
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
 return range('rainbow_width',draft.effect==='rainbow_wave'?ui('Ширина волны','Wave width'):'Масштаб рисунка',1,50);
}
function sceneSourceControls() {
 if(audioEffects.has(draft.effect))return '<p class="muted" id="scene-audio-source"></p>';
 const source=reactiveEffects.get(draft.effect);
 if(source){
  const exportButtons=state?.native?source==='game'
   ?action(ui('Сохранить настройку CS2','Export CS2 setup'),'export-cs2')+action(ui('Сохранить настройку Dota 2','Export Dota 2 setup'),'export-dota2')+action(ui('Сохранить адрес и ключ','Export address and key'),'export-event-source')
   :source==='external'?action(ui('Сохранить адрес и ключ','Export address and key'),'export-event-source'):'':'';
  const cadence=['game_resources','game_round','weather_scene'].includes(draft.effect)?range('speed',ui('Скорость','Speed'),1,100):'';
  return '<p class="muted" id="scene-reactive-source"></p>'+exportButtons+cadence;
 }
 if(draft.effect==='day_night')return `<p class="muted">${ui('Источник — местное время с часовым поясом из расписания.','Uses local time in the schedule time zone.')}</p>`;
 if(draft.effect==='thermal_tint'||draft.effect==='thermal_gauge')return `<p class="muted">${ui('Источник — самая высокая свежая температура из настроенных источников.','Uses the highest recent temperature from the configured sources.')}</p>`;
 const speed=range('speed',ui('Скорость','Speed'),draft.effect==='breathing'?10:1,draft.effect==='breathing'?200:100);
 return draft.effect==='cpu_pulse'?`<p class="muted">${ui('Источник — загрузка CPU в подключённом профиле.','Uses CPU load from the connected profile.')}</p>`+speed:speed;
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
     <label for="palette-position">${ui('Позиция цвета','Color position')}</label>
     <select id="palette-position" data-mutation ${disabled()}>${options}</select>
   </div>
   <div class="native-palette-order-actions" role="group" aria-label="${ui('Порядок цветов','Color order')}">
     ${action(ui('Вставить после','Insert after'),'insert-palette-color',false,`title="${ui('Вставить копию выбранного цвета','Insert a copy of the selected color')}"`)}
     ${action(ui('← Раньше','← Earlier'),'move-palette-earlier')}
     ${action(ui('Позже →','Later →'),'move-palette-later')}
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
 return heading(draftName,ui('Применение обновляет активную сцену. Сохранение записывает версию в библиотеку.','Apply updates the active scene. Save keeps a version in your library.'),action(ui('Сохранить в библиотеку','Save to library'),'save-scene')+action(ui('Создать копию','Make a copy'),'clone')+(selectedId?action(ui('Переименовать','Rename'),'rename'):'')+action(ui('Применить сцену','Apply scene'),'apply-scene',true,'data-needs-change="scene"'))+`<p class="scene-status" role="status" aria-atomic="true"></p><p id="editor-lamp-status" class="scene-activation-status" role="status"></p><div class="editor-layout"><section class="preview-panel" aria-label="${ui('Предпросмотр','Preview')}"><div class="stage"><canvas id="stage" aria-label="${ui('Предпросмотр сцены из движка Prismestra','Prismestra engine scene preview')}"></canvas><div class="stage-bar"><span class="stage-chip">${ui('Предпросмотр','Preview')}</span></div><div class="stage-caption">${ui('Кадр движка · черновик','Engine frame · draft')}</div>${renderPlacementObjects()}</div>${renderPlacementControls()}<p class="preview-description">${ui('Предпросмотр не меняет подсветку. Выходы RGB и LCD включаются отдельно.','Preview leaves lighting unchanged. RGB and LCD outputs are enabled separately.')}</p><div class="native-field"><label for="scene-effect">${ui('Эффект','Effect')}</label><select id="scene-effect" data-mutation ${disabled()}>${effectIds.map(id=>effectOption(id,draft.effect)).join('')}</select></div><div class="parameter-panel" style="margin-top:20px"><h2>${ui('Выходы','Outputs')}</h2><label class="native-group"><input type="checkbox" data-group="pc" ${state.groups.pc?'checked':''} data-mutation ${disabled()}> ${ui('Основной свет · RGB и LCD','Primary lighting · RGB and LCD')}</label><label class="native-group"><input type="checkbox" data-group="door" ${state.groups.door?'checked':''} data-mutation ${disabled()}> ${ui('Дополнительный свет','Secondary lighting')}</label><button class="button" data-view="devices">${ui('Устройства и свойства','Devices and properties')}</button></div></section><aside class="parameter-panel" id="scene-parameters" aria-label="${ui('Параметры сцены','Scene parameters')}">${sceneParameters()}</aside></div>`;
}
function placementLabel(object,objects) {
 return objects.some(other=>other.id!==object.id&&other.label===object.label)
  ? `${object.label} · ${object.id}` : object.label;
}
function renderPlacementObjects() {
 if(!placementOpen||!state.draftMapRevision)return '';
 const {width,height,objects}=state.canvasPlacement;
 return objects.map(object=>{
  const {id,label,bounds:[x,y,right,bottom]}=object;
  const name=esc(placementLabel(object,objects));
  const left=(x+right+1)*50/width, top=(y+bottom+1)*50/height;
  return `<button type="button" class="placement-object" id="placement-${esc(id)}" data-placement-device="${esc(id)}" aria-describedby="placement-help" aria-label="${name}" title="${name}" style="left:${left}%;top:${top}%">${esc(label)}</button>`;
 }).join('');
}
function renderPlacementControls() {
 const objects=state.canvasPlacement?.objects;
 if(!objects?.length)return '';
 if(!placementOpen)return action(ui('Разместить RGB','Position RGB devices'),'open-placement',false,'id="open-placement"');
 const picker=objects.length>1&&state.draftMapRevision ? `<label class="native-field placement-picker" for="placement-picker">${ui('RGB-устройство','RGB device')}
  <select id="placement-picker">${objects.map(object=>`<option value="${esc(object.id)}" ${object.id===placementSelection?'selected':''}>${esc(placementLabel(object,objects))}</option>`).join('')}</select>
 </label><button type="button" id="focus-placement" class="button" data-action="focus-placement">${ui('К метке','Go to marker')}</button>` : '';
 return `<div class="placement-actions" aria-label="${ui('Раскладка RGB','RGB layout')}">
  <p id="placement-help" role="status">${placementHelp()}</p>
  ${picker}
  <span role="status">${placementDirty()?ui('Раскладка не применена','Layout not applied'):''}</span>
  ${action(ui('Применить раскладку','Apply layout'),'apply-placement',true,'data-needs-change="document"')}${action(ui('Отменить изменения','Discard changes'),'revert-document')}
 </div>`;
}
function openPlacement() {
 if(busy()||!state.canvasPlacement?.objects.length)return;
 placementOpen=true;
 if(state.editor?.page==='devices'){render();return;}
 if(send({op:'page',page:'devices'},true))pendingView={previous:view,next:view};
 else placementOpen=false;
}
function movePlacement(device,revision,dx,dy) {
 if(placementOpen&&revision&&(dx||dy))send({op:'move_canvas_device',device,revision,dx,dy},true);
}
function showPlacementLimit(limited) {
 if(placementLimited===limited)return;
 placementLimited=limited;
 const help=$('#placement-help');
 if(help)help.textContent=placementHelp();
}
function placementDelta(drag,clientX,clientY) {
 const rawX=Math.round((clientX-drag.x)/drag.cellX);
 const rawY=Math.round((clientY-drag.y)/drag.cellY);
 const dx=Math.min(drag.maxX,Math.max(drag.minX,rawX));
 const dy=Math.min(drag.maxY,Math.max(drag.minY,rawY));
 return {dx,dy,limited:dx!==rawX||dy!==rawY};
}
function sceneParameters() {
 const noise=draft.effect==='noise_map';
 return `${directEffects.has(draft.effect)?'':`<div class="control-block"><div class="control-label">${ui('Палитра','Palette')} <small>${draft.colors.length} / 8</small></div><div class="native-palette">${draft.colors.map((c,i)=>`<div class="native-palette-item"><label class="color-control"><input type="color" id="palette-color-${i}" data-color="${i}" value="${hex(c)}" aria-label="${ui('Цвет','Color')} ${i+1}" data-mutation ${disabled()}></label><button class="icon-button" data-remove-color="${i}" data-mutation aria-label="${ui('Удалить цвет','Remove color')} ${i+1}" ${draft.colors.length===1?'disabled':disabled()}>×</button></div>`).join('')}<button class="icon-button" data-action="add-color" aria-label="${ui('Добавить цвет','Add color')}" data-mutation ${draft.colors.length>=8?'disabled':disabled()}>${icon('plus')}</button></div>${paletteOrderMarkup()}</div>`}${sceneSourceControls()}${range('brightness',ui('Яркость','Brightness'),0,100,1,'%')}<details id="advanced-scene" ${advancedOpen?'open':''}><summary>${ui('Дополнительно','Advanced')}</summary>${range('fps',ui('Частота кадров','Frame rate'),1,144,1,' fps')}${range('temperature',ui('Температура','Temperature'),-100,100)}${range('tint',ui('Оттенок','Tint'),-100,100)}${patternScale()}${noise?`<div class="native-field control-block"><label for="noise-palette">${ui('Палитра шума','Noise palette')}</label><select id="noise-palette" data-scene-select="noise_palette" data-mutation ${disabled()}>${Object.entries({custom:ui('Своя палитра','Custom palette'),rainbow:ui('Радуга','Rainbow'),inverse_rainbow:ui('Обратная радуга','Reverse rainbow')}).map(([v,t])=>`<option value="${v}" ${draft.noise_palette===v?'selected':''}>${t}</option>`).join('')}</select></div>${range('noise.frequency',ui('Частота шума','Noise frequency'),0.0001,0.5,0.0001)}${range('noise.amplitude',ui('Амплитуда','Amplitude'),0.0001,5,0.0001)}${range('noise.lacunarity',ui('Лакунарность','Lacunarity'),0.0001,5,0.0001)}${range('noise.persistence',ui('Затухание','Falloff'),0.0001,5,0.0001)}${range('noise.octaves',ui('Октавы','Octaves'),1,20)}${range('noise.motion',ui('Направление движения','Motion direction'),0,3)}${range('noise.motion_speed',ui('Скорость движения','Motion speed'),0,99)}`:''}</details>`;
}
const deviceFieldLabels=new Map([
 ['name','Name'],['enabled','Enabled'],['rate','Refresh rate'],['group','Lighting group'],
 ['preset','Device family'],['role','Output mode'],['layout_line','Arrange in a line'],
 ['layout_grid','Arrange in a grid'],['point','Move an LED'],['leds_line','LED count and layout'],
 ['replace_line','Replace device and layout'],['headers','LEDs per header'],['headers_line','Headers and layout'],
]);
const deviceFieldHints=new Map([
 ['name','Name in the profile.'],['enabled','Allows output when the device is connected.'],
 ['group','Devices in the same group can be turned on or off together.'],
 ['preset','To change the LED count, use Replace device and layout. The new family starts disabled.'],
 ['layout_line','x0,y0,x1,y1. Replaces the existing layout.'],
 ['layout_grid','x,y,width,height,columns,serpentine (true/false).'],
 ['point','index,x,y. LED indices start at zero.'],
 ['leds_line','count;x0,y0,x1,y1. Use a count supported by the driver.'],
 ['replace_line','preset_id;x0,y0,x1,y1. The new family starts disabled.'],
 ['headers','1–256,1–256. To change the total, use Headers and layout.'],
 ['headers_line','N,N;x0,y0,x1,y1. Replaces the existing layout.'],
]);
const deviceFieldChoices=new Map([
 ['enabled:true','Yes'],['enabled:false','No'],['group:pc','Primary lighting'],
 ['group:door','Secondary lighting'],['preset:current','Current'],
 ['role:canvas','Shared canvas'],['role:ambient','Screen lighting'],
]);
function deviceFieldHint(field) {
 if(!english||view!=='devices')return field.hint;
 if(field.key==='rate'){
  const type=state.draftDevices?.[state.editor.index]?.driver?.type;
  return ({
   home_assistant:'Home Assistant: one light, 1 Hz.',
   wled_ddp:'WLED: 1–60 Hz, up to 1 MB RGB/s per output.',
   open_rgb:'OpenRGB: 1–60 Hz. The limit depends on the controller.',
  })[type]??'1–144 Hz.';
 }
 if(field.key==='role'){
  const device=state.draftDevices?.[state.editor.index];
  return device?.driver?.type==='home_assistant'&&device.role==='ambient'
   ?'This Home Assistant light uses an older screen-lighting assignment.'
   :'Changing the output mode turns off the strip and removes its previous layout or assignment. Canvas mode creates a line.';
 }
 return deviceFieldHints.get(field.key)??field.hint;
}
function nativeProperties() {
 const editor=state.editor;
 if(!editor || editor.page!==view)return state.nativeAux?.page===view?renderNativeForm(state.nativeAux):`<div class="native-wait">${ui('Настройки загружаются…','Loading settings…')}</div>`;
 const argb=view==='devices'&&state.argbDevice
  ? `<button id="open-argb" class="button" data-action="open-argb" data-mutation>${ui('Каналы ARGB','ARGB channels')}</button>` : '';
 const repair=state.devices.some(device=>device.id===editor.deviceId)
  ? `<button class="button" data-view="connections" data-connection-device="${esc(editor.deviceId)}" data-mutation ${disabled()}>${ui('Открыть подключение устройства','Open device connection')}</button>` : '';
 const copy=view==='devices'&&repair
  ? `<button class="button" data-action="copy-output-status" data-copy-output="${esc(editor.deviceId)}">${ui('Скопировать статус','Copy status')}</button><span id="output-copy-result" class="muted" role="status"></span>` : '';
 return `<section class="native-properties"><div class="native-inline"><label for="native-target">${ui('Объект','Item')}</label><select id="native-target" class="native-target" data-target data-mutation ${disabled()}>${view==='devices'&&editor.index>=editor.targets.length?`<option value="" disabled selected>${ui('Выберите устройство…','Choose a device…')}</option>`:''}${editor.targets.map((name,i)=>`<option value="${i}" ${i===editor.index?'selected':''}>${esc(name)}</option>`).join('')}</select>${repair}${argb}${copy}</div><div class="parameter-panel">${english?`<details id="native-runtime-diagnostics" ${state.detail?'':'hidden'}><summary>Technical details</summary><p id="native-runtime-detail">${esc(state.detail||'')}</p></details>`:`<p class="parameter-note" id="native-runtime-detail"${view==='devices'?'':' role="status"'}>${esc(state.detail||'')}</p>`}<div class="native-form">${editor.fields.map((field,i)=>`<div class="native-field"><label for="field-${i}">${esc(english&&view==='devices'?deviceFieldLabels.get(field.key)??field.label:field.label)}</label>${field.choices?.length?`<select id="field-${i}" data-field="${esc(field.key)}" data-mutation ${disabled()}>${field.choices.map(choice=>`<option value="${esc(choice.value)}" ${String(choice.value)===String(field.value)?'selected':''}>${esc(english&&view==='devices'?deviceFieldChoices.get(field.key+':'+choice.value)??choice.label:choice.label)}</option>`).join('')}</select>`:`<div class="native-field-file"><input id="field-${i}" data-field="${esc(field.key)}" value="${esc(genericValue(field))}" data-mutation ${disabled()}>${field.file?`<button class="button" data-asset="${esc(field.key)}" data-mutation ${disabled()}>${ui('Выбрать…','Choose…')}</button>`:''}</div>`}${field.hint?`<small>${esc(deviceFieldHint(field))}</small>`:''}</div>`).join('')||`<p class="muted">${ui('Параметров объекта нет.','No settings for this item.')}</p>`}</div><div class="native-property-actions">${action(ui('Применить','Apply'),'apply-document',true,'data-needs-change="document"')}${action(ui('Отменить изменения','Discard changes'),'revert-document')}${action(ui('Повторить','Retry'),'retry-document')}</div></div></section>${state.nativeAux?.page===view?renderNativeForm(state.nativeAux):''}`;
}
const deviceStatus=device=>outputStatusLabel(rawDeviceStatus(device));
function rawDeviceStatus(device) {
 if(!device)return '';
 const labels=state.deviceActivity?.labels;
 if(state.deviceActivity?.reasons?.[device.id]==='endpointLeaseBusy')return 'Подключение занято другим выходом Prismestra';
 if(labels?.[device.id]==='Подключается' && outputRetries.get(device.id)?.awaitingActivity)return 'Кадры отправлены';
 if(labels&&Object.hasOwn(labels,device.id))return labels[device.id];
 return !device.enabled?'Выключено':state.native?'Недоступно':'Предпросмотр';
}
const roleLabels = new Map([['canvas','RGB'],['lcd','LCD'],['ambient','Screen lighting']]);
const transportLabels = new Map([
 ['skydimo_serial',ui('Последовательный порт','Serial port')],['door_ble','Bluetooth LE'],
 ['surplife_e1_pixels','Bluetooth LE'],['wled_ddp','WLED (DDP)'],['open_rgb','OpenRGB'],
 ['home_assistant','Home Assistant'],['tt_lcd','USB HID'],['fusion2','USB HID'],
 ['ene_dram','SMBus'],['valkyrie','USB HID'],['unsupported',ui('Не поддерживается','Unsupported')],
]);
const outputStatuses=new Map([
 ['Выключено','Off'],['Выключается','Turning off'],['Подключается','Connecting'],
 ['Ожидает повтора','Waiting to retry'],['Кадры отправлены','Frames sent'],['Недоступно','Unavailable'],
 ['Предпросмотр','Preview'],['Подключение занято другим выходом Prismestra','Connection in use by another Prismestra output'],
 ['Проверьте доступ Home Assistant','Check Home Assistant access'],
]);
const outputStatusLabel=label=>ui(label,outputStatuses.get(label)??(label?'Status unavailable':''));
async function copyOutputStatus(id) {
 if(view!=='devices'||closing||pendingView||commandPending||state?.editor?.page!=='devices'||state.editor.deviceId!==id)return;
 const device=state.devices.find(item=>item.id===id);
 const feedback=$('#output-copy-result');
 if(!device||!feedback)return;
 const transport=transportLabels.get(device.driver?.type)??ui('Неизвестен','Unknown');
 const rawStatus=rawDeviceStatus(device);
 const safeStatus=outputStatuses.has(rawStatus)?outputStatusLabel(rawStatus):ui('Нет данных о состоянии','Status unavailable');
 const frames=state.deviceActivity?.framesSent?.[id];
 const lines=[ui(`Транспорт: ${transport}`,`Transport: ${transport}`),ui(`Состояние: ${safeStatus}`,`Status: ${safeStatus}`)];
 if(rawStatus==='Кадры отправлены'&&typeof frames==='string'&&/^[1-9][0-9]{0,19}$/.test(frames)){
  lines.push(ui(`Отправлено кадров: ${frames}`,`Frames sent: ${frames}`));
 }
 feedback.textContent='';
 try{
  await navigator.clipboard.writeText(lines.join('\n'));
  feedback.textContent=ui('Статус скопирован','Status copied');
 }catch{
  feedback.textContent=ui('Не удалось скопировать статус','Could not copy status');
 }
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
   ? ui(`Найдено устройство: ${newCandidates[0].label}.`,`Device found: ${newCandidates[0].label}.`)
   : ui(`Найдено устройств: ${newCandidates.length}.`,`Devices found: ${newCandidates.length}.`);
 }
}
function pageButton(page, label=pages[page], primary=false) {
 return `<button class="button${primary?' primary':''}" data-view="${page}">${label}</button>`;
}

function outputPageAvailable(page) {
 return !outputPage(page) || !!state?.devices.some(device=>device.role===page);
}

function startupDiscoveryAction() {
 return state.startupDiscoveryRetry ? action(ui('Найти устройства','Find devices'),'retry-startup-discovery',true) : '';
}
const startupSearchSummary='<p id="startup-empty-search" class="device-summary" role="status" aria-atomic="true" hidden></p>';

function renderDevices(candidates) {
 const firstSetup=state.pending===true && !state.devices.length && candidates.length>0 && !pageHasEdits();
 const labelCounts=new Map();
 for(const {label} of state.devices)labelCounts.set(label,(labelCounts.get(label)??0)+1);
 const devices=state.devices.map(d=>{
  const statusId=`device-status-${encodeURIComponent(d.id)}`;
  const label=labelCounts.get(d.label)>1?`${d.label} (${d.id})`:d.label;
  return `<article class="device">
  <div class="device-symbol">${icon('devices')}</div>
  <div class="device-info"><h2>${esc(label)}</h2><p>${esc(ui(d.driver?.type,transportLabels.get(d.driver?.type)??d.driver?.type))} · ${esc(ui(d.role,roleLabels.get(d.role)??d.role))}</p></div>
  <div class="device-meta"><span>${esc(d.leds)} LED · ${esc(d.rate_hz)} ${ui('Гц','Hz')}</span><small id="${statusId}" data-output-status="${esc(d.id)}">${esc(deviceStatus(d))}</small></div>
  <button class="icon-button" data-device-id="${esc(d.id)}" data-mutation aria-disabled="${busy()}" aria-label="${ui('Свойства','Properties')}: ${esc(label)}" aria-describedby="${statusId}">${icon('edit')}</button>
  <div class="device-retry" hidden><span id="${statusId}-retry-help" data-retry-help hidden></span><button class="button" id="${statusId}-retry" data-output-retry="${esc(d.id)}" aria-describedby="${statusId}-retry-help ${statusId}-retry-status" aria-disabled="true" hidden>${ui('Повторить подключение','Retry connection')}</button><span id="${statusId}-retry-status" data-retry-status role="status"></span></div>
 </article>`;
 });
 const found=candidates.map(candidate=>`<article class="device" data-candidate-id="${esc(candidate.id)}">
  <div class="device-symbol">${icon('devices')}</div>
  <div class="device-info"><h2>${esc(candidate.label)}</h2><p>${esc(ui(candidate.role,roleLabels.get(candidate.role)??candidate.role))}</p></div>
  <div class="device-meta"><small>${firstSetup?ui('Настраивается автоматически','Setting up automatically'):ui('Найдено','Found')}</small></div>
 </article>`);
 const argbLabel=ui('Каналы ARGB','ARGB channels');
 const unconfigured=(state.unconfiguredArgb??[]).map((controller,index)=>`<article class="device">
  <div class="device-symbol">${icon('devices')}</div>
  <div class="device-info"><h2>${esc(controller.label)}</h2><p>OpenRGB · ${ui('нет настроенных LED','no LEDs configured')}</p></div>
  <button id="preview-argb-${index}" class="button" data-action="preview-argb" data-argb-index="${index}" aria-label="${argbLabel}: ${esc(controller.label)}">${argbLabel}</button>
 </article>`);
 const entries=[...devices,...found,...unconfigured].join('')||`<div class="empty"><h2>${ui('В профиле нет устройств','No devices in this profile')}</h2><p>${state.startupDiscoveryRetry?ui('Подключите устройство и запустите поиск.','Connect a device and start a search.'):ui('Добавьте устройство в свойствах профиля.','You can add a device in the profile properties.')}</p></div>`;
 const add = !firstSetup && state.nativeAux?.page==='devices' && state.nativeAux.fields.some(field=>field.id===devicePresetField && field.enabled)
  ? action(ui('Добавить устройство','Add device'),'focus-device-setup',true)
  : '';
 const shortcuts = startupDiscoveryAction() + add + pageButton('connections')
  + (outputPageAvailable('lcd') ? pageButton('lcd',ui('Настроить LCD','Set up LCD')) : '')
  + (outputPageAvailable('ambient') ? pageButton('ambient',ui('Настроить экран','Set up screen lighting')) : '');
 const description=firstSetup
  ? (candidates.length===1?ui('Prismestra добавляет найденное устройство в профиль.','Prismestra is adding the device to this profile.'):ui('Prismestra добавляет найденные устройства в профиль.','Prismestra is adding the devices to this profile.'))
  : ui('Изменения действуют после «Применить». Профиль сохраняется отдельно.','Choose Apply to use your changes. The profile is saved separately.');
 return heading(pages.devices,description,shortcuts)
  + startupSearchSummary
  + `<p id="startup-discovery-help" class="device-summary" hidden>${ui('OpenRGB не определил тип одного из устройств. Проверьте, есть ли подходящий тип в «Добавить устройство». После применения можно настроить привязку в «Подключениях».','OpenRGB could not identify a device type. Look for a matching type under Add device. After applying it, configure the binding in Connections.')}</p>`
  + `<div class="device-list">${entries}</div>`
  + (firstSetup?'':`<div id="device-properties" style="margin-top:24px">${nativeProperties()}</div>`);
}
function renderNativeForm(form=nativeFormForView()) {
 if(!form||form.page!==view)return `<div class="native-wait" role="status">${ui('Загрузка настроек…','Loading settings…')}</div>`;
 const notice=form.notice?`<p class="parameter-note" role="status">${esc(form.notice)}</p>`:'';
 return `<section class="native-properties"><div class="parameter-panel">${notice}<div class="native-form">${form.fields.map((field,index)=>{
  const id='native-field-'+index,common=`id="${id}" data-native-field="${esc(field.id)}" data-native-kind="${esc(field.kind)}" data-native-control ${nativeControlDisabled({dataset:{nativeField:String(field.id)}})?'disabled':''}`;
  if(field.kind==='checkbox')return `<label class="native-group" for="${id}"><input type="checkbox" ${common} ${field.value==='true'?'checked':''}>${esc(field.label)}</label>`;
  const control=field.kind==='select'?`<select ${common}>${!field.choices.some(choice=>String(choice.value)===String(field.value))?`<option value="" disabled selected>${ui('Выберите…','Choose…')}</option>`:''}${field.choices.map(choice=>`<option value="${esc(choice.value)}" ${String(choice.value)===field.value?'selected':''}>${esc(choice.label)}</option>`).join('')}</select>`:`<input ${common} type="${field.kind==='secret'?'password':'text'}" autocomplete="off" value="${field.kind==='secret'?'':esc(nativeValue(field))}">`;
  return `<div class="native-field"><label for="${id}">${esc(field.label)}</label>${control}</div>`;
 }).join('')}</div><div class="native-property-actions">${form.actions.map(item=>`<button class="button" data-native-action="${esc(item.id)}" data-native-control ${nativeControlDisabled({dataset:{nativeAction:String(item.id)}})?'disabled':''}>${esc(item.label)}</button>`).join('')}<button class="button" data-action="discard-page-fields" data-local-discard ${currentPageStaged()?'':'hidden'}>${ui('Отменить незавершённый ввод','Discard unfinished input')}</button></div>${form.notes.map(note=>`<p class="parameter-note">${esc(note)}</p>`).join('')}</div></section>`;
}
function renderSettings() {
 const preference=state.preferredUiLanguage;
 const language=preference==='en'||preference==='ru' ? `<section class="parameter-panel">
   <h2>${ui('Язык приложения','Application language')}</h2>
   <div class="native-field"><label for="ui-language">${ui('Язык','Language')}</label>
   <select id="ui-language" data-mutation aria-disabled="${!!busy()}" aria-describedby="ui-language-status">
    <option value="en" lang="en" ${preference==='en'?'selected':''}>English</option>
    <option value="ru" lang="ru" ${preference==='ru'?'selected':''}>Русский</option>
   </select></div>
   <p id="ui-language-status" class="parameter-note" role="status">${preference!==uiLanguage?ui('Язык сохранён. Полностью выйдите из Prismestra и запустите её снова.','Language saved. Exit Prismestra completely and reopen it.'):ui('Текущий язык редактора.','Current editor language.')}</p>
  </section>` : '';
 const schedule = state.schedule?.enabled
  ? (state.schedule.paused ? ui('Расписание на паузе','Schedule paused') : ui('Расписание активно','Schedule active'))
  : ui('Расписание выключено','Schedule off');
 return heading(pages.settings,ui('Настройки локального профиля.','Settings for the local profile.'))
  + `<div class="settings-sections">${language}
    <section class="parameter-panel"><h2>${ui('Диагностика','Diagnostics')}</h2><div class="settings-links">${pageButton('temperatures')}${pageButton('connections')}${pageButton('sources',ui('Источники температур','Temperature sources'))}</div></section>
    <section class="parameter-panel"><h2>${ui('Алиса','Alice')}</h2><div class="settings-links"><button class="button" data-action="alice-open">${ui('Управление лампами','Lamp controls')}</button></div></section>
    <section class="parameter-panel"><h2>${ui('Расписание','Schedule')}</h2><p class="parameter-note">${schedule}</p><div class="settings-links">${pageButton('schedule',ui('Управлять расписанием','Manage schedule'))}</div></section>
  </div>`;
}

function renderProfilePage() {
 if (view==='settings') return renderSettings();
 if (outputPage(view)) {
  if(detachedOutputEdits()){
   return heading(pages[view],ui('Выход больше не выбран. Незавершённый ввод сохранён для этого выхода.','This output is no longer selected. Unfinished input is kept for this output.'))
    + `<div class="native-property-actions">${action(ui('Отменить незавершённый ввод','Discard unfinished input'),'discard-page-fields')}</div>`;
  }
  const available = outputPageAvailable(view);
  const actions = (available ? pageButton('connections',ui('Проверить подключение','Check connection')) : '')
   + (available && view==='lcd' ? pageButton('temperatures') : '') + pageButton('devices',ui('К устройствам','Back to Devices'));
  return heading(pages[view],available?ui('Настройки локального профиля.','Settings for the local profile.'):ui('В применённом профиле нет такого выхода.','The applied profile has no output of this type.'),actions)
   + nativeProperties();
 }
 return heading(pages[view],ui('Настройки локального профиля.','Settings for the local profile.'),pageButton('settings',ui('К настройкам','Back to Settings')))
  + (advancedPage()?renderNativeForm():nativeProperties());
}

function pageHasEdits() {
 return stagedDirty() || placementDirty() || (state?.nativeForm?.page===view&&state.nativeForm.dirty)
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
 if(!state)$('#main').innerHTML=`<div class="native-wait"><h1>${ui('Подключение к Prismestra…','Connecting to Prismestra…')}</h1><p>${ui('Ожидание локального профиля.','Waiting for the local profile.')}</p></div>`;
 else {$('#main').innerHTML=view==='library'?renderLibrary():view==='editor'?renderScene():view==='devices'?renderDevices(candidates):renderProfilePage();$('.project-context').innerHTML=`<span>${ui('Локальный профиль','Local profile')}</span><small title="${esc(state.profilePath||'')}">${esc(state.profilePath?.split(/[\\/]/).pop()||ui('Не сохранён','Not saved'))}</small>`;}
 document.querySelectorAll('[data-native-kind="secret"]').forEach(input=>{const staged=formStages.get(stageKey('native',input.dataset.nativeField));if(staged)input.value=staged.value;});
 if(view==='devices')window.outputMap?.mount(state,send,currentGenericStages);
 stopCardMotion();syncControls(node?.dataset?.outputRetry);paintPreview();paintLibraryPreviews();
 if(returnKey)restoreFocus(returnKey);
 else if(headingFocused || (node?.dataset?.field && detachedOutputEdits()))focusHeading();
 else if(hiddenNavFocused)$('nav [data-view="devices"]')?.focus({preventScroll:true});
 else restoreFocus(key,selection);
 syncCardMotion();
 syncEditorPreview();
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
let editorPreviewVisible=false;
function syncEditorPreview() {
 const visible=!!state&&view==='editor'&&!document.hidden;
 if(visible===editorPreviewVisible)return;
 if(send({op:'editor_preview_visible',visible})){
  editorPreviewVisible=visible;
  if(!visible)frame=null;
 }
}
function paintLibraryPreviews() {
 document.querySelectorAll('[data-library-preview]').forEach(canvas=>paintPixels(canvas,state?.library.find(item=>String(item.id)===canvas.dataset.libraryPreview)?.preview));
}
const reducedCardMotion=window.matchMedia?.('(prefers-reduced-motion: reduce)');
let cardMotion=null, cardMotionTimer=null, cardRequest=0, lastCardRequest=-Infinity;
function stopCardMotion() {
 clearTimeout(cardMotionTimer);cardMotionTimer=null;
 if(!cardMotion)return;
 const {canvas,id}=cardMotion;
 cardMotion=null;
 if(canvas.isConnected)paintPixels(canvas,state?.library.find(item=>item.id===id)?.preview);
}
function syncCardMotion() {
 const button=view==='library'&&!closing&&!document.hidden&&document.hasFocus?.()!==false&&!reducedCardMotion?.matches
  ? $('.scene-card:hover .scene-preview')||$('.scene-card:focus-within .scene-preview')
  : null;
 const canvas=button?.querySelector?.('[data-library-preview]');
 if(canvas===cardMotion?.canvas)return;
 stopCardMotion();
 if(!canvas)return;
 cardMotion={canvas,id:button.dataset.preview,first:true,pending:null};
 requestCardFrame();
}
function requestCardFrame() {
 if(!cardMotion)return;
 if(!cardMotion.canvas.isConnected||closing||document.hidden){stopCardMotion();return;}
 const now=performance.now();
 if(cardMotion.pending!==null&&now-lastCardRequest>=1000){
  cardMotion.pending=null;cardMotion.first=true;
 }
 if(cardMotion.pending===null&&now-lastCardRequest>=125){
  const request=++cardRequest;
  if(!send({op:'card_preview',id:cardMotion.id,request,start:cardMotion.first})){
   stopCardMotion();return;
  }
  cardMotion.pending=request;lastCardRequest=now;
 }
 cardMotionTimer=setTimeout(requestCardFrame,125);
}
function receiveCardFrame(message) {
 if(!cardMotion||message.id!==cardMotion.id||message.request!==cardMotion.pending)return;
 cardMotion.pending=null;
 if(!message.frame)return;
 cardMotion.first=false;
 paintPixels(cardMotion.canvas,message.frame);
}
function changeView(next, source) {
 if(!Object.hasOwn(pages,next)||!outputPageAvailable(next)||next===view||closing||pendingView)return;
 if(pageHasEdits()){showError(ui('Примените или отмените изменения перед переходом в другой раздел.','Apply or discard changes before leaving this page.'));return;}
 const previous=view;
 const opener = previous==='settings' && settingsPages.includes(next) && source?.closest('.settings-links')
  ? '#main .settings-links '+focusKey(source)
  : null;
 placementOpen=false;
 placementSelection=null;
 placementLimited=false;
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
 draftName=origin?origin.name??ui('Текущая сцена','Current scene'):ui('Восстановленный черновик','Recovered draft');
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
function loadEntry(entry, message=null) {
 editorSceneResult=message?{scene:entry.scene,message}:null;
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
 $('#draft-switch-description').textContent=ui(`Сохранить изменения в сцене «${draftName}» перед открытием «${entry.name}»?`,`Save changes to “${draftName}” before opening “${sceneLabel(entry)}”?`);
 $('#draft-switch').showModal();
 $('#draft-stay').focus();
}
function previewLibraryEntry(entry) {
 $('#library-preview-title').textContent=entry.audioDemo?sceneLabel(entry)+ui(' · Тестовый сигнал',' · Test signal'):sceneLabel(entry);
 $('#library-preview-art').innerHTML=entry.preview?'<canvas aria-hidden="true"></canvas>':palette(entry.scene);
 if(entry.preview)paintPixels($('#library-preview-art canvas'),entry.preview);
 $('#library-preview-dialog').showModal();
}
function activateScene(scene,entry=null,editor=false) {
 if(busy())return;
 const request=++sceneActivationRequest;
 const submitted=clone(scene);
 const command={op:'apply_scene',scene:submitted,request};
 if(entry&&!entry.builtIn)command.id=entry.id;
 if(!send(command))return;
 if(sceneActivation?.id)syncSceneActivation(sceneActivation.id,null);
 sceneActivation={request,scene:submitted,editor,pending:true};
 if(entry)Object.assign(sceneActivation,{
  id:entry.id,name:entry.name,displayName:sceneLabel(entry),binding:entry.aliceLamp,
  profileRevision:state.profileRevision,
  message:ui(`Применяем «${entry.name}»…`,`Applying “${sceneLabel(entry)}”…`)
 });
 if(editor)editorSceneResult=null;
 syncControls();
}
function sceneActivationMessage(result) {
 return result?.lampMessage ? result.message+' '+result.lampMessage : result?.message||'';
}
function clearSceneLamp(id) {
 if(!sceneActivation?.binding || sceneActivation.id!==id)return false;
 sceneActivation.binding=null;
 sceneActivation.lampMessage='';
 return true;
}
function reconcileSceneLamp() {
 if(!sceneActivation?.binding)return;
 const {id,name,scene,binding,profileRevision,applied}=sceneActivation;
 const entry=state.library.find(item=>item.id===id);
 if(profileRevision!==state.profileRevision || !state.native || !entry || entry.name!==name || !same(entry.scene,scene) || !same(entry.aliceLamp,binding) ||
    (applied && !same(state.scene,scene)))clearSceneLamp(id);
}
function syncSceneActivation(id,result) {
 const key=CSS.escape(id);
 const status=$('#scene-activation-'+key);
 const message=sceneActivationMessage(result);
 if(status&&status.textContent!==message)status.textContent=message;
 const devices=$('#scene-devices-'+key);
 if(devices)devices.hidden=!(result?.applied&&state?.native);
}
function receiveSceneActivation({request,status,output,message,lamp}) {
 if(!sceneActivation?.pending||request!==sceneActivation.request)return;
 if(sceneActivation.editor){
  let result;
  if(status==='failed')result=ui(`Не удалось применить сцену.${message?' '+message:''}`,'Could not apply the scene.');
  else if(status==='applied'){
   if(output==='preview_only')result=ui('Применено в предпросмотре.','Applied to preview.');
   else if(output==='no_active_rgb')result=ui('Сцена применена. Нет активных RGB-выходов.','Scene applied. No active RGB outputs.');
   else result=ui('Сцена применена.','Scene applied.');
  }else return;
  editorSceneResult={scene:sceneActivation.scene,message:result};
  if(!sceneActivation.id){
   sceneActivation=null;
   syncControls();
   return;
  }
 }
 if(status==='applied'){
  const detail=output==='preview_only'?ui(' Режим предпросмотра.',' Preview mode.'):output==='no_active_rgb'?ui(' Нет активных RGB-выходов для этой сцены.',' No active RGB outputs for this scene.'):'';
  sceneActivation.message=ui(`Сцена «${sceneActivation.name}» применена.${detail}`,`Scene “${sceneActivation.displayName}” applied.${detail}`);
 }
 else if(status==='failed'){
  sceneActivation.message=ui(`Не удалось применить «${sceneActivation.name}».${message?' '+message:''}`,
   `Could not apply “${sceneActivation.displayName}”. Check Devices for output status.`);
 }
 else return;
 sceneActivation.pending=false;
 sceneActivation.applied=status==='applied';
 if(sceneActivation.applied&&sceneActivation.binding)sceneActivation.lampMessage=aliceStatusText(lamp);
 syncControls();
}
const englishAliceStatus=new Map([
 ['Применяем яркость лампы сцены…','Applying the scene lamp brightness…'],
 ['Яндекс подтвердил команду лампе.','Yandex confirmed the lamp command.'],
 ['Для лампы сцены войдите через Яндекс ID.','Sign in with Yandex ID to use the scene lamp.'],
 ['Дождитесь завершения операции Алисы и примените сцену заново.','Wait for the current Alice operation before applying the scene again.'],
 ['Команда лампе отменена.','The lamp command was cancelled.'],
 ['Яндекс не подтвердил команду. Автоматического повтора не будет.','Yandex did not confirm the command. No automatic retry.'],
 ['Проверьте системное время для входа в Алису.','Check the system clock before signing in to Alice.'],
 ['Доступ к Алисе истёк или отозван. Войдите через Яндекс ID заново.','Alice access expired or was revoked. Sign in with Yandex ID again.'],
 ['Яндекс отклонил доступ. Войдите заново и проверьте права на лампу.','Yandex denied access. Sign in again and check access to the lamp.'],
 ['Подтвердите вход в браузере.','Confirm sign-in in your browser.'],
 ['Загружаем доступные лампы…','Loading available lights…'],
 ['Выберите лампу.','Choose a light.'],
 ['Нет доступных ламп с регулировкой яркости.','No dimmable lights are available.'],
 ['Вы вышли из Алисы в Prismestra.','Signed out of Alice in Prismestra.'],
 ['Завершаем выход из Алисы…','Signing out of Alice…'],
 ['Яндекс подтвердил изменение яркости.','Yandex confirmed the brightness change.'],
 ['Яндекс подтвердил включение лампы.','Yandex confirmed that the light is on.'],
 ['Яндекс подтвердил выключение лампы.','Yandex confirmed that the light is off.'],
]);
function aliceStatusText(message) {
 if(!english||!message)return message;
 return englishAliceStatus.get(message)??(message.endsWith('Автоматического повтора не будет.')
  ?'The lamp command was not confirmed. No automatic retry.'
  :/[А-Яа-яЁё]/u.test(message)?'Alice could not complete the request.':message);
}
function receiveSceneLamp({request,message}) {
 if(sceneActivation?.request!==request||!sceneActivation.applied||!sceneActivation.binding)return;
 sceneActivation.lampMessage=aliceStatusText(message);
 syncControls();
}
function applyEditorScene() {
 if(!draft||placementDirty())return;
 const entry=savedDraft();
 activateScene(draft,state?.native&&entry?.aliceLamp?entry:null,true);
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
function saveDraft(name,id=null,nextId=null,editCreatedFrom=null) {
 const scene=clone(draft);
 if(!send({op:'save_scene',id,name,scene},true))return false;
 editorSceneResult=null;
 savedSceneRequest={id,name,scene,nextId,editCreatedFrom,ids:state.library.map(item=>item.id)};
 return true;
}
function nameDialog(mode) {
 if(!draft||busy())return;
 dialogMode=mode;dialogReturn=focusKey();$('#dialog-title').textContent={new:ui('Новая сцена','New scene'),clone:ui('Создать копию','Make a copy'),rename:ui('Переименовать сцену','Rename scene'),save:ui('Сохранить в библиотеку','Save to library'),'save-switch':ui('Сохранить в библиотеку','Save to library')}[mode];
 $('#scene-name').value=mode==='new'?'':mode==='clone'?(draftName+ui(' · копия',' · copy')).slice(0,48):draftName;$('#scene-name').setCustomValidity('');$('#name-dialog').showModal();$('#scene-name').focus();$('#scene-name').select();
}
window.prismestraReceive = message => {
 try {
  if(typeof message==='string')message=JSON.parse(message);
  if(message.kind==='alice'){
   if(alicePickerStarted&&message.sequence===aliceSequence){
    if(aliceAction==='sign_out'&&!message.error&&clearSceneLamp(sceneActivation?.id))syncControls();
    receiveAlice(message);
   }
   return;
  }
  if(message.kind==='alice_binding'){receiveAliceBinding(message);return;}
  if(message.kind==='scene_lamp'){receiveSceneLamp(message);return;}
  if(message.kind==='output_preview'){window.outputMap?.receive(message);return;}
  if(message.kind==='argb_channels'||message.kind==='argb_channel_saved'){window.argbChannel?.receive(message);return;}
  if(message.kind==='output_retry'){receiveOutputRetry(message);return;}
  if(message.kind==='scene_activation'){receiveSceneActivation(message);return;}
  if(message.kind==='card_preview'){receiveCardFrame(message);return;}
  if(message.kind==='flush'){
   if(!Number.isSafeInteger(message.id)||message.id<0)throw Error('Некорректный запрос закрытия');
   if(window.sceneFile?.blockClose()||window.argbChannel?.blockClose?.()){
    window.ipc.postMessage(JSON.stringify({op:'close_ready',id:message.id,scene:null,blocked:true}));return;
   }
   stageText(document.activeElement);closing=true;clearTimeout(previewTimer);previewTimer=null;previewQueued=false;syncControls();
   window.ipc.postMessage(JSON.stringify({op:'close_ready',id:message.id,scene:draft,origin:draftOrigin,form_dirty:stagedDirty()}));return;
  }
  if(message.kind==='close_cancelled'){closing=false;syncControls();return;}
  if(message.kind==='draft_error'){showRuntimeError(message.message,true);syncControls();return;}
  if(message.kind==='error'){
   if(state?.editor?.page!=='devices')placementOpen=false;awaitingCommandAck=false;closing=false;submittedStages=[];savedSceneRequest=null;commandPending=false;nativeAwaitingAck=false;
   if(pendingView){view=pendingView.previous;pendingView=null;render();focusHeading();}
   if(message.code==='startupDiscoveryRetry'){
    showError(ui('Не удалось повторить поиск устройств. Попробуйте снова, когда поиск станет доступен.',
     'Could not start device search. Try again when search is available.'),true,{detail:message.message,retrySearch:true});
   } else if(message.code==='uiLanguage'){
    const language=$('#ui-language');
    if(language)language.value=state.preferredUiLanguage;
    showError(ui('Не удалось сохранить язык приложения.','Could not save the application language.'),true,{detail:message.message,allowRetry:false});
   }else showRuntimeError(message.message,true);
   syncControls();return;
  }
  if(['scene_file','scene_file_frame','scene_file_lcd_frame'].includes(message.kind)){window.sceneFile?.receive(message);return;}
  if(message.kind==='snapshot') {
   const acknowledged=awaitingCommandAck;awaitingCommandAck=false;completeStageAck();
   if(retryScope()!==message.state.deviceActivity?.retryScope)outputRetries.clear();
   state=message.state;
   reconcileSceneLamp();
   sceneActivationRequest=Math.max(sceneActivationRequest,state.lastSceneApplyRequest??0);
   if(Array.isArray(state.effects)){
    effects=Object.create(null);
    audioEffects=new Set(state.effects.filter(effect=>effect.audio).map(effect=>effect.id));
    contextEffects=new Set(state.effects.filter(effect=>effect.context).map(effect=>effect.id));
    reactiveEffects=new Map(state.effects.filter(effect=>effect.reactive).map(effect=>[effect.id,effect.reactive]));
    directEffects=new Set(state.effects.filter(effect=>effect.directColor).map(effect=>effect.id));
    for(const {id,name} of state.effects)effects[id]=english?titleFromId(id):name;
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
    const {id,name,scene,ids,nextId,editCreatedFrom}=savedSceneRequest;
    const saved=state.library.find(item=>(id?item.id===id:!ids.includes(item.id))&&item.name===name&&same(item.scene,scene));
    if(saved){
     savedSceneRequest=null;
     const savedMessage=ui('Сохранено в библиотеку.','Saved to your library.');
     if(editCreatedFrom===view){
      loadEntry(saved,savedMessage);
      $('#scene-effect')?.focus();
      return;
     }
     setDraftOrigin({id:saved.id,name:saved.name,scene:saved.scene});
     requestPreview();
     const next=state.library.find(item=>item.id===nextId);
     if(next){loadEntry(next);return;}
     editorSceneResult={scene:saved.scene,message:savedMessage};
    }
   }
   checkDraftOrigin();
   followActiveScene();
   render();if(!busy())flushPreview();
  } else if(message.kind==='status'&&state) {
   const searchChanged=(view==='library'||view==='devices')&&Object.hasOwn(message.state,'startupDiscoveryRetry')
    && state.startupDiscoveryRetry!==message.state.startupDiscoveryRetry;
   const firstSetupPendingChanged=view==='devices'&&!state.devices.length&&state.deviceCandidates?.length>0
    && Object.hasOwn(message.state,'pending')&&state.pending!==message.state.pending;
   const foundChanged=view==='devices'&&(
    (Object.hasOwn(message.state,'deviceCandidates')&&!same(state.deviceCandidates,message.state.deviceCandidates)) ||
     (Object.hasOwn(message.state,'unconfiguredArgb')&&!same(state.unconfiguredArgb,message.state.unconfiguredArgb)) ||
    (Object.hasOwn(message.state,'argbDevice')&&state.argbDevice!==message.state.argbDevice)
   );
   const nativeFormChanged=(Object.hasOwn(message.state,'nativeForm')&&!same(state.nativeForm,message.state.nativeForm))||(Object.hasOwn(message.state,'nativeAux')&&!same(state.nativeAux,message.state.nativeAux));
   if(message.state.deviceActivity&&retryScope()!==message.state.deviceActivity.retryScope)outputRetries.clear();
   Object.assign(state,message.state);
   reconcileSceneLamp();
   reconcileRetryActivity(message.state.deviceActivity);
   if(state.pending||Object.hasOwn(message.state,'mapConnection'))window.outputMap?.update(state);
   followActiveScene();
   if(!state.pending&&!nativeAwaitingAck&&!savedSceneRequest)commandPending=false;
   if(editorPreviewVisible&&message.state.preview)frame=message.state.preview;
   if(nativeFormChanged||foundChanged||searchChanged||firstSetupPendingChanged||(!busy()&&!outputPageAvailable(view)))render();else {syncControls();syncDeviceAnnouncement();paintPreview();}if(!busy())flushPreview();
  }
 }catch(e){
  awaitingCommandAck=false;closing=false;commandPending=false;
  showError('Ошибка ответа приложения: '+e.message);
  syncControls();
 }
};
let aliceTimer, aliceState, aliceSequence=0, aliceAction, aliceScene, aliceBindingPending=false, alicePickerStarted=false;
let aliceLamps=[];
function aliceRequest(action, options={}) {
 clearTimeout(aliceTimer);
 aliceAction=action;
 if(!send({op:'alice',sequence:++aliceSequence,request:{action,...options}}) && action!=='cancel')
  receiveAlice({error:'Не удалось связаться с Prismestra.'});
}
function aliceWait() {
 clearTimeout(aliceTimer);
 aliceTimer=setTimeout(()=>{if($('#alice-dialog')?.open)aliceRequest('status');},500);
}
function aliceSelection() {
 const level=$('#alice-brightness');
 const picker=$('#alice-lamp');
 picker.disabled=level.disabled=Boolean(aliceState?.busy);
 const chosen=Boolean(aliceState?.ticket) && !aliceState.busy && picker.value!=='';
 $('#alice-turn-on').disabled=!chosen;
 $('#alice-turn-off').disabled=!chosen;
 $('#alice-apply').disabled=!chosen || !level.value || !level.checkValidity();
 $('#alice-bind').disabled=aliceBindingPending || $('#alice-apply').disabled;
 $('#alice-unbind').disabled=aliceBindingPending;
}
function openAlice(scene) {
 aliceScene=scene;
 aliceBindingPending=false;
 alicePickerStarted=false;
 $('#alice-title').textContent=scene ? ui(`Лампа сцены «${scene.name}»`,`Lamp for “${scene.name}”`) : ui('Управление лампами','Lamp controls');
 const binding=scene?.aliceLamp;
 const stale=scene?.aliceLampStale;
 $('#alice-bound-lamp').textContent=stale ? ui('Сцена изменилась. Лампа не будет участвовать, пока вы заново не сохраните привязку.','The scene changed. Save the lamp binding again to use it with this scene.')
  : binding ? ui(`Выбрана лампа «${binding.label}».`,`Selected lamp: “${binding.label}”.`) : '';
 $('#alice-bound-lamp').hidden=!binding&&!stale;
 $('#alice-unbind').hidden=!binding&&!stale;
 $('#alice-reconfigure').hidden=!stale||!state.native;
 $('#alice-direct-actions').hidden=Boolean(scene);
 $('#alice-scene-actions').hidden=!scene;
 $('#alice-lamp').value='';
 $('#alice-brightness').value=String(binding?.brightness??40);
 $('#alice-allow-power').checked=binding?.allowPowerOn??false;
 $('#alice-dialog').showModal();
 if(stale)receiveAlice({state:{busy:false,lamps:[],needsLogin:false,message:''}});
 else startAlicePicker();
}
function startAlicePicker() {
 if(alicePickerStarted)return;
 alicePickerStarted=true;
 $('#alice-reconfigure').hidden=true;
 receiveAlice({state:{busy:true,lamps:[],needsLogin:true,message:ui('Проверяем вход…','Checking sign-in…')}});
 aliceRequest('open');
}
function configureAliceBinding(selection) {
 if(!aliceScene||aliceBindingPending)return;
 if(!send({op:'configure_scene_lamp',id:aliceScene.id,selection}))return;
 aliceBindingPending=true;
 aliceSelection();
}
function receiveAliceBinding({id,error}) {
 if(!error&&clearSceneLamp(id))syncControls();
 if(!aliceScene||aliceScene.id!==id)return;
 aliceBindingPending=false;
 if(error){$('#alice-status').textContent=aliceStatusText(error);$('#alice-status').title=english?error:'';aliceSelection();return;}
 $('#alice-dialog').close();
}
function receiveAlice(message) {
 const dialog=$('#alice-dialog');
 if(!dialog?.open)return;
 clearTimeout(aliceTimer);
 const retry=dialog.querySelector('[data-action="alice-retry"]');
 const signOut=dialog.querySelector('[data-action="alice-sign-out"]');
 const state=message.error
  ? {...(aliceState||{lamps:[],needsLogin:true}),busy:false,message:message.error}
  : message.state;
 aliceState=state;
 const status=$('#alice-status');
 status.textContent=aliceStatusText(state.message);
 status.title=english&&(status.textContent!==state.message)?state.message:'';
 $('#alice-login').hidden=state.busy || !state.needsLogin;
 retry.hidden=state.busy || !state.saved || state.needsLogin || Boolean(state.ticket);
 signOut.hidden=!state.saved && !state.ticket;
 signOut.disabled=state.busy;
 if(aliceLamps.length!==state.lamps.length || aliceLamps.some((label,index)=>label!==state.lamps[index])){
  aliceLamps=state.lamps;
  const picker=$('#alice-lamp');
  picker.replaceChildren();
  const placeholder=document.createElement('option');
  placeholder.value='';
  placeholder.textContent=ui('Выберите лампу','Choose a light');
  picker.append(placeholder);
  state.lamps.forEach((label,index)=>{
   const option=document.createElement('option');
   option.value=String(index);
   option.textContent=label;
   picker.append(option);
  });
 }
 $('#alice-choice').hidden=!state.lamps.length;
 aliceSelection();
 if(state.busy&&message.sequence!==undefined)aliceWait();
}
$('#alice-dialog')?.addEventListener('close',()=>{
 clearTimeout(aliceTimer);
 aliceState=undefined;
 aliceScene=undefined;
 aliceBindingPending=false;
 $('#alice-lamp').value='';
 $('#alice-choice').hidden=true;
 $('#alice-remember').checked=false;
 if(alicePickerStarted)aliceRequest('cancel');
 alicePickerStarted=false;
});
$('#alice-lamp')?.addEventListener('change',aliceSelection);
$('#alice-brightness')?.addEventListener('input',aliceSelection);
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
 if(b.dataset.preview!==undefined){const entry=state?.library.find(e=>e.id===b.dataset.preview);if(entry)previewLibraryEntry(entry);return;}
 if(b.dataset.edit){const entry=state.library.find(e=>e.id===b.dataset.edit);if(entry)openEntry(entry);return;}
 if(b.dataset.apply){const entry=state.library.find(e=>e.id===b.dataset.apply);if(entry)activateScene(entry.scene,entry);return;}
 if(b.dataset.aliceScene){const entry=state.library.find(e=>e.id===b.dataset.aliceScene);if(entry&&!entry.builtIn)openAlice(entry);return;}
 if(b.dataset.remove){const entry=state.library.find(e=>e.id===b.dataset.remove);if(entry&&!entry.builtIn){dialogReturn=focusKey();$('#remove-description').textContent=ui(`Удалить сцену «${entry.name}» из профиля? Запущенный эффект продолжит работать.`,`Delete “${entry.name}” from the profile? The active effect will keep running.`);$('#remove-dialog').dataset.id=entry.id;$('#remove-dialog').showModal();$('#cancel-remove').focus();}return;}
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
 case 'copy-output-status':void copyOutputStatus(b.dataset.copyOutput);break;
 case 'alice-open':openAlice();break;
 case 'alice-close':$('#alice-dialog').close();break;
 case 'alice-reconfigure':if(aliceScene?.aliceLampStale&&state.native)startAlicePicker();break;
 case 'alice-sign-in':
 case 'alice-retry':
 case 'alice-sign-out':
  if(aliceState?.busy)break;
  receiveAlice({state:{...aliceState,busy:true,lamps:[]}});
  if(b.dataset.action==='alice-sign-in')aliceRequest('sign_in',{remember:$('#alice-remember').checked});
  else aliceRequest(b.dataset.action==='alice-retry'?'open':'sign_out');
  break;
 case 'alice-turn-on':
 case 'alice-turn-off':
 case 'alice-bind':
 case 'alice-apply':{
  const ticket=aliceState?.ticket;
  if(!ticket||aliceState.busy||$('#alice-lamp').value==='')break;
  const lamp=Number($('#alice-lamp').value);
  if(b.dataset.action==='alice-bind'){
   configureAliceBinding({ticket,lamp,brightness:Number($('#alice-brightness').value),allow_power_on:$('#alice-allow-power').checked});
   break;
  }
  receiveAlice({state:{...aliceState,busy:true}});
  if(b.dataset.action!=='alice-apply')aliceRequest('power',{ticket,lamp,on:b.dataset.action==='alice-turn-on'});
  else aliceRequest('apply',{ticket,lamp,brightness:Number($('#alice-brightness').value)});
  break;
 }
 case 'alice-unbind':configureAliceBinding(null);break;
 case 'preview-argb': {
  const index=Number(b.dataset.argbIndex);
  const controller=state?.unconfiguredArgb?.[index];
  if(controller&&view==='devices')window.argbChannel?.openPreview(controller,()=>restoreFocus(`#preview-argb-${index}`));
  return;
 }
 case 'open-argb': {
  const device=state?.devices.find(item=>item.id===state.argbDevice);
  if(device&&view==='devices')window.argbChannel?.open(device.id,device.label,{
   list: request=>send({op:'argb_channels',...request}),
   save: request=>send({op:'argb_channel_save',...request}),
   restoreFocus: ()=>restoreFocus('#open-argb'),
  });
  break;
 }
 case 'focus-device-setup':$(`[data-native-field="${devicePresetField}"]`)?.focus();break;
 case 'retry-startup-discovery':if(!controlDisabled(b))send({op:'retry_startup_discovery'},true);break;
 case 'new':case 'clone':case 'rename':return nameDialog(b.dataset.action);
 case 'save-scene':return nameDialog('save');
 case 'apply-scene':applyEditorScene();break;
 case 'open-placement':openPlacement();break;
 case 'focus-placement':document.getElementById('placement-'+$('#placement-picker')?.value)?.focus();break;
 case 'apply-placement':showPlacementLimit(false);send({op:'apply_document'},true);break;
 case 'insert-palette-color':case 'move-palette-earlier':case 'move-palette-later':return reorderPalette(b.dataset.action);
 case 'add-color':if(draft.colors.length<8)draft.colors.push(draft.colors.at(-1));render();requestPreview();break;
 case 'apply-document':send({op:'apply_fields'},true);break;
 case 'revert-document':showPlacementLimit(false);formStages.clear();submittedStages=[];if(state.editor?.dirty||state.pending||commandPending)send({op:'revert_document'},true);else render();break;
 case 'discard-page-fields':discardPageFields();break;
 case 'discard-fields':discardPageFields();showError(ui('Незавершённый ввод отменён.','Unfinished input discarded.'));break;
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
 if(input.id==='ui-language'){
  if(view!=='settings'||!state)return;
  if(busy()||!['en','ru'].includes(input.value)){
   input.value=state.preferredUiLanguage;
   return;
  }
  if(input.value!==state.preferredUiLanguage && !send({op:'set_ui_language',language:input.value},true))input.value=state.preferredUiLanguage;
  return;
 }
 if(input.id==='placement-picker'){placementSelection=input.value;return;}
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
$('#footer-apply').onclick=applyEditorScene;
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
 const name=$('#scene-name').value.trim();if(!name||name.length>48||/[\u0000-\u001f\u007f]/.test(name)){$('#scene-name').setCustomValidity(ui('Введите название до 48 символов без управляющих знаков','Use up to 48 characters without control characters.'));$('#scene-name').reportValidity();return;}
 const id=['save','rename'].includes(dialogMode)?selectedId:null;
 const editCreatedFrom=['new','clone'].includes(dialogMode)?view:null;
 if(saveDraft(name,id,dialogMode==='save-switch'?draftSwitchId:null,editCreatedFrom)){draftSwitchId=null;$('#name-dialog').close();restoreFocus(dialogReturn);}
};
$('#scene-name').oninput=()=>$('#scene-name').setCustomValidity('');
$('#cancel-name').onclick=()=>{$('#name-dialog').close();draftSwitchId=null;restoreFocus(dialogReturn);};
$('#name-dialog').addEventListener('cancel',()=>{draftSwitchId=null;restoreFocus(dialogReturn);});
const removal=document.createElement('dialog');removal.id='remove-dialog';removal.setAttribute('aria-labelledby','remove-title');removal.innerHTML=`<form id="remove-form"><h2 id="remove-title">${ui('Удалить сцену','Delete scene')}</h2><p id="remove-description"></p><div class="dialog-actions"><button type="button" class="button" id="cancel-remove">${ui('Отмена','Cancel')}</button><button type="submit" class="button primary" data-mutation>${ui('Удалить','Delete')}</button></div></form>`;document.body.append(removal);
$('#cancel-remove').onclick=()=>{removal.close();restoreFocus(dialogReturn);};removal.addEventListener('cancel',()=>restoreFocus(dialogReturn));$('#remove-form').onsubmit=event=>{event.preventDefault();if(send({op:'delete_scene',id:removal.dataset.id},true)){removal.close();restoreFocus(dialogReturn);}};
const projectActions=document.createElement('div');
projectActions.className='native-project-actions';
const exportDescription=ui('Профиль и используемые медиа. «Мои сцены» хранятся отдельно.','The profile and its media. Saved scenes are stored separately.');
projectActions.innerHTML=action(ui('Открыть','Open'),'open')+action(ui('Сохранить','Save'),'save')+
 action(ui('Экспорт профиля','Export profile'),'export',false,`title="${esc(exportDescription)}" aria-describedby="profile-export-scope"`)+
 `<span id="profile-export-scope" class="sr-only">${esc(exportDescription)}</span>`;
$('#save-status').before(projectActions);
document.addEventListener('pointerdown',event=>{
 pointerGesture=true;
 const node=event.target?.closest?.('[data-placement-device]');
 if(!node||event.button!==0||placementDrag||busy()||!state.draftMapRevision)return;
 const {width,height,objects}=state.canvasPlacement;
 const object=objects.find(({id})=>id===node.dataset.placementDevice);
 if(!object)return;
 const [left,top,right,bottom]=object.bounds;
 const stage=node.parentElement.getBoundingClientRect();
 showPlacementLimit(false);
 placementDrag={node,pointer:event.pointerId,x:event.clientX,y:event.clientY,
  cellX:stage.width/width,cellY:stage.height/height,revision:state.draftMapRevision,
  minX:-left,maxX:width-1-right,minY:-top,maxY:height-1-bottom};
 node.setPointerCapture(event.pointerId);
},true);
document.addEventListener('pointermove',event=>{
 if(placementDrag?.pointer!==event.pointerId)return;
 const {dx,dy,limited}=placementDelta(placementDrag,event.clientX,event.clientY);
 const {node,cellX,cellY}=placementDrag;
 node.style.translate=`${dx*cellX}px ${dy*cellY}px`;
 showPlacementLimit(limited);
});
function finishPlacementDrag(event) {
 if(!placementDrag||(event&&placementDrag.pointer!==event.pointerId))return;
 const drag=placementDrag;
 placementDrag=null;
 drag.node.style.translate='';
 if(event?.type==='pointerup'){
  const {dx,dy,limited}=placementDelta(drag,event.clientX,event.clientY);
  showPlacementLimit(limited);
  movePlacement(drag.node.dataset.placementDevice,drag.revision,dx,dy);
 }else showPlacementLimit(false);
}
document.addEventListener('pointerup',event=>{pointerGesture=false;finishPlacementDrag(event);releaseGesture();},true);
document.addEventListener('pointercancel',event=>{pointerGesture=false;finishPlacementDrag(event);releaseGesture();},true);
document.addEventListener('lostpointercapture',finishPlacementDrag);
document.addEventListener('focusin',event=>{
 const device=event?.target?.dataset?.placementDevice;
 if(device){
  placementSelection=device;
  const picker=$('#placement-picker');
  if(picker)picker.value=device;
 }
 syncDeviceAnnouncement();syncOutputRetries();requestAnimationFrame(syncCardMotion);
});
for(const name of ['pointerover','pointerout','focusout','visibilitychange']){
 document.addEventListener(name,()=>{
  if(name==='visibilitychange')syncEditorPreview();
  requestAnimationFrame(syncCardMotion);
 });
}
reducedCardMotion?.addEventListener('change',syncCardMotion);
document.addEventListener('keydown',event=>{
 const device=event.target?.dataset?.placementDevice;
 const delta={ArrowLeft:[-1,0],ArrowRight:[1,0],ArrowUp:[0,-1],ArrowDown:[0,1]}[event.key];
 if(device&&Array.isArray(delta)){
  event.preventDefault();
  if(!placementDrag&&!event.isComposing){showPlacementLimit(false);movePlacement(device,state.draftMapRevision,...delta);}
  return;
 }
 if((event.key===' '||event.key==='Spacebar')&&event.target.closest('button'))keyGesture=true;
 if(event.key!=='Escape'||event.defaultPrevented||event.isComposing||!deviceInspection||
  view!=='devices'||!event.target.closest('#device-properties'))return;
 event.preventDefault();
 restoreFocus(deviceFocusKey(deviceInspection.id));
},true);
document.addEventListener('keyup',()=>{keyGesture=false;releaseGesture();},true);
document.addEventListener('compositionstart',()=>{composing=true;},true);
document.addEventListener('compositionend',()=>{composing=false;releaseGesture();},true);
window.addEventListener('blur',()=>{stopCardMotion();finishPlacementDrag();pointerGesture=false;keyGesture=false;composing=false;releaseGesture();});
document.addEventListener('toggle',event=>{if(event.target.id==='advanced-scene')advancedOpen=event.target.open;},true);
window.addEventListener('pagehide',()=>flushPreview(true));
window.addEventListener('beforeunload',()=>flushPreview(true));
if(english){
 document.documentElement.lang='en';
 document.querySelectorAll('[data-en]').forEach(node=>{node.textContent=node.dataset.en;});
 document.querySelectorAll('[data-en-aria-label]').forEach(node=>node.setAttribute('aria-label',node.dataset.enAriaLabel));
 document.querySelectorAll('[data-en-title]').forEach(node=>{node.title=node.dataset.enTitle;});
 $('nav').setAttribute('aria-label','Main navigation');
 $('.brand').setAttribute('aria-label','Prismestra, scene library');
 $('#appearance-toggle').setAttribute('aria-label','Appearance');
 $('#appearance-toggle').title='Appearance';
 $('#appearance-toggle span').textContent='Appearance';
}
render();send({op:'snapshot'});
