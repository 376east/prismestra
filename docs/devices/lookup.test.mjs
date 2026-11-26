import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { buildCatalog, findCards } from './catalog.mjs';
import { families, retailModels } from './openrgb-coverage.mjs';

const load = async (name) => JSON.parse(await readFile(new URL(name, import.meta.url), 'utf8'));
const openrgb = await load('./openrgb.json');
const prismestra = await load('./prismestra.json');
const locked = await load('../../components/openrgb/source-lock.json');
const cards = buildCatalog(openrgb, prismestra);
const find = (query, category, route) => findCards(cards, query, category, route);
const named = (name) => cards.find((card) => card.name === name);

test('catalog retains true provenance and supported evidence labels', () => {
  assert.equal(openrgb.revision, locked.revision);
  assert.ok(openrgb.devices.length > 100);
  assert.ok(prismestra.devices.some((device) => device.status === 'research'));
  const entries = new Set(cards.flatMap((card) => card.entries));
  assert.equal(entries.size, openrgb.devices.length + prismestra.devices.length + retailModels.length - 1);
  assert.deepEqual(find('Debug Controllers'), []);
  for (const entry of entries) {
    assert.ok(entry.name.trim());
    assert.match(entry.source, /^https:\/\//);
    assert.ok(['openrgb', 'no-direct', 'implemented', 'verified', 'research'].includes(entry.status));
    assert.ok(['lighting', 'display'].includes(entry.capability));
    assert.ok(!entry.scope || ['family', 'protocol'].includes(entry.scope));
  }
  for (const [family] of families) assert.ok(openrgb.devices.some(({ name }) => name === family), family);
});

test('known missing Direct mode applies only to checked models on the pinned OpenRGB revision', () => {
  for (const id of ['17EF:619A', '0DB0:0D11']) {
    const [card] = find(id, 'peripherals', 'openrgb');
    assert.equal(card.entries[0].status, 'no-direct');
    assert.match(card.entries[0].modesSource, new RegExp(locked.revision));
  }
  assert.equal(named('Acer Nitro keyboard controller').entries[0].status, 'openrgb');
  const vengeance = named('Corsair Vengeance RGB DDR4');
  assert.equal(vengeance.entries[0].status, 'no-direct');
  assert.match(vengeance.entries[0].modesSource, /RGBController_CorsairVengeance.cpp/);
  const newRevision = buildCatalog({ ...openrgb, revision: 'new' }, null);
  assert.equal(findCards(newRevision, '17EF:619A')[0].entries[0].status, 'openrgb');
});

test('joins exact models with evidence from each route and keeps retail variants separate', () => {
  for (const name of ['Valkyrie VK99', 'Valkyrie VK99 Pro']) {
    const card = named(name);
    assert.equal(card.category, 'peripherals');
    assert.deepEqual(card.entries.map((entry) => entry.route), ['openrgb', 'native']);
  }
  assert.equal(find('VK-99')[0].name, 'Valkyrie VK99');
  assert.equal(find('VK-99', '', 'openrgb')[0], named('Valkyrie VK99'));
  assert.equal(find('VK99 Pro')[0].name, 'Valkyrie VK99 Pro');
  const cases = buildCatalog({ devices: [{ name: 'Model X' }, { name: 'Model X+' }, { name: 'Model X Pro' }] }, null);
  assert.equal(cases.length, 3);
  assert.deepEqual(findCards(cases, 'Model X+').map((card) => card.name), ['Model X+']);
});

test('finds Dark Project by retail model without merging the Valkyrie entries', () => {
  const keyboard = named('Dark Project KD98A');
  for (const query of ['KD98A', 'DP-KD-98A-100100-GMS']) {
    assert.deepEqual(find(query, 'peripherals', 'openrgb'), [keyboard]);
  }
  assert.equal(keyboard.entries.length, 1);
  assert.ok(!find('Valkyrie VK99').includes(keyboard));
  assert.ok(!find('05AC:024F').includes(keyboard));
});

test('finds retail RAM families and keeps the original driver names searchable', () => {
  for (const [driver, names] of [
    ['Kingston Fury DDR4 DRAM', ['Kingston FURY Beast RGB DDR4']],
    ['Kingston Fury DDR5 DRAM', ['Kingston FURY Beast RGB DDR5', 'Kingston FURY Renegade RGB DDR5']],
    ['HyperX DRAM', ['HyperX Fury RGB', 'HyperX Predator RGB']],
    ['Gigabyte RGB Fusion 2 DRAM', ['Gigabyte AORUS RGB Memory DDR4']],
    ['T-Force Xtreem DDR4 DRAM', ['TEAMGROUP T-FORCE XTREEM ARGB DDR4']],
    ['Corsair Vengeance RGB DRAM', ['Corsair Vengeance RGB DDR4']],
  ]) {
    assert.deepEqual(find(driver, 'pc', 'openrgb').slice(0, names.length).map(({ name }) => name), names);
    for (const name of names) assert.equal(find(name)[0], named(name));
  }
  const gskill = named('G.SKILL Trident Z5 DDR5 F5-6400J3239G16G');
  assert.equal(find('F5-6400J3239G16G', 'pc', 'native')[0], gskill);
  assert.equal(find('Trident Z5 Royal', 'pc', 'native')[0], gskill);
  assert.match(gskill.entries[0].note, /part number/);
  assert.match(gskill.entries[0].noteRu, /номер/);
  assert.ok(named('ENE DRAM AUDA0-E6K5-0101'));
});

test('links the B650 EAGLE AX to its native evidence and keeps the Fusion 2 family', () => {
  const model = named('Gigabyte B650 EAGLE AX');
  const family = named('Gigabyte RGB Fusion 2 USB');
  assert.equal(model.category, 'pc');
  assert.deepEqual(model.entries.map((entry) => entry.route), ['openrgb', 'native']);
  assert.deepEqual(family.entries.map((entry) => entry.route), ['openrgb', 'native']);
  assert.equal(model.entries[1], family.entries[1]);
  assert.equal(find('B650 Eagle AX')[0], model);
  assert.ok(!find('B650 Eagle AX').includes(family));
  assert.equal(find('fusion2')[0], family);
  assert.equal(named('Gigabyte B650 EAGLE').entries.length, 1);
});

test('keeps an optical marker as historical evidence on the E1 card', () => {
  const model = named('Surplife E1');
  assert.equal(model.category, 'strips');
  assert.deepEqual(model.entries.map((entry) => entry.status), ['implemented', 'verified']);
  assert.equal(model.entries[1].name, 'Surplife E1 153-point optical marker');
  assert.deepEqual(find('Surplife E1 153-point optical marker'), [model]);
  const preview = named('Surplife BLE 153-point preview profile');
  assert.equal(find('door153')[0], preview);
  assert.ok(find('E104').includes(preview));
  assert.ok(!find('E103').includes(preview));
});

test('separates Kraken lighting from LCD research under the same model', () => {
  const model = named('NZXT Kraken Elite RGB 2024');
  assert.equal(model.category, 'displays');
  assert.deepEqual(model.entries.map((entry) => [entry.route, entry.capability]), [
    ['openrgb', 'lighting'], ['research', 'display'],
  ]);
  assert.deepEqual(find('Kraken 2024 Elite RGB', 'displays', 'research'), [model]);
  assert.deepEqual(find('1E71:3012', 'displays', 'openrgb'), [model]);
  assert.deepEqual(find('1E71:3012', 'displays', 'research'), [model]);
  assert.deepEqual(find('1E71:3012', 'displays', 'native'), []);
  assert.equal(find('1E71:3008', 'displays', 'research').length, 3);
});

test('matches USB pairs and interface IDs exactly across supported formats', () => {
  const model = named('Thermaltake TH420 V2 Ultra EX ARGB Sync');
  for (const query of ['264a:233c', '264A 233C', '264a233c', '0x264a 0x233c',
    'VID_264A&PID_233C', 'USB\\VID_264A&PID_233C&MI_01', '264A:233C MI_01']) {
    assert.deepEqual(find(query, 'displays', 'native'), [model], query);
  }
  assert.equal(model.entries[0].capability, 'display');
  assert.equal(find('TH420 V2 Ultra EX')[0], model);
  assert.equal(find('Thermaltake LCD 264A:233C')[0], model);
  for (const query of ['264a:233d', 'USB\\VID_264A&PID_233C&MI_00', '046d:233c']) assert.deepEqual(find(query), [], query);
  assert.equal(find('046d:c539', '', 'openrgb')[0].name, 'Logitech Lightspeed Receiver');
});

test('search respects word boundaries and recognizes common Russian device types', () => {
  const wled = named('WLED');
  const howl = named('SteelSeries Rival 310 CS:GO Howl Edition');
  assert.deepEqual(find('wled'), [wled]);
  assert.deepEqual(find('WLED Howl Edition'), []);
  assert.deepEqual(find('WLED + Howl Edition'), []);
  assert.deepEqual(find('Howl Edition'), [howl]);
  assert.ok(find('клавиатура', 'peripherals').includes(named('Valkyrie VK99')));
  assert.ok(find('мышь', 'peripherals').includes(howl));
  assert.ok(find('LED лента', 'strips').includes(wled));
  assert.ok(find('монитор', 'displays').includes(named('Alienware AW3225QF')));
  assert.ok(find('экраны', 'displays').includes(named('Thermaltake TH420 V2 Ultra EX ARGB Sync')));
  assert.ok(find('Gigabyte материнская плата', 'pc').includes(named('Gigabyte B650 EAGLE AX')));
  assert.ok(!find('материнская плата').includes(wled));
  assert.deepEqual(find('WLED', 'peripherals'), []);
  assert.deepEqual(find('no-such-product-7e54782a'), []);
});

test('shows a readable Acer label while keeping the exact detector name searchable', () => {
  const model = named('Acer Nitro keyboard controller');
  assert.equal(model.category, 'peripherals');
  assert.equal(model.entries[0].name, 'AcerNitroHidKeyboardController');
  assert.equal(find('Acer Nitro keyboard controller')[0], model);
  assert.deepEqual(find('AcerNitroHidKeyboardController'), [model]);
  assert.match(model.entries[0].note, /0CF2:5130/);
  const asrock = named('ASRock motherboard lighting');
  assert.equal(find('ASRock Motherboard SMBus Controllers', 'pc')[0], asrock);
  assert.match(asrock.entries[0].noteRu, /Polychrome/);
});

test('finds models from the driver tables with the correct source, mode and identity', () => {
  const cases = [
    ['Dominator Titanium', 'Corsair Dominator Titanium RGB DDR5', /CorsairDRAMDevices.cpp/],
    ['RTX 5090 FE', 'NVIDIA GeForce RTX 5090 FE', /NVIDIAIlluminationControllerDetect/],
    ['MEG Z390 ACE', 'MSI MEG Z390 ACE', /MSIMotherboard162Controller.cpp/],
    ['3434:0106', 'Keychron Q1 V2 ANSI', /QMKKeychronController_Devices.cpp/],
    ['Thrustmaster Sol-R Left', 'Thrustmaster Sol-R Left', /ThrustmasterSolControllerDetect.cpp/],
    ['Corsair H115i PRO RGB', 'Corsair H115i PRO RGB', /CorsairHydroControllerDetect.cpp/],
  ];
  for (const [query, name, source] of cases) {
    assert.equal(find(query)[0]?.name, name, query);
    assert.match(find(query)[0].entries[0].source, source);
  }
  const msi = find('MS-7E48', 'pc').map(({ name }) => name);
  assert.deepEqual(msi, ['MSI MEG X870E GODLIKE', 'MSI MEG X870E GODLIKE MAX']);
  assert.equal(find('MPG Z490M GAMING EDGE', 'pc')[0].entries[0].status, 'no-direct');
});

test('keeps wired keyboard variants, precise display capabilities and network and BLE routes separate', () => {
  for (const name of ['Dark Project KD87A', 'Dark Project KD104A', 'AULA F98', 'Pulsar PCMK TKL', 'Redragon K556']) {
    assert.equal(find(name, 'peripherals', 'openrgb')[0], named(name));
    assert.ok(!find('05AC:024F').includes(named(name)));
  }
  assert.equal(find('05AC:024F')[0].name, 'Keychron K3 V2 Optical RGB');
  assert.equal(find('CL-W417-PL14SW-A', 'displays')[0].entries[0].capability, 'display');
  assert.equal(find('US88v1', 'displays')[0].entries[0].capability, 'lighting');
  assert.match(find('US88v1')[0].entries[0].note, /display media/i);
  assert.equal(find('H619A', 'home', 'openrgb')[0].entries[0].connection, 'LAN');
  assert.equal(find('H6053', 'home', 'research')[0].entries[0].status, 'research');
  assert.equal(find('NL42', 'home')[0].name, 'Nanoleaf Shapes');
  assert.equal(named('E1.31').entries[0].scope, 'protocol');
  assert.equal(named('EVision Keyboard 320F:5084').entries[0].scope, 'family');
  assert.equal(named('WLED').entries[0].scope, 'protocol');
});

test('keeps the generic Skydimo serial detector distinct from cataloged models', () => {
  assert.deepEqual(find('1A86:7523').map((card) => card.name), ['Skydimo Serial']);
  assert.equal(named('Skydimo SK0121').entries.length, 1);
  assert.equal(named('Skydimo SK0121').entries[0].route, 'native');
  assert.deepEqual(find('1A86:E316').map((card) => card.name), ['Skydimo SK0902']);
});

test('one unavailable catalog leaves evidence from the other searchable', () => {
  assert.deepEqual(buildCatalog(null, null), []);
  assert.deepEqual(findCards(buildCatalog(openrgb, null), 'VK99')[0].entries.map((entry) => entry.route), ['openrgb']);
  assert.deepEqual(findCards(buildCatalog(null, prismestra), 'VK99')[0].entries.map((entry) => entry.route), ['native']);
});

test('accepts pasted revisions and leads monitor-backlight searches to the right family', () => {
  assert.deepEqual(find('B650 Eagle AX rev. 1.0').map((card) => card.name), ['Gigabyte B650 EAGLE AX']);
  assert.deepEqual(find('Gigabyte B650 EAGLE AX ревизия 1.2').map((card) => card.name), ['Gigabyte B650 EAGLE AX']);
  assert.deepEqual(find(String.raw`USB\VID_264A&PID_233C&REV_0001&MI_01`).map((card) => card.name), ['Thermaltake TH420 V2 Ultra EX ARGB Sync']);
  assert.equal(find(String.raw`USB\VID_264A&PID_233C&REV_0001&MI_03`).length, 0);
  assert.equal(find(String.raw`USB\VID_264A&PID_233C&REV_0001`).length, 1);
  for (const phrase of ['подсветка за монитором', 'lights behind the monitor', 'monitor backlight']) {
    assert.ok(find(phrase).some((card) => card.name === 'Skydimo SK0121'));
    assert.ok(!find(phrase).some((card) => card.name === 'Logitech G502 HERO Gaming Mouse'));
  }
});
