'use strict';

window.outputMap = (() => {
  const dialog = document.getElementById('output-map');
  const description = document.getElementById('output-map-description');
  const table = document.getElementById('output-map-table');
  const status = document.getElementById('output-map-status');
  const back = document.getElementById('output-map-back');
  const next = document.getElementById('output-map-next');
  const refresh = document.getElementById('output-map-refresh');
  const applied = document.getElementById('output-map-applied');
  const mode = document.getElementById('output-map-mode');
  const connectionNote = document.getElementById('output-map-connection');
  const closeButton = document.getElementById('output-map-close');
  const connectionMessages = {
    connecting: ['Выход подключается…','Connecting output…'],
    offline: ['Выход отключён.','The output is disconnected.'],
    disabled: ['Выход выключен в применённом профиле.','The output is off in the applied profile.'],
  };
  let device = null;
  let draft = null;
  let profile = null;
  let connection = null;
  let selectionValid = false;
  let opener = '';
  let request = 0;
  let offset = 0;
  let pageSize = 0;
  let send = null;

  function connectionFor(state) {
    return state.mapConnection?.device === device ? state.mapConnection.state : null;
  }

  function connectionMessage() {
    const message = connectionMessages[connection];
    return message && ui(...message);
  }

  function showSource() {
    mode.hidden = applied.hidden = !draft;
    description.textContent = draft
      ? ui('Локальный предпросмотр черновика. Вывод изменится после применения.','Local draft preview. The output changes when you apply the draft.')
      : ui('Снимок цветов сцены по применённой карте. Индексы и координаты начинаются с нуля.','Scene colors on the applied map. LED indices and coordinates start at zero.');
    showConnection();
  }

  function showConnection() {
    const message = connectionMessage();
    connectionNote.hidden = !draft || !message;
    connectionNote.textContent = message ?? '';
  }

  function load(page) {
    request = (request + 1) >>> 0;
    offset = page;
    back.disabled = next.disabled = true;
    table.hidden = true;
    status.title = '';
    const unavailable = connectionMessage() ??
      (connection === 'no_map' ? ui('У выбранного выхода нет применённой LED-карты.','The selected output has no applied LED map.') : null);
    if (!draft && unavailable) {
      status.textContent = unavailable;
      return;
    }
    status.textContent = ui('Получаем кадр…','Getting the frame…');
    if (!send({op: 'preview_output', request, device, offset, source: 'applied', ...draft})) {
      status.textContent = ui('Не удалось запросить кадр.','Could not request the frame.');
    }
  }

  function receive(message) {
    if (!dialog.open || message.request !== request || message.device !== device ||
        message.source !== (draft?.source ?? 'applied') ||
        (draft && message.revision !== draft.revision)) return;
    if (draft && message.availability !== 'ready') {
      status.textContent = ui(message.error ?? 'Предпросмотр черновика недоступен.','Draft preview is unavailable.');
      status.title = english ? message.error ?? '' : '';
      return;
    }
    if (message.error) {
      status.textContent = ui(message.error,'Could not preview the LED map.');
      status.title = english ? message.error : '';
      return;
    }
    const {label, width, height, total, entries} = message.preview;
    if (message.preview.device !== device) return;
    offset = message.preview.offset;
    if (offset === 0) pageSize = entries.length;
    back.disabled = offset === 0;
    next.disabled = offset + entries.length >= total;
    status.textContent = ui(`${label} · холст ${width} × ${height} · LED ${offset}–${offset + entries.length - 1} из ${total}`,
      `${label} · canvas ${width} × ${height} · LEDs ${offset}–${offset + entries.length - 1} of ${total}`);
    const rows = entries.map(({led, x, y, color}) => {
      const row = document.createElement('tr');
      for (const value of [led, x, y]) row.insertCell().textContent = value;
      const cell = row.insertCell();
      const swatch = document.createElement('span');
      const hex = '#' + color.toString(16).padStart(6,'0').toUpperCase();
      swatch.className = 'output-map-swatch';
      swatch.style.backgroundColor = hex;
      swatch.setAttribute('aria-hidden','true');
      cell.append(swatch, hex);
      return row;
    });
    table.tBodies[0].replaceChildren(...rows);
    table.hidden = false;
  }

  function selectedDevice(state) {
    if (state?.editor?.page !== 'devices') return null;
    return (state.draftDevices ?? state.devices)[state.editor.index];
  }

  function update(state) {
    if (!dialog.open || !selectionValid) return;
    if (state.profilePath !== profile || selectedDevice(state)?.id !== device) {
      selectionValid = false;
      applied.disabled = true;
      invalidate(ui('Выбран другой выход. Откройте предпросмотр снова.','The selected output changed. Reopen the preview.'));
      return;
    }
    const nextConnection = connectionFor(state);
    if (nextConnection !== null && connection !== nextConnection) {
      connection = nextConnection;
      showConnection();
      if (!draft) load(0);
    }
    if (draft && !refresh.disabled &&
        (state.pending || state.draftMapRevision !== draft.revision)) {
      invalidate(ui('Черновик изменился. Откройте предпросмотр снова.','The draft changed. Reopen the preview.'));
    }
  }

  function invalidate(message) {
    const focused = document.activeElement;
    request = (request + 1) >>> 0;
    back.disabled = next.disabled = refresh.disabled = true;
    table.hidden = true;
    status.textContent = message;
    if (focused?.disabled) closeButton.focus({preventScroll:true});
  }

  function mount(state, dispatch, stagedFields) {
    const target = document.getElementById('native-target');
    const selected = selectedDevice(state);
    if (!target || !selected) return;
    const buttons = [];
    function button(id, label, isDraft) {
      const node = document.createElement('button');
      node.id = id;
      node.type = 'button';
      node.className = 'button';
      node.dataset.mutation = '';
      node.textContent = label;
      node.onclick = () => {
        device = selected.id;
        profile = state.profilePath;
        connection = connectionFor(state);
        selectionValid = true;
        send = dispatch;
        opener = id;
        draft = isDraft ? {source: 'draft', revision: state.draftMapRevision, fields: stagedFields()} : null;
        applied.disabled = refresh.disabled = false;
        showSource();
        dialog.showModal();
        load(0);
      };
      buttons.push(node);
    }
    if (state.devices.some(item => item.id === selected.id && item.role === 'canvas' && item.leds > 0)) {
      button('output-map-open', ui('Проверить применённую LED-карту','Inspect applied LED map'), false);
    }
    if (state.draftMapRevision && selected.role === 'canvas') {
      button('draft-output-map-open', ui('Предпросмотр черновой LED-карты','Preview draft LED map'), true);
    }
    target.after(...buttons);
  }

  back.onclick = () => load(Math.max(0, offset - pageSize));
  next.onclick = () => load(offset + pageSize);
  refresh.onclick = () => load(offset);
  applied.onclick = () => {
    draft = null;
    refresh.disabled = false;
    showSource();
    load(0);
    refresh.focus({preventScroll:true});
  };
  function close() {
    dialog.close();
    const origin = selectionValid && document.getElementById(opener);
    const target = document.getElementById('native-target');
    const fallback = origin && !origin.disabled ? origin :
      target && !target.disabled ? target : document.querySelector('[data-view="devices"]');
    fallback?.focus({preventScroll:true});
  }
  closeButton.onclick = close;
  dialog.addEventListener('cancel', event => {
    event.preventDefault();
    close();
  });
  return {mount, receive, update};
})();
