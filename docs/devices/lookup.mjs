import { buildCatalog, findCards, modelWithoutRevision } from './catalog.mjs';

const pageSize = 24;
const exampleQueries = ['B650 Eagle AX', 'Surplife E1', 'NZXT Kraken'];
const filters = ['peripherals', 'pc', 'strips', 'displays', 'home', 'other', 'route:openrgb', 'route:native', 'route:research'];

const copy = {
  en: {
    title: 'Find your device', intro: 'Search by model, device type or USB ID.',
    download: 'Download for Windows', device: 'Device', filter: 'Filter', all: 'All devices',
    examples: 'Try', results: 'Matching devices', more: 'Show more', report: 'Report a device',
    guide: 'Compatibility guide', catalog: 'About the catalog', upstream: 'OpenRGB development catalog',
    placeholder: 'Surplife E1, keyboard, 264A:233C…', loading: 'Loading devices…',
    empty: 'No matches. Try a shorter model name, clear the filter or use a USB ID.',
    unavailable: 'The catalog could not be loaded. See the compatibility guide.',
    partial: ' Part of the catalog could not be loaded.', details: 'Connection and evidence',
    byDevice: 'Device type', bySource: 'Connection source',
    source: 'Source', checked: 'What was tested', noCheck: 'A device listing does not establish physical output.',
    lighting: 'Lighting', display: 'LCD', openrgb: 'Listed in bundled OpenRGB',
    implemented: 'Connection in Prismestra', research: 'Under research', displayResearch: 'Output unavailable',
    board: 'Check the two ARGB headers in Devices.', wired: 'Connect by cable.',
    skydimo: 'Look for an SK model code on the controller or in its documentation.',
    revision: 'Check your hardware revision in the source notes.',
    listedHelp: 'For a live scene, the controller needs a usable Direct mode.',
    nativeHelp: 'Check the connection in Devices.',
    researchHelp: 'This feature still needs an adapter or verification.',
    categories: ['Keyboards and mice', 'PC components', 'Strips and controllers', 'Displays', 'Home lighting', 'Other devices', 'OpenRGB listings', 'Prismestra connections', 'Research candidates'],
  },
  ru: {
    title: 'Найти устройство', intro: 'Поиск по модели, типу устройства или USB ID.',
    download: 'Скачать для Windows', device: 'Устройство', filter: 'Фильтр', all: 'Все устройства',
    examples: 'Для примера', results: 'Найденные устройства', more: 'Показать ещё', report: 'Сообщить об устройстве',
    guide: 'Справочник совместимости', catalog: 'О каталоге', upstream: 'Каталог новых сборок OpenRGB',
    placeholder: 'Surplife E1, клавиатура, 264A:233C…', loading: 'Загружаем устройства…',
    empty: 'Совпадений нет. Попробуйте сократить название, убрать фильтр или ввести USB ID.',
    unavailable: 'Каталог не загрузился. Воспользуйтесь справочником совместимости.',
    partial: ' Часть каталога не загрузилась.', details: 'Подключение и проверка',
    byDevice: 'Тип устройства', bySource: 'Источник подключения',
    source: 'Источник', checked: 'Что проверяли', noCheck: 'Запись в каталоге не подтверждает физический вывод.',
    lighting: 'Подсветка', display: 'LCD', openrgb: 'Есть в списке встроенного OpenRGB',
    implemented: 'Есть подключение в Prismestra', research: 'На этапе исследования', displayResearch: 'Вывод пока недоступен',
    board: 'Проверьте два разъёма ARGB в разделе «Устройства».', wired: 'Подключите кабелем.',
    skydimo: 'Поищите код модели SK… на контроллере или в его документации.',
    revision: 'Сверьте ревизию устройства с источником.',
    listedHelp: 'Для вывода общей сцены контроллеру нужен рабочий режим Direct.',
    nativeHelp: 'Проверьте подключение в разделе «Устройства».',
    researchHelp: 'Для этой функции ещё нужен адаптер или проверка.',
    categories: ['Клавиатуры и мыши', 'Компоненты ПК', 'Ленты и контроллеры', 'Экраны', 'Домашнее освещение', 'Другие устройства', 'Каталог OpenRGB', 'Подключения Prismestra', 'Исследуемые устройства'],
  },
};

