'use strict';

window.argbChannel = (() => {
  const element = id => document.getElementById('argb-' + id);
  const dialog = element('dialog');
  const form = element('form');
  const channel = element('channel');
  const template = element('template');
  const count = element('count');
  const options = element('options');
  const preview = element('preview');
  const dots = element('dots');
  const summary = element('summary');
  const status = element('status');
  const save = element('save');
  const close = element('close');
  const catalog = [
    {id:'custom', label:'Точное число LED'},
    {id:'fan', label:'Универсальный вентилятор', shape:'fan'},
    {id:'fan-8', label:'Вентилятор · 8 LED', shape:'fan', leds:8},
    {id:'fan-12', label:'Вентилятор · 12 LED', shape:'fan', leds:12},
    {id:'strip', label:'Универсальная лента', shape:'strip'},
    {id:'strip-10', label:'Лента · 10 LED', shape:'strip', leds:10},
    {id:'strip-30', label:'Лента · 30 LED', shape:'strip', leds:30},
    {id:'strip-60', label:'Лента · 60 LED', shape:'strip', leds:60},
  ];
  let context = null, pending = null, channels = [], request = 0, saved = false, writable = false;
  let exactCount = '';

  const current = () => channels.find(item => item.id === channel.value);
  const bounds = item => {
    if (item?.mode !== 'manual' || !Number.isSafeInteger(item.min) || !Number.isSafeInteger(item.max)) return null;
    const min = Math.max(1, item.min);
    return min <= item.max ? {min, max: item.max} : null;
  };
  const validCount = limits => limits && /^\d+$/.test(count.value) &&
    count.valueAsNumber >= limits.min && count.valueAsNumber <= limits.max;
  const chosenTemplate = () => catalog.find(item => item.id === template.value);

  function drawPreview(length) {
    const visible = length > 12 ? 11 : length;
    dots.dataset.shape = chosenTemplate()?.shape ?? 'strip';
    const values = Array.from({length:visible}, (_,index) => {
      const led = document.createElement('span');
      led.style.setProperty('--angle', `${index*360/visible}deg`);
      return led;
    });
    if (visible < length) {
      const gap = document.createElement('b');
      gap.textContent = '…';
      values.splice(6, 0, gap);
    }
    dots.replaceChildren(...values);
    summary.textContent = length === 1 ? '1 LED' : `LED 1–${length}`;
  }

  function render() {
    const item = current();
    const limits = bounds(item);
    const canPreview = validCount(limits);
    channel.disabled = !!pending || saved || !channels.length;
    options.hidden = !limits;
    template.disabled = !!pending || saved || !limits;
    count.disabled = !limits;
    count.readOnly = !!pending || saved;
    count.setAttribute('aria-invalid', String(!!limits && !!count.value && !canPreview));
    close.setAttribute('aria-disabled', String(pending?.operation === 'save'));
    save.hidden = !writable;
    save.setAttribute('aria-disabled', String(!!pending || saved || !canPreview));
    element('read-only').hidden = !limits || writable;
    preview.hidden = !canPreview;
    if (canPreview) drawPreview(count.valueAsNumber);
  }

  function selectChannel() {
    const item = current();
    const limits = bounds(item);
    if (limits) {
      count.min = limits.min;
      count.max = limits.max;
      count.value = item.leds >= limits.min && item.leds <= limits.max ? item.leds : '';
      element('current').textContent = `Сейчас в канале: ${item.leds} LED.`;
      exactCount = count.value;
      template.replaceChildren(...catalog.map(item => {
        const option = document.createElement('option');
        option.value = item.id;
        option.textContent = item.label;
        option.disabled = item.leds !== undefined && (item.leds < limits.min || item.leds > limits.max);
        return option;
      }));
      template.value = 'custom';
      element('range').textContent = `Допустимое число LED: ${limits.min}–${limits.max}.`;
      status.textContent = '';
    } else {
      count.value = '';
      status.textContent = item?.mode === 'autoUnsupported'
        ? `Текущий размер: ${item.leds} LED. Ручная настройка для этой зоны недоступна.`
        : 'Выберите канал.';
    }
    render();
  }

  function start(operation, data={}) {
    const id = ++request;
    pending = {id, operation};
    status.textContent = operation === 'list' ? 'Загружаем каналы…' : 'Сохраняем настройки канала…';
    render();
    if (!context.api[operation]({device:context.device,request:id,...data})) {
      pending = null;
      status.textContent = 'Не удалось отправить запрос. Повторите попытку.';
      render();
    }
  }

  function receive(message) {
    if (!dialog.open || !pending || context.device !== message.device || pending.id !== message.request) return;
    const {operation} = pending;
    if (operation === 'list' && message.status === 'ready') {
      pending = null;
      channels = message.channels;
      writable = message.writable === true && typeof context.api.save === 'function';
      const placeholder = document.createElement('option');
      placeholder.value = '';
      placeholder.textContent = 'Выберите канал…';
      placeholder.disabled = true;
      channel.replaceChildren(placeholder, ...channels.map(item => {
        const option = document.createElement('option');
        option.value = item.id;
        option.textContent = item.label;
        return option;
      }));
      channel.value = '';
      selectChannel();
      if (!channels.length) status.textContent = 'Контроллер не сообщил доступных каналов.';
      else if (document.activeElement === close) channel.focus({preventScroll:true});
    } else if (operation === 'save' && message.status === 'saved') {
      pending = null;
      saved = true;
      status.textContent = message.message || 'Настройки канала сохранены.';
      render();
      if (document.activeElement === save) close.focus({preventScroll:true});
    } else if (message.status === 'failed') {
      pending = null;
      status.textContent = message.message || 'Не удалось настроить канал.';
      render();
    }
  }

  function open(device, label, api) {
    context = {device, api};
    channels = [];
    saved = writable = false;
    channel.replaceChildren();
    count.value = '';
    element('label').textContent = label;
    dialog.showModal();
    start('list');
  }

  function dismiss() {
    if (pending?.operation === 'save') return;
    const {api} = context;
    context = pending = null;
    dialog.close();
    api.restoreFocus();
  }

  channel.onchange = selectChannel;
  template.onchange = () => {
    count.value = chosenTemplate()?.leds ?? exactCount;
    render();
  };
  count.oninput = () => {
    exactCount = count.value;
    if (chosenTemplate()?.leds !== undefined) template.value = 'custom';
    render();
  };
  form.onsubmit = event => {
    event.preventDefault();
    const item = current();
    if (!writable || pending || saved || !validCount(bounds(item))) return;
    start('save', {channel:item.id,leds:count.valueAsNumber});
  };
  save.onclick = event => {
    if (save.getAttribute('aria-disabled') === 'true') event.preventDefault();
  };
  close.onclick = dismiss;
  dialog.addEventListener('cancel', event => {
    event.preventDefault();
    dismiss();
  });
  return {open, receive};
})();
