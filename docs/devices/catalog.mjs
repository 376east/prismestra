const board = 'Gigabyte B650 EAGLE AX';
const fusion = 'Gigabyte RGB Fusion 2 USB';
const kraken = 'NZXT Kraken Elite RGB 2024';
const surplife = 'Surplife E1';
const surplifeMarker = 'Surplife E1 153-point optical marker';
const openRgbCardNames = new Map([
  ['NZXT Kraken 2024 ELITE Series RGB', kraken],
  ['AcerNitroHidKeyboardController', 'Acer Nitro keyboard controller'],
]);
const directEvidenceRevision = '6bf7d9f5348be41dcf6a45dc7ad57f00b0b69b43';
const withoutDirect = new Map([
  ['Lenovo Legion K510 Mini Pro', { type: 'Keyboard', source: 'LenovoControllers/LenovoLegionK510Controller/RGBController_LenovoK510.cpp#L38' }],
  ['MSI Clutch GM11 controller', { type: 'Mouse', source: 'MSIClutchController/MSIClutchGM11Controller/RGBController_MSIClutchGM11.cpp#L39' }],
]);

const fold = (value) => String(value ?? '')
  .toLowerCase().normalize('NFKD').replace(/\p{M}/gu, '')
  .replace(/[^\p{L}\p{N}+]+/gu, ' ').trim().replace(/\s*\+\s*/g, '+');
const modelKey = (name) => name.normalize('NFC').trim().replace(/\s+/g, ' ').toLowerCase();
const boardAlias = (name) => /^(?:gigabyte )?b650 eagle ax$/i.test(name);
const driverName = (entry) => (entry.source?.split('/Controllers/')[1]?.split('/')[0] ?? '')
  .replace(/([a-z])([A-Z])/g, '$1 $2');

function entryFor(device, catalog, inherited) {
  const known = inherited ? withoutDirect.get(device.name) : undefined;
  const modes = catalog.revision === directEvidenceRevision ? known?.source : undefined;
  const status = inherited ? (modes ? 'no-direct' : 'openrgb') : device.status;
  return {
    ...device,
    type: device.type ?? known?.type,
    status,
    capability: !inherited && device.type === 'LCD' ? 'display' : 'lighting',
    route: inherited ? 'openrgb' : status === 'research' ? 'research' : 'native',
    routeLabel: device.route,
    source: device.source ?? catalog.source,
    ...(modes && { modesSource: `https://github.com/hello-nexus/openrgb-headless/blob/${directEvidenceRevision}/Controllers/${modes}` }),
  };
}

function categoryFor(card) {
  if (card.entries.some((entry) => entry.capability === 'display')) return 'displays';
  const types = card.entries.map((entry) => entry.type).join(' ');
  const device = fold(card.name + ' ' + types);
  const drivers = card.entries.map(driverName).join(' ').toLowerCase();
  const traits = device + ' ' + drivers;
  if (/\b(light integration|smart bulb|zone light)\b/.test(device)) return 'home';
  if (/\b(led strip|light strip|led controller|skydimo|wled)\b/.test(device) || /\bledstrip\b/.test(drivers)) return 'strips';
  if (/\b(keyboard|mouse|mousemat|headset|headphone|keypad|rival|arctis|sensei)\b/.test(traits)) return 'peripherals';
  if (/\b(monitor|display)\b/.test(traits)) return 'displays';
  if (/\b(motherboard|gpu|ram|dram|memory controller|geforce|radeon|nzxt kraken|cooler|chassis|wraith prism)\b/.test(traits)) return 'pc';
  return 'other';
}

export function buildCatalog(openrgb, native) {
  const cards = new Map();
  function add(name, entry) {
    const key = modelKey(name);
    if (!cards.has(key)) cards.set(key, { name, entries: [] });
    cards.get(key).entries.push(entry);
  }
  for (const device of openrgb?.devices ?? []) {
    add(openRgbCardNames.get(device.name) ?? device.name, entryFor(device, openrgb, true));
  }
  for (const device of native?.devices ?? []) {
    const entry = entryFor(device, native, false);
    add(device.name === surplifeMarker ? surplife : device.name, entry);
    // The family listing remains available. The B650 card cites the exact board used for the native implementation.
    if (device.name === fusion && device.aliases?.some(boardAlias)) add(board, entry);
  }
  return [...cards.values()].map((card) => ({ ...card, category: categoryFor(card) }));
}

const usbPair = /^(?:(?:usb|hid)\\)?vid[_\s:=]*([a-f0-9]{4})[&\s:/_-]+pid[_\s:=]*([a-f0-9]{4})$|^(?:0x)?([a-f0-9]{4})\s*[:\s/_-]?\s*(?:0x)?([a-f0-9]{4})$/i;
const usbSuffix = /(?:[&\s]+mi[_\s:=]*([a-f0-9]{2}))$/i;

