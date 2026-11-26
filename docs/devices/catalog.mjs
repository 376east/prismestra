import { familyCoverage, retailModels, revision as coverageRevision } from './openrgb-coverage.mjs';

const board = 'Gigabyte B650 EAGLE AX';
const fusion = 'Gigabyte RGB Fusion 2 USB';
const kraken = 'NZXT Kraken Elite RGB 2024';
const surplife = 'Surplife E1';
const surplifeMarker = 'Surplife E1 153-point optical marker';
const openRgbCardNames = new Map([
  ['NZXT Kraken 2024 ELITE Series RGB', [kraken]],
  ['AcerNitroHidKeyboardController', ['Acer Nitro keyboard controller']],
  ['Kingston Fury DDR4 DRAM', ['Kingston FURY Beast RGB DDR4']],
  ['Kingston Fury DDR5 DRAM', ['Kingston FURY Beast RGB DDR5', 'Kingston FURY Renegade RGB DDR5']],
  ['HyperX DRAM', ['HyperX Fury RGB', 'HyperX Predator RGB']],
  ['Gigabyte RGB Fusion 2 DRAM', ['Gigabyte AORUS RGB Memory DDR4']],
  ['T-Force Xtreem DDR4 DRAM', ['TEAMGROUP T-FORCE XTREEM ARGB DDR4']],
  ['Corsair Vengeance RGB DRAM', ['Corsair Vengeance RGB DDR4']],
  ['ASRock Motherboard SMBus Controllers', ['ASRock motherboard lighting']],
  ['EVGA Motherboard SMBus Controllers', ['EVGA motherboard lighting']],
  ['Keychron Gaming Keyboard 1', ['Keychron K3 V2 Optical RGB']],
  ['Ionico Keyboard', ['Ionico II 17 keyboard']],
  ['Ionico Light Bar', ['Ionico II 17 light bar']],
  ['Lian Li Universal Screen', ['Lian Li 8.8-inch Universal Screen']],
]);
const openRgbAliases = new Map([
  ['Lian Li Universal Screen', ['US88v1', 'Lian Li 8.8″ Universal Screen']],
  ['Elgato Stream Deck MK.2', ['20GBA9901']],
  ['NZXT Hue 2 Ambient', ['NZXT HUE 2 Ambient RGB Lighting Kit']],
  ['Robobloq Monitor Light Strip', ['Robobloq Monitor Sync LED Strip Light']],
  ['Gigabyte GeForce RTX 5070 Gaming OC', ['Gigabyte GeForce RTX 5070 GAMING OC 12G', 'GV-N5070GAMING OC-12GD']],
  ['Corsair Vengeance RGB DDR5', ['CMH32GX5M2B6000Z30K']],
]);
const lightingOnly = new Set([
  'YICO 14 LCD', 'Lian Li Universal Screen', 'Gigabyte Aorus Waterforce X II 360',
  'Corsair iCUE Link System Hub',
]);
const openRgbFamilyNotes = new Map([
  ['AcerNitroHidKeyboardController', ['For Acer Nitro laptops with keyboard controller 0CF2:5130.', 'Для ноутбуков Acer Nitro с контроллером клавиатуры 0CF2:5130.']],
  ['ASRock Motherboard SMBus Controllers', ['For boards with a compatible ASRock RGB LED or Polychrome controller.', 'Для плат с совместимым контроллером ASRock RGB LED или Polychrome.']],
  ['EVGA Motherboard SMBus Controllers', ['Support depends on the lighting controller on the board.', 'Поддержка зависит от контроллера подсветки на плате.']],
  ['Corsair DRAM', ['Support depends on the lighting controller in the memory module.', 'Поддержка зависит от контроллера подсветки в модуле памяти.']],
  ['ENE SMBus DRAM', ['This driver covers several memory brands. Search by the model or part number printed on your module.', 'С этим драйвером работает память разных брендов. Ищите модель или номер с наклейки модуля.']],
]);
const withoutDirect = new Map([
  ['Corsair Vengeance RGB DRAM', { type: 'RAM', source: 'CorsairVengeanceController/RGBController_CorsairVengeance.cpp#L20' }],
  ['Lenovo Legion K510 Mini Pro', { type: 'Keyboard', source: 'LenovoControllers/LenovoLegionK510Controller/RGBController_LenovoK510.cpp#L38' }],
  ['MSI Clutch GM11 controller', { type: 'Mouse', source: 'MSIClutchController/MSIClutchGM11Controller/RGBController_MSIClutchGM11.cpp#L39' }],
  ['Keychron Gaming Keyboard 1', { type: 'Keyboard', source: 'KeychronKeyboardController/RGBController_KeychronKeyboard.cpp#L419' }],
  ['MSI Keyboard MS_1565', { type: 'Keyboard', source: 'MSIKeyboardController/MSIKeyboard1565Controller/RGBController_MSIMysticLightKB.cpp#L94' }],
  ['MSI Katana 15 B13V', { type: 'Keyboard', source: 'MSIKeyboardController/MSIKeyboard1565Controller/RGBController_MSIMysticLightKB.cpp#L94' }],
  ['Winbond Gaming Keyboard', { type: 'Keyboard', source: 'WinbondGamingKeyboardController/RGBController_WinbondGamingKeyboard.cpp#L17' }],
  ['Pulsar PCMK TKL', { type: 'Keyboard', source: 'WinbondGamingKeyboardController/RGBController_WinbondGamingKeyboard.cpp#L17' }],
]);
const evisionV1 = '/EVisionKeyboardController/EVisionKeyboardController/EVisionKeyboardControllerDetect.cpp';
const goveeConnection = {
  type: 'Light integration', connection: 'LAN', scope: 'external-openrgb', connectionGuide: 'govee',
  note: 'Enable LAN Control in Govee Home and add the light’s IP address to a separate OpenRGB server.',
  noteRu: 'Включите LAN Control в Govee Home и добавьте IP-адрес светильника в отдельный сервер OpenRGB.',
};

