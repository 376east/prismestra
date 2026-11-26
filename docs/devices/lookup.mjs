const pageSize = 24;
const fold = (value) => String(value ?? '')
  .normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();

const statuses = {
  openrgb: 'OpenRGB listing',
  implemented: 'Prismestra connection',
  verified: 'Hardware observation',
  research: 'Research candidate',
};

export function indexCatalog(catalog, inherited) {
  return catalog.devices.map((item) => {
    const status = inherited ? 'openrgb' : item.status;
    const route = inherited ? 'openrgb' : status === 'research' ? 'research' : 'native';
    const nameKey = fold(item.name);
    const key = fold([item.name, item.vendor, item.type, item.connection, ...(item.aliases ?? [])].join(' '));
    return { ...item, status, route, nameKey, key, compact: key.replaceAll(' ', ''), source: item.source ?? catalog.source };
  });
}

export function searchDevices(items, query, route = '') {
  const name = fold(query);
  const tokens = name ? name.split(' ') : [];
  return items.filter((item) => (!route || item.route === route)
    && tokens.every((token) => item.key.includes(token) || item.compact.includes(token)))
    .sort((a, b) => {
      const rank = (item) => item.nameKey === name ? 0
        : name && item.nameKey.startsWith(name) ? 1
        : item.status === 'research' ? 3 : 2;
      return rank(a) - rank(b) || a.name.localeCompare(b.name, 'en');
    });
}

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

function deviceRow(item) {
  const row = node('li');
  row.append(node('h2', item.name), node('span', statuses[item.status] ?? 'Catalog entry', 'status'));
  const metadata = [item.vendor, item.type, item.connection].filter(Boolean).join(' · ');
  if (metadata) row.append(node('p', metadata, 'metadata'));
  if (item.detail) row.append(node('p', item.detail, 'details'));
  const link = sourceLink(item.source, 'Evidence');
  if (link) {
    link.setAttribute('aria-label', 'Evidence for ' + item.name);
    row.append(link);
  }
  return row;
}

async function start() {
  const query = document.getElementById('query');
  const route = document.getElementById('route');
  const count = document.getElementById('count');
  const results = document.getElementById('results');
  const more = document.getElementById('more');
  const provenance = document.getElementById('provenance');
  const params = new URLSearchParams(location.search);
  query.value = params.get('q') ?? '';
  route.value = params.get('route') ?? '';
  if (route.selectedIndex < 0) route.value = '';

  const loads = await Promise.allSettled(['openrgb.json', 'prismestra.json'].map(async (path) => {
    const response = await fetch(path);
    if (!response.ok) throw new Error('Catalog request failed: ' + response.status);
    return response.json();
  }));
  const items = loads.flatMap((result, index) => result.status === 'fulfilled'
    ? indexCatalog(result.value, index === 0) : []);
  const missing = loads.filter((result) => result.status === 'rejected').length;
  const openrgb = loads[0].status === 'fulfilled' ? loads[0].value : null;

  if (openrgb) {
    provenance.append('OpenRGB source revision ', node('code', openrgb.revision.slice(0, 7)),
      '. These are detector entries from the bundled backend. Operation depends on the device, connection and firmware.');
    const source = sourceLink(openrgb.source, ' View source.');
    if (source) provenance.append(source);
  }

  let limit = pageSize;
  function render() {
    const filtered = searchDevices(items, query.value, route.value);
    const partial = missing ? ' Part of the catalog could not be loaded.' : '';
    count.textContent = missing === loads.length
      ? 'The device catalog could not be loaded. See the compatibility guide below.'
      : (filtered.length ? filtered.length + ' matching ' + (filtered.length === 1 ? 'entry' : 'entries')
          + (filtered.length > limit ? ' · showing ' + limit : '')
          : 'No matches. Try a shorter model name or report your device.') + partial;
    results.replaceChildren(...filtered.slice(0, limit).map(deviceRow));
    more.hidden = limit >= filtered.length;
  }

  function applyFilters() {
    limit = pageSize;
    const url = new URL(location.href);
    for (const [key, value] of [['q', query.value.trim()], ['route', route.value]]) {
      if (value) url.searchParams.set(key, value);
      else url.searchParams.delete(key);
    }
    history.replaceState(null, '', url);
    render();
  }

  document.getElementById('filters').addEventListener('submit', (event) => {
    event.preventDefault();
    applyFilters();
  });
  query.addEventListener('input', applyFilters);
  route.addEventListener('change', applyFilters);
  more.addEventListener('click', () => {
    limit += pageSize;
    render();
  });
  render();
}

if (typeof document !== 'undefined') start();
