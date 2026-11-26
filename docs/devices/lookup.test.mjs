import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { indexCatalog, searchDevices } from './lookup.mjs';

const load = async (name) => JSON.parse(await readFile(new URL(name, import.meta.url), 'utf8'));
const openrgb = await load('./openrgb.json');
const prismestra = await load('./prismestra.json');
const locked = await load('../../components/openrgb/source-lock.json');
const index = [...indexCatalog(openrgb, true), ...indexCatalog(prismestra, false)];

test('catalog retains true provenance and supported evidence labels', () => {
  assert.equal(openrgb.revision, locked.revision);
  assert.ok(openrgb.devices.length > 100);
  assert.ok(prismestra.devices.some((device) => device.status === 'research'));
  for (const device of index) {
    assert.ok(device.name.trim());
    assert.match(device.source, /^https:\/\//);
    assert.ok(['openrgb', 'implemented', 'verified', 'research'].includes(device.status));
  }
});

test('finds model and USB IDs with case and separator changes', () => {
  assert.ok(searchDevices(index, 'THERMALTAKE').length);
  assert.ok(searchDevices(index, '264a:233c').some((device) => device.route === 'native'));
  assert.ok(searchDevices(index, '264a 233c').some((device) => device.route === 'native'));
  assert.ok(searchDevices(index, 'skydimo', 'native').length > 1);
  assert.ok(searchDevices(index, 'surplife').length);
  assert.ok(searchDevices(index, 'corsair', 'openrgb').length);
  assert.ok(searchDevices(index, '046d:c539', 'openrgb').some((device) => device.name === 'Logitech Lightspeed Receiver'));
});

test('research remains separate from implemented routes', () => {
  assert.ok(searchDevices(index, '', 'research').length);
  assert.ok(searchDevices(index, '', 'research').every((device) => device.status === 'research'));
  assert.ok(searchDevices(index, '', 'native').every((device) => device.status !== 'research'));
  assert.deepEqual(searchDevices(index, 'no-such-product-7e54782a'), []);
});