const fold = (value) => String(value ?? '')
  .toLowerCase().normalize('NFKD').replace(/\p{M}/gu, '')
  .replace(/[^\p{L}\p{N}+]+/gu, ' ').trim().replace(/\s*\+\s*/g, '+');
const modelKey = (name) => name.normalize('NFC').trim().replace(/\s+/g, ' ').toLowerCase();
const boardAlias = (name) => /^(?:gigabyte )?b650 eagle ax$/i.test(name);
const driverName = (entry) => (entry.source?.split('/Controllers/')[1]?.split('/')[0] ?? '')
  .replace(/([a-z])([A-Z])/g, '$1 $2');

function entryFor(device, catalog, inherited) {
  const pinned = inherited && catalog.revision === coverageRevision;
  const known = pinned ? (device.source?.includes(evisionV1) ? {
    type: 'Keyboard', source: 'EVisionKeyboardController/EVisionV2KeyboardController/RGBController_EVisionKeyboard.cpp#L26',
  } : withoutDirect.get(device.name)) : undefined;
  const modes = known?.source;
  const status = inherited ? (modes || device.scenes === 'unavailable' ? 'no-direct' : 'openrgb') : device.status;
  const [note, noteRu] = (inherited && openRgbFamilyNotes.get(device.name)) || [];
  const [scope, familyNote, familyNoteRu] = pinned ? familyCoverage(device.name) : [];
  const aliases = pinned ? openRgbAliases.get(device.name) : undefined;
  const settings = pinned && device.source?.includes('/Controllers/GoveeController/') ? goveeConnection
    : pinned && lightingOnly.has(device.name) ? {
    note: 'Controls RGB lighting. Display media is outside this connection.',
    noteRu: 'Это подключение управляет RGB-подсветкой. Изображения на экран через него не выводятся.',
  } : device.source?.includes('/QMKKeychronController_Devices.cpp') ? {
    note: 'Use a firmware version that exposes Keychron RGB over USB.',
    noteRu: 'Используйте прошивку клавиатуры с поддержкой Keychron RGB по USB.',
  } : {};
  return {
    ...device,
    ...settings,
    type: device.type ?? settings.type ?? known?.type ?? (device.source?.includes('/MSIMotherboardController/') ? 'Motherboard' : undefined),
    status,
    capability: !inherited && device.type === 'LCD' ? 'display' : 'lighting',
    route: inherited || status === 'openrgb' ? 'openrgb' : ['research', 'unconfirmed'].includes(status) ? 'research' : 'native',
    source: device.source ?? catalog.source,
    ...(note && { note, noteRu }),
    ...(scope && { scope }),
    ...(familyNote && { note: familyNote, noteRu: familyNoteRu }),
    ...(aliases && { aliases: [...(device.aliases ?? []), ...aliases] }),
    ...(modes && { modesSource: `https://github.com/hello-nexus/openrgb-headless/blob/${coverageRevision}/Controllers/${modes}` }),
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
  if (/\b(keyboard|mouse|mousemat|headset|headphone|keypad|joystick|rival|arctis|sensei)\b/.test(traits)) return 'peripherals';
  if (/\b(monitor|display|screen|lcd)\b/.test(traits)) return 'displays';
  if (/\b(motherboard|gpu|ram|dram|ddr[345]|memory controller|geforce|radeon|nzxt kraken|cooler|chassis|wraith prism)\b/.test(traits)) return 'pc';
  return 'other';
}

export function buildCatalog(openrgb, native) {
  const cards = new Map();
  function add(name, entry) {
    const key = modelKey(name);
    if (!cards.has(key)) cards.set(key, { name, entries: [] });
    cards.get(key).entries.push(entry);
  }
  const pinned = openrgb?.revision === coverageRevision;
  const inherited = [...(openrgb?.devices ?? []), ...(pinned ? retailModels : [])];
  for (const device of inherited) {
    if (device.name === 'Debug Controllers') continue;
    const entry = entryFor(device, openrgb, true);
    for (const name of (pinned && openRgbCardNames.get(device.name)) || [device.name]) add(name, entry);
  }
  for (const device of native?.devices ?? []) {
    const entry = entryFor(device, native, false);
    add(device.name === surplifeMarker ? surplife : device.model ?? device.name, entry);
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
  const context = fold([...deviceTypes, entry.vendor, entry.connection].join(' ')).split(' ');
  return names.some((name) => {
    const words = [...name.split(' '), ...context];
    return terms.every((term) => words.some((word) => word.startsWith(term)));
  }) ? 2 : Infinity;
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
