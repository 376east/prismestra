'use strict';

window.outputMap = (() => {
  const dialog = document.getElementById('output-map');
  const description = document.getElementById('output-map-description');
  const table = document.getElementById('output-map-table');
  const status = document.getElementById('output-map-status');
  const back = document.getElementById('output-map-back');
  const next = document.getElementById('output-map-next');
  const refresh = document.getElementById('output-map-refresh');
  let device = null;
  let draft = null;
  let opener = '';
  let request = 0;
  let offset = 0;
  let pageSize = 0;
  let send = null;

  function load(page) {
    request = (request + 1) >>> 0;
    offset = page;
    back.disabled = next.disabled = true;
    status.textContent = 'Получаем кадр…';
    table.hidden = true;
    if (!send({op: 'preview_output', request, device, offset, ...draft})) {
      status.textContent = 'Не удалось запросить кадр.';
    }
  }

  function receive(message) {
    if (!dialog.open || message.request !== request) return;
    if (draft && (message.source !== draft.source || message.revision !== draft.revision || message.device !== device)) return;
    if (!draft && message.source === 'draft') return;
    if (draft && message.availability !== 'ready') {
      status.textContent = message.error ?? 'Предпросмотр черновика недоступен.';
      return;
    }
    if (message.error) {
      status.textContent = message.error;
      return;
    }
    const {label, width, height, total, entries} = message.preview;
    if (message.preview.device !== device) return;
    offset = message.preview.offset;
    if (offset === 0) pageSize = entries.length;
    back.disabled = offset === 0;
    next.disabled = offset + entries.length >= total;
    status.textContent = `${label} · холст ${width} × ${height} · LED ${offset}–${offset + entries.length - 1} из ${total}`;
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
    if (!dialog.open || !draft || refresh.disabled) return;
    if (!state?.pending && state?.draftMapRevision === draft.revision && selectedDevice(state)?.id === device) return;
    request = (request + 1) >>> 0;
    back.disabled = next.disabled = refresh.disabled = true;
    table.hidden = true;
    status.textContent = 'Черновик изменился. Откройте предпросмотр снова.';
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
        send = dispatch;
        opener = id;
        draft = isDraft ? {source: 'draft', revision: state.draftMapRevision, fields: stagedFields()} : null;
        description.textContent = isDraft
          ? 'Локальный предпросмотр черновика. Вывод изменится после применения.'
          : 'Снимок цветов сцены по применённой карте. Индексы и координаты начинаются с нуля.';
        refresh.disabled = false;
        dialog.showModal();
        load(0);
      };
      buttons.push(node);
    }
    if (state.devices.some(item => item.id === selected.id && item.role === 'canvas' && item.leds > 0)) {
      button('output-map-open', 'Проверить применённую LED-карту', false);
    }
    if (state.draftMapRevision && selected.role === 'canvas') {
      button('draft-output-map-open', 'Предпросмотр черновой LED-карты', true);
    }
    target.after(...buttons);
  }

  back.onclick = () => load(Math.max(0, offset - pageSize));
  next.onclick = () => load(offset + pageSize);
  refresh.onclick = () => load(offset);
  function close() {
    dialog.close();
    document.getElementById(opener)?.focus({preventScroll:true});
  }
  document.getElementById('output-map-close').onclick = close;
  dialog.addEventListener('cancel', event => {
    event.preventDefault();
    close();
  });
  return {mount, receive, update};
})();