function node(tag, value = '', className = '') {
  const element = document.createElement(tag);
  element.textContent = value;
  if (className) element.className = className;
  return element;
}

function sourceLink(value, label) {
  if (!value) return null;
  try {
    const url = new URL(value, document.baseURI);
    if (url.protocol !== 'https:' && url.origin !== window.location.origin) return null;
    const link = node('a', label);
    link.href = url.href;
    return link;
  } catch {
    return null;
  }
}

function deviceRow(card, t) {
  const row = node('li');
  row.append(node('h2', card.name));
  const evidence = node('details');
  evidence.append(node('summary', t.details));
  for (const capability of ['lighting', 'display']) {
    const entries = card.entries.filter((entry) => entry.capability === capability);
    if (!entries.length) continue;
    const status = ['implemented', 'openrgb', 'research'].find((name) => entries.some((entry) => entry.status === name));
    if (status) {
      const feature = node('p', '', 'feature');
      const label = capability === 'display' && status === 'research' ? t.displayResearch : t[status];
      feature.append(node('strong', t[capability]), ' · ', label);
      row.append(feature);
    }
    const group = node('div', '', 'evidence-group');
    group.append(node('h3', t[capability]));
    for (const entry of entries) {
      const statusText = entry.status === 'verified' ? t.checked : t[entry.status];
      const usbIds = (entry.aliases ?? []).filter((alias) => /^\p{ASCII_Hex_Digit}{4}[: ]\p{ASCII_Hex_Digit}{4}$/u.test(alias));
      group.append(node('p', [statusText, entry.connection, ...usbIds].filter(Boolean).join(' · ')));
      if (entry.status === 'implemented' && entry.routeLabel) {
        const route = node('p', entry.routeLabel);
        route.lang = 'en';
        group.append(route);
      }
      const hint = entry.status === 'openrgb' ? t.listedHelp
        : entry.status === 'implemented' ? t.nativeHelp
        : entry.status === 'research' ? t.researchHelp : '';
      if (hint) group.append(node('p', hint));
      if (entry.detail) {
        const description = node('p', entry.detail);
        description.lang = 'en';
        group.append(description);
      }
      const source = sourceLink(entry.source, t.source);
      if (source) group.append(source);
    }
    evidence.append(group);
  }
  const hint = card.name === 'Gigabyte B650 EAGLE AX' && card.entries.some((entry) => entry.route === 'native') ? t.board
    : card.name === 'Skydimo Serial' ? t.skydimo
    : /\(wired\)/i.test(card.name) ? t.wired : '';
  if (hint) row.append(node('p', hint, 'connection-hint'));
  row.append(evidence);
  return row;
}

