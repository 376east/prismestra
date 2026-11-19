'use strict';

window.outputMap = (() => {
  const dialog = document.getElementById('output-map');
  const table = document.getElementById('output-map-table');
  const status = document.getElementById('output-map-status');
  const back = document.getElementById('output-map-back');
  const next = document.getElementById('output-map-next');
  const refresh = document.getElementById('output-map-refresh');
  let device = null;
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
    if (!send({op: 'preview_output', request, device, offset})) {
      status.textContent = 'Не удалось запросить кадр.';
    }
  }

  function receive(message) {
    if (!dialog.open || message.request !== request) return;
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

  function mount(state, dispatch) {
    const target = document.getElementById('native-target');
    if (!target || state?.editor?.page !== 'devices') return;
    const selected = (state.draftDevices ?? state.devices)[state.editor.index];
    if (!selected || !state.devices.some(item => item.id === selected.id && item.role === 'canvas' && item.leds > 0)) return;
    const button = document.createElement('button');
    button.id = 'output-map-open';
    button.type = 'button';
    button.className = 'button';
    button.dataset.mutation = '';
    button.textContent = 'Проверить LED-карту';
    button.onclick = () => {
      device = selected.id;
      send = dispatch;
      dialog.showModal();
      load(0);
    };
    target.after(button);
  }

  back.onclick = () => load(Math.max(0, offset - pageSize));
  next.onclick = () => load(offset + pageSize);
  refresh.onclick = () => load(offset);
  function close() {
    dialog.close();
    document.getElementById('output-map-open')?.focus({preventScroll:true});
  }
  document.getElementById('output-map-close').onclick = close;
  dialog.addEventListener('cancel', event => {
    event.preventDefault();
    close();
  });
  return {mount, receive};
})();
