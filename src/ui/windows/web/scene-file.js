'use strict';

window.sceneFile = (() => {
  const byId = key => document.getElementById('scene-file-' + key);
  const dialog = byId('dialog');
  const controls = byId('controls');
  const status = byId('status');
  const positions = ['x','y','width','height'];
  const edges = [...byId('edges').querySelectorAll('input')];
  const effects = {noise_map:'Карта шума',breathing:'Дыхание',rainbow_wave:'Радужная волна'};
  const copy = value => structuredClone(value);
  let file = null;
  let baseline = null;
  let pending = null;
  let previewRequest = null;
  let sequence = 0;
  let previewTimer = null;
  let dispatch;
  let paint;

  function hasUnsaved() {
    return file !== null && JSON.stringify(file) !== JSON.stringify(baseline);
  }

  function send(type, args = {}) {
    const request = sequence = (sequence + 1) >>> 0;
    return dispatch({op:'scene_file',request,operation:{type,...args}}) ? request : null;
  }

  function syncButtons() {
    controls.disabled = !file || pending !== null;
    byId('open').disabled = pending !== null;
    byId('save').disabled = !file || pending !== null || !validDraft();
    byId('close').disabled = pending !== null;
    byId('revert').disabled = pending !== null || !hasUnsaved();
    byId('add-color').disabled = !file || file.layers.length >= 8;
  }

  function validDraft() {
    const region = file.outputs.lcd?.region;
    const validRegion = !region || (region.width > 0 && region.height > 0 &&
      region.x >= 0 && region.y >= 0 && region.x + region.width <= 10000 &&
      region.y + region.height <= 10000);
    return file.name.length > 0 && validRegion;
  }

  function setCanvas(key, pixels) {
    const canvas = byId(key);
    canvas.hidden = !pixels;
    if (pixels) paint(canvas,pixels);
  }

  function showFrame(frame) {
    setCanvas('canvas',frame.canvas);
    setCanvas('lcd',frame.lcd);
    setCanvas('strip',frame.strip);
  }

  function inputLayer(layer, index) {
    const item = document.createElement('div');
    item.className = 'scene-file-layer';
    const title = document.createElement('p');
    const named = layer.content.type === 'color' ? 'Цвет' : effects[layer.content.settings.effect];
    title.textContent = named;
    item.append(title);
    if (layer.content.type === 'color') {
      const color = document.createElement('input');
      color.type = 'color';
      color.setAttribute('aria-label','Цвет слоя ' + (index + 1));
      color.value = '#' + layer.content.rgb.toString(16).padStart(6,'0');
      color.oninput = () => {
        layer.content.rgb = parseInt(color.value.slice(1),16);
        changed();
      };
      item.append(color);
    }
    const opacity = document.createElement('input');
    opacity.type = 'range';
    opacity.min = '0';
    opacity.max = '100';
    opacity.value = layer.opacity;
    opacity.setAttribute('aria-label','Непрозрачность слоя ' + (index + 1));
    opacity.oninput = () => {
      layer.opacity = Number(opacity.value);
      changed();
    };
    item.append(opacity);
    const remove = document.createElement('button');
    remove.type = 'button';
    remove.className = 'button';
    remove.textContent = 'Убрать';
    remove.setAttribute('aria-label','Убрать слой ' + (index + 1));
    remove.disabled = file.layers.length < 2;
    remove.onclick = () => {
      file.layers.splice(index,1);
      drawLayers();
      byId('add-color').focus();
      changed();
    };
    item.append(remove);
    return item;
  }

  function drawLayers() {
    byId('layers').replaceChildren(...file.layers.map(inputLayer));
  }

  function syncOutputs() {
    const lcd = file.outputs.lcd;
    byId('lcd-mode').value = !lcd ? 'off' : lcd.region ? 'crop' : 'full';
    byId('region').hidden = !lcd?.region;
    if (lcd?.region) {
      for (const key of positions) byId(key).value = lcd.region[key]/100;
    }
    const strip = file.outputs.strip;
    for (const edge of edges) edge.checked = strip?.edges.includes(edge.value) ?? false;
    byId('reverse').checked = strip?.reverse ?? false;
    byId('reverse').disabled = !strip;
    byId('lcd-note').textContent = lcd ? 'Логический выход: '+lcd.target : 'Выход выключен';
    byId('strip-note').textContent = strip ? 'Логический выход: '+strip.target : 'Кромки не выбраны';
    drawOutline();
  }

  function drawOutline() {
    const region = file.outputs.lcd?.region;
    const outline = byId('outline');
    outline.hidden = !region || !validDraft();
    if (outline.hidden) return;
    Object.assign(outline.style,{
      left:region.x/100+'%',top:region.y/100+'%',width:region.width/100+'%',height:region.height/100+'%'
    });
  }

  function showDocument() {
    byId('name').value = file.name;
    drawLayers();
    syncOutputs();
    syncButtons();
  }

  function refresh() {
    clearTimeout(previewTimer);
    if (!dialog.open || pending || !file || !validDraft()) return;
    previewRequest = send('preview',{file});
  }

  function changed() {
    previewRequest = null;
    clearTimeout(previewTimer);
    status.textContent = validDraft() ? '' : 'Проверьте название и границы области LCD.';
    drawOutline();
    syncButtons();
    if (validDraft()) previewTimer = setTimeout(refresh,100);
  }

  function requestFile(type,args={}) {
    if (pending) return;
    const request = send(type,args);
    if (request === null) {
      status.textContent = 'Не удалось связаться с редактором.';
      return;
    }
    pending = {type,request};
    status.textContent = '';
    syncButtons();
  }

  function canLeave() {
    if (pending) {
      status.textContent = 'Дождитесь завершения операции с файлом.';
      return false;
    }
    if (!hasUnsaved()) return true;
    status.textContent = 'Сохраните файл или отмените правки.';
    return false;
  }

  function close() {
    if (!canLeave()) return;
    clearTimeout(previewTimer);
    previewRequest = null;
    send('stop');
    dialog.close();
    byId('launch')?.focus({preventScroll:true});
  }

  function receive(message) {
    if (!dialog.open) return;
    if (message.kind === 'scene_file_frame') {
      if (message.request === previewRequest) showFrame(message.frame);
      return;
    }
    const data = message.data;
    if (message.request === pending?.request) {
      pending = null;
      if (message.error) status.textContent = message.error;
      else if (data?.document) {
        file = data.document;
        baseline = copy(file);
        previewRequest = message.request;
        showDocument();
        showFrame(data.frame);
      } else if (data?.saved) {
        baseline = copy(file);
        status.textContent = 'Файл сцены сохранён.';
      }
      syncButtons();
      if (previewRequest === null) refresh();
    } else if (message.request === previewRequest) {
      if (message.error) status.textContent = message.error;
      else if (data?.frame) showFrame(data.frame);
    }
  }

  function open(draft, sendCommand, renderPixels) {
    if (!draft || dialog.open) return;
    dispatch = sendCommand;
    paint = renderPixels;
    dialog.showModal();
    if (file) refresh();
    else requestFile('from_effect',{scene:draft});
  }

  byId('open').onclick = () => { if (canLeave()) requestFile('open'); };
  byId('save').onclick = () => { if (file && validDraft()) requestFile('save',{file}); };
  byId('close').onclick = close;
  dialog.addEventListener('cancel',event=>{ event.preventDefault(); close(); });
  byId('revert').onclick = () => {
    file = copy(baseline);
    showDocument();
    changed();
  };
  byId('name').oninput = () => {
    file.name = byId('name').value.trim();
    changed();
  };
  byId('add-color').onclick = () => {
    file.layers.push({opacity:35,content:{type:'color',rgb:0x172432}});
    drawLayers();
    changed();
  };
  byId('lcd-mode').onchange = () => {
    const previous = file.outputs.lcd;
    const mode = byId('lcd-mode').value;
    file.outputs.lcd = mode === 'off' ? null : {
      target:previous?.target ?? 'scene-lcd',
      region:mode === 'crop' ? (previous?.region ?? {x:2500,y:2500,width:5000,height:5000}) : null
    };
    syncOutputs();
    changed();
  };
  for (const key of positions) byId(key).oninput = () => {
    const value = Number(byId(key).value);
    file.outputs.lcd.region[key] = Math.round(value*100);
    changed();
  };
  for (const edge of edges) edge.onchange = () => {
    const previous = file.outputs.strip;
    const remaining = (previous?.edges ?? []).filter(value=>value !== edge.value);
    if (edge.checked) remaining.push(edge.value);
    file.outputs.strip = remaining.length ? {
      target:previous?.target ?? 'scene-strip',edges:remaining,reverse:previous?.reverse ?? false
    } : null;
    syncOutputs();
    changed();
  };
  byId('reverse').onchange = () => {
    file.outputs.strip.reverse = byId('reverse').checked;
    changed();
  };

  return {open,receive,blockClose:()=>dialog.open&&!canLeave()};
})();