function parseUsb(value) {
  // Windows can include REV_ before MI_ in the hardware ID.
  value = value.replace(/&rev_[a-f0-9]{4}(?=&|$)/i, '');
  const suffix = value.match(usbSuffix);
  const pair = (suffix ? value.slice(0, suffix.index) : value).trim().match(usbPair);
  return pair ? {
    id: ((pair[1] ?? pair[3]) + (pair[2] ?? pair[4])).toLowerCase(),
    interface: suffix?.[1].toLowerCase(),
  } : null;
}

function entryNames(card, entry) {
  if (card.name === board && entry.name === fusion) {
    return [card.name, ...(entry.aliases ?? []).filter(boardAlias)].map(fold);
  }
  return [card.name, entry.name, ...(entry.aliases ?? []).filter((alias) =>
    card.name !== fusion || !boardAlias(alias))].map(fold);
}

function entryUsb(entry) {
  const ids = [...(entry.aliases ?? []), ...(entry.name.match(/\b[a-f0-9]{4}:[a-f0-9]{4}\b/gi) ?? [])];
  // These two research entries list the complete USB IDs in their evidence descriptions.
  if (entry.capability === 'display' && [kraken, 'NZXT Kraken Elite 2023'].includes(entry.name)) {
    ids.push(...(entry.detail?.match(/\b[a-f0-9]{4}:[a-f0-9]{4}\b/gi) ?? []));
  }
  return ids.map(parseUsb).filter(Boolean);
}

export function modelWithoutRevision(query) {
  return String(query ?? '').trim().replace(/\s+(?:rev(?:ision)?\.?|рев(?:изия)?\.?)\s*[:=]?\s*\d+(?:[.-]\d+)*\s*$/iu, '');
}

function queryTerms(query) {
  if (/^(?:подсветка (?:за|для) монитор[а-я]*|(?:back)?lights? behind (?:the )?monitor|monitor backlight(?:ing)?|ambilight)$/u.test(query)) {
    return ['ambient', 'lighting'];
  }
  const words = query.replace(/(?:^| )материнск[а-я]* плат[а-я]*(?= |$)/g, ' motherboard');
  return words.split(' ').filter(Boolean).map((token) => {
    if (/^клавиатур[а-я]*$/.test(token)) return 'keyboard';
    if (/^мыш[ьи][а-я]*$/.test(token) || token === 'мышка') return 'mouse';
    if (/^лент[а-я]*$/.test(token)) return 'strip';
    if (/^светодиодн[а-я]*$/.test(token)) return 'led';
    if (/^монитор[а-я]*$/.test(token)) return 'monitor';
    if (/^экран[а-я]*$/.test(token)) return 'display';
    return token;
  });
}

function termRank(card, entry, query, terms) {
  const names = entryNames(card, entry);
  if (names.includes(query)) return 0;
  if (names.some((name) => name.startsWith(query + ' '))) return 1;
  const deviceTypes = [entry.type, driverName(entry)];
  if (card.category === 'strips') deviceTypes.push('LED strip');
  if (/^Skydimo /i.test(entry.name)) deviceTypes.push('ambient lighting');
  if (/\b(rival|sensei)\b/i.test(entry.name)) deviceTypes.push('mouse');
  if (entry.capability === 'display' || /\bmonitor\b/i.test(deviceTypes.join(' '))) deviceTypes.push('display');
  const words = [...names, fold(deviceTypes.join(' ')), fold(entry.vendor), fold(entry.connection)]
    .flatMap((value) => value.split(' '));
  return terms.every((term) => words.some((word) => word.startsWith(term))) ? 2 : Infinity;
}

export function findCards(cards, query, category = '', route = '') {
  const usb = parseUsb(String(query ?? '').trim());
  const name = fold(query);
  const terms = queryTerms(name);
  const baseModel = fold(modelWithoutRevision(query));
  const matches = [];
  for (const card of cards) {
    if (category && card.category !== category) continue;
    const entries = route ? card.entries.filter((entry) => entry.route === route) : card.entries;
    if (!entries.length) continue;
    let rank = 0;
    if (usb) {
      if (!entries.some((entry) => entryUsb(entry).some((candidate) =>
        candidate.id === usb.id && (!usb.interface || candidate.interface === usb.interface)))) continue;
    } else if (name) {
      rank = Math.min(...card.entries.map((entry) => termRank(card, entry, name, terms)));
      if (rank === Infinity && baseModel && baseModel !== name) {
        const baseTerms = queryTerms(baseModel);
        rank = 3 + Math.min(...card.entries.map((entry) => termRank(card, entry, baseModel, baseTerms)));
      }
      if (rank === Infinity) continue;
    }
    matches.push({ card, rank });
  }
  return matches.sort((a, b) => a.rank - b.rank || a.card.name.localeCompare(b.card.name, 'en'))
    .map(({ card }) => card);
}
