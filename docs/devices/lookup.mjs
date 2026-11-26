import { buildCatalog, findCards, modelWithoutRevision } from './catalog.mjs';
import { connectWindowsSetup } from '../setup-download.mjs';

const pageSize = 24;
const exampleQueries = ['B650 Eagle AX', 'Surplife E1', 'NZXT Kraken'];
const filters = ['peripherals', 'pc', 'strips', 'displays', 'home', 'other', 'route:openrgb', 'route:native', 'route:research'];

const copy = {
  en: {
    title: 'Check your devices', intro: 'Find your model to see what Prismestra can control and how to set it up.',
    installNote: 'OpenRGB comes with Prismestra. One installer is all you need.',
    download: 'Get Windows Setup', manualDownload: 'All release files', device: 'Model or device type', filter: 'Category', all: 'All devices',
    examples: 'Try', results: 'Matching devices', more: 'Show more', report: 'Report a device',
    guide: 'Compatibility guide', catalog: 'About the catalog', upstream: 'OpenRGB development catalog',
    placeholder: 'Surplife E1, keyboard, B650 Eagle AX…', loading: 'Loading devices…',
    empty: 'No matches. Try a shorter model name or clear the category. If it is still missing, tell us the exact model.',
    unavailable: 'The catalog could not be loaded. See the compatibility guide.',
    partial: ' Part of the catalog could not be loaded.', details: 'Technical details',
    byDevice: 'Device type', bySource: 'Connection source',
    sources: { openrgb: 'OpenRGB source', native: 'Prismestra source', research: 'Technical reference' },
    checked: 'Hardware check', noCheck: 'The catalog uses Prismestra drivers and the OpenRGB version included in Setup.',
    lighting: 'Lighting', display: 'LCD', openrgb: 'Built-in OpenRGB', 'no-direct': 'Built-in OpenRGB',
    implemented: 'Prismestra driver', research: 'Research', unconfirmed: 'Model identification',
    board: 'Check the two ARGB headers in Devices.', wired: 'Connect by cable.',
    devicesStep: 'Open Devices in Prismestra to set it up.', lcdStep: 'Open LCD in Prismestra to choose what to display.',
    skydimo: 'Look for an SK model code on the controller or in its documentation.',
    revision: 'Check your hardware revision in the source notes.',
    featureStatuses: {
      implemented: 'Supported', openrgb: 'Supported', 'no-direct': 'Scenes unavailable',
      verified: 'Supported', research: 'Not supported yet', unconfirmed: 'Compatibility unconfirmed',
    },
    featureSteps: {
      'no-direct': 'Use the device’s built-in effects. It can’t follow a Prismestra scene.',
    },
    setup: 'Setup guide', modesSource: 'Driver modes', modelSource: 'Model reference',
    scopes: { family: 'Compatible models', protocol: 'Protocol support' },
    familyStep: 'Check the exact model or controller in Devices.',
    categories: ['Keyboards and mice', 'PC components', 'Strips and controllers', 'Displays', 'Home lighting', 'Other devices', 'OpenRGB listings', 'Prismestra connections', 'Research candidates'],
  },
  ru: {
    title: 'Проверьте свои устройства', intro: 'Найдите свою модель, чтобы узнать, чем можно управлять и как подключить устройство.',
    installNote: 'OpenRGB уже входит в Prismestra. Достаточно одного установщика.',
    download: 'Установщик для Windows', manualDownload: 'Все файлы релиза', device: 'Модель или тип устройства', filter: 'Категория', all: 'Все устройства',
    examples: 'Для примера', results: 'Найденные устройства', more: 'Показать ещё', report: 'Сообщить об устройстве',
    guide: 'Справочник совместимости', catalog: 'О каталоге', upstream: 'Каталог новых сборок OpenRGB',
    placeholder: 'Surplife E1, клавиатура, B650 Eagle AX…', loading: 'Загружаем устройства…',
    empty: 'Совпадений нет. Попробуйте сократить название или убрать фильтр. Если модель так и не нашлась, сообщите о ней.',
    unavailable: 'Каталог не загрузился. Воспользуйтесь справочником совместимости.',
    partial: ' Часть каталога не загрузилась.', details: 'Технические подробности',
    byDevice: 'Тип устройства', bySource: 'Источник подключения',
    sources: { openrgb: 'Исходники OpenRGB', native: 'Исходники Prismestra', research: 'Технический источник' },
    checked: 'Проверка на устройстве', noCheck: 'Каталог основан на драйверах Prismestra и версии OpenRGB, которая входит в установщик.',
    lighting: 'Подсветка', display: 'LCD', openrgb: 'Встроенный OpenRGB', 'no-direct': 'Встроенный OpenRGB',
    implemented: 'Драйвер Prismestra', research: 'Исследование', unconfirmed: 'Идентификация модели',
    board: 'Проверьте два разъёма ARGB в разделе «Устройства».', wired: 'Подключите кабелем.',
    devicesStep: 'Для настройки откройте «Устройства» в Prismestra.', lcdStep: 'Выберите изображение в разделе «LCD» в Prismestra.',
    skydimo: 'Поищите код модели SK… на контроллере или в его документации.',
    revision: 'Сверьте ревизию устройства с источником.',
    featureStatuses: {
      implemented: 'Поддерживается', openrgb: 'Поддерживается', 'no-direct': 'Сцены недоступны',
      verified: 'Поддерживается', research: 'Пока не поддерживается', unconfirmed: 'Совместимость не подтверждена',
    },
    featureSteps: {
      'no-direct': 'Устройство поддерживает собственные эффекты. В общую сцену оно не включается.',
    },
    setup: 'Инструкция по установке', modesSource: 'Режимы драйвера', modelSource: 'Сведения о модели',
    scopes: { family: 'Совместимые модели', protocol: 'Поддержка протокола' },
    familyStep: 'Сверьте точную модель или контроллер в разделе «Устройства».',
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

function deviceRow(card, t, lang) {
  const row = node('li');
  row.append(node('h2', card.name));
  const evidence = node('details');
  evidence.append(node('summary', t.details));
  const capabilities = new Set(card.entries.map((entry) => entry.capability));
  for (const capability of capabilities) {
    const entries = card.entries.filter((entry) => entry.capability === capability);
    const status = ['implemented', 'verified', 'openrgb', 'no-direct', 'research', 'unconfirmed'].find((name) => entries.some((entry) => entry.status === name));
    const supported = entries.filter((entry) => ['implemented', 'verified', 'openrgb'].includes(entry.status));
    const scope = supported[0]?.scope;
    const scoped = scope && supported.every((entry) => entry.scope === scope);
    const feature = node('p', '', 'feature');
    feature.append(node('strong', t[capability]), ' · ', scoped ? t.scopes[scope] : t.featureStatuses[status]);
    row.append(feature);
    const note = entries.find((entry) => entry.note)?.[lang === 'ru' ? 'noteRu' : 'note'];
    if (status === 'research' || status === 'unconfirmed') {
      const step = node('p', '', 'connection-hint');
      step.append(sourceLink('https://github.com/376east/prismestra/issues/new?template=device.yml', t.report));
      row.append(step);
    } else {
      const step = capability === 'display' ? t.lcdStep : scoped && scope === 'family' ? t.familyStep : t.devicesStep;
      row.append(node('p', t.featureSteps[status] ?? note ?? step, 'connection-hint'));
    }
    const group = node('div', '', 'evidence-group');
    if (capabilities.size > 1) group.append(node('h3', t[capability]));
    for (const entry of entries) {
      const statusText = entry.status === 'verified' ? t.checked : t[entry.status];
      const usbIds = (entry.aliases ?? []).filter((alias) => /^\p{ASCII_Hex_Digit}{4}[: ]\p{ASCII_Hex_Digit}{4}$/u.test(alias));
      group.append(node('p', [statusText, entry.connection, ...usbIds].filter(Boolean).join(' · ')));
      const detail = lang === 'ru' ? entry.detailRu : entry.detail;
      if (detail) group.append(node('p', detail));
      const links = node('p');
      const label = entry.status === 'verified' ? t.checked : t.sources[entry.route];
      for (const [url, text] of [[entry.source, label], [entry.modesSource, t.modesSource], [entry.modelSource, t.modelSource]]) {
        const link = sourceLink(url, text);
        if (!link) continue;
        if (links.childNodes.length) links.append(' · ');
        links.append(link);
      }
      if (links.childNodes.length) group.append(links);
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
  connectWindowsSetup(document.getElementById('download'));
  const el = (id) => document.getElementById(id);
  const params = new URLSearchParams(location.search);
  const lang = params.get('lang') === 'ru' ? 'ru' : 'en';
  const t = copy[lang];
  if (lang === 'ru') el('home').href = '../?lang=ru';
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
    title: t.title, intro: t.intro, 'install-note': t.installNote, download: t.download, 'manual-download': t.manualDownload, 'query-label': t.device,
    'filter-label': t.filter, count: t.loading, report: t.report, guide: t.guide,
    'catalog-label': t.catalog, upstream: t.upstream, more: t.more, 'report-missing': t.report,
    setup: t.setup,
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
    results.replaceChildren(...filtered.slice(0, limit).map((card) => deviceRow(card, t, lang)));
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