async function start() {
  const el = (id) => document.getElementById(id);
  const params = new URLSearchParams(location.search);
  const lang = params.get('lang') === 'ru' ? 'ru' : 'en';
  const t = copy[lang];
  document.documentElement.lang = lang;
  document.title = t.title + ' · Prismestra';

  const query = el('query');
  const filter = el('filter');
  const count = el('count');
  const results = el('results');
  const more = el('more');
  const reportMissing = el('report-missing');
  const examples = el('examples');
  const language = el('language');
  for (const [id, value] of Object.entries({
    title: t.title, intro: t.intro, download: t.download, 'query-label': t.device,
    'filter-label': t.filter, count: t.loading, report: t.report, guide: t.guide,
    'catalog-label': t.catalog, upstream: t.upstream, more: t.more, 'report-missing': t.report,
  })) el(id).textContent = value;
  results.setAttribute('aria-label', t.results);
  query.placeholder = t.placeholder;
  query.value = params.get('q') ?? '';
  filter.options[0].textContent = t.all;
  const deviceFilters = node('optgroup');
  const sourceFilters = node('optgroup');
  deviceFilters.label = t.byDevice;
  sourceFilters.label = t.bySource;
  filters.forEach((value, index) => {
    const option = node('option', t.categories[index]);
    option.value = value;
    (value.startsWith('route:') ? sourceFilters : deviceFilters).append(option);
  });
  filter.append(deviceFilters, sourceFilters);
  const selected = params.get('category') || (params.has('route') ? 'route:' + params.get('route') : '');
  filter.value = filters.includes(selected) ? selected : '';

  const loads = await Promise.allSettled(['openrgb.json', 'prismestra.json'].map(async (path) => {
    const response = await fetch(path);
    if (!response.ok) throw new Error('Catalog request failed: ' + response.status);
    const catalog = await response.json();
    if (!Array.isArray(catalog.devices)) throw new Error('Invalid catalog');
    return catalog;
  }));
  const [openrgb, native] = loads.map((result) => result.status === 'fulfilled' ? result.value : null);
  const cards = buildCatalog(openrgb, native);
  const missing = loads.filter((result) => result.status === 'rejected').length;
  el('provenance').textContent = t.noCheck;
  if (openrgb) {
    const source = sourceLink(openrgb.source, 'OpenRGB ' + openrgb.revision.slice(0, 7));
    if (source) el('provenance').append(' ', source);
  }

  let limit = pageSize;
  function render() {
    const currentUrl = new URL(location.href);
    const otherLang = lang === 'ru' ? 'en' : 'ru';
    currentUrl.searchParams.set('lang', otherLang);
    language.href = currentUrl.href;
    language.hreflang = otherLang;
    language.lang = otherLang;
    language.textContent = otherLang === 'ru' ? 'Русский' : 'English';

    const active = query.value.trim() || filter.value;
    examples.hidden = Boolean(active) || missing === loads.length;
    const category = filter.value.startsWith('route:') ? '' : filter.value;
    const route = filter.value.startsWith('route:') ? filter.value.slice(6) : '';
    const filtered = active ? findCards(cards, query.value, category, route) : [];
    const size = filtered.length;
    const shown = size > limit ? ' · ' + (lang === 'ru' ? 'показано ' : 'showing ') + limit : '';
    const total = (lang === 'ru' ? 'Найдено: ' : 'Matches: ') + size + shown;
    const message = missing === loads.length ? t.unavailable
      : ((active ? (size ? total : t.empty) : '') + (missing ? t.partial : ''));
    const trimmed = query.value.trim();
    const revision = size > 0 && modelWithoutRevision(trimmed) !== trimmed ? ' ' + t.revision : '';
    count.textContent = message + revision;
    count.hidden = !message;
    reportMissing.hidden = !active || size > 0 || missing === loads.length;
    el('report').hidden = !reportMissing.hidden;
    results.replaceChildren(...filtered.slice(0, limit).map((card) => deviceRow(card, t)));
    results.hidden = !size;
    more.hidden = limit >= size;
  }

  function applyFilters() {
    limit = pageSize;
    const url = new URL(location.href);
    const selected = filter.value;
    const route = selected.startsWith('route:') ? selected.slice(6) : '';
    for (const [key, value] of [['q', query.value.trim()], ['category', route ? '' : selected], ['route', route]]) {
      if (value) url.searchParams.set(key, value);
      else url.searchParams.delete(key);
    }
    history.replaceState(null, '', url);
    render();
  }

  examples.append(node('span', t.examples));
  for (const sample of exampleQueries) {
    const button = node('button', sample);
    button.type = 'button';
    button.addEventListener('click', () => {
      query.value = sample;
      query.focus();
      applyFilters();
    });
    examples.append(button);
  }
  el('filters').addEventListener('submit', (event) => {
    event.preventDefault();
    applyFilters();
  });
  query.addEventListener('input', applyFilters);
  filter.addEventListener('change', applyFilters);
  more.addEventListener('click', () => {
    limit += pageSize;
    render();
  });
  render();
}

if (typeof document !== 'undefined') start();
