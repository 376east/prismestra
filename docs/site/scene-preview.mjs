const shotCount = 100;
const cutIntervalMs = 1000 / 3;
const columns = 5;
const rows = 4;
const tilesPerSheet = columns * rows;
const sheetCount = shotCount / tilesPerSheet;

export function connectScenePreview() {
  const scene = document.getElementById('scene');
  for (const effect of scene.querySelectorAll('.scene-choice')) {
    effect.addEventListener('click', () => {
      scene.dataset.previewTriggered = '';
      scene.dataset.previewReset = '';
      // Flush the stopped style so selecting the active scene replays its effect.
      void scene.offsetWidth;
      delete scene.dataset.previewReset;
    });
  }
  connectMontage(scene.querySelector('.scene-shot'));
}

function loadImage(name) {
  const image = new Image();
  image.src = new URL(`./shots/${name}.webp`, import.meta.url);
  return image.decode().then(() => image);
}

function connectMontage(shot) {
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
  const sheets = new Map();
  let current = 0;
  let visibleSheet = -1;
  let timer;
  let nextCut;

  function showFrame(index) {
    const sheet = Math.floor(index / tilesPerSheet);
    if (sheet !== visibleSheet) {
      const { photo, mask } = sheets.get(sheet).images;
      photo.className = 'scene-photo';
      photo.alt = '';
      shot.querySelector('.scene-photo').replaceWith(photo);
      shot.style.setProperty('--light-mask', `url("${mask.src}")`);
      shot.style.setProperty('--tile-size', `${columns * 100}% ${rows * 100}%`);
      shot.style.setProperty('--sheet-width', `${columns * 100}%`);
      shot.style.setProperty('--sheet-height', `${rows * 100}%`);
      visibleSheet = sheet;
    }

    const tile = index % tilesPerSheet;
    const column = tile % columns;
    const row = Math.floor(tile / columns);
    const x = column / (columns - 1) * 100;
    const y = row / (rows - 1) * 100;
    shot.style.setProperty('--sheet-x', `${-column / columns * 100}%`);
    shot.style.setProperty('--sheet-y', `${-row / rows * 100}%`);
    shot.style.setProperty('--tile-position', `${x}% ${y}%`);
    shot.dataset.shot = String(index + 1).padStart(3, '0');
  }

  function preload(sheet) {
    if (sheets.has(sheet)) return;
    const entry = {};
    sheets.set(sheet, entry);
    const name = `sets-${sheet + 1}`;
    Promise.all([loadImage(name), loadImage(`${name}-mask`)]).then(([photo, mask]) => {
      entry.images = { photo, mask };
      syncTimer();
    }).catch(() => {
      // The visible frame stays on screen if a download fails.
      sheets.delete(sheet);
    });
  }

  function advance() {
    const next = (current + 1) % shotCount;
    const sheet = Math.floor(next / tilesPerSheet);
    if (!sheets.get(sheet)?.images) {
      preload(sheet);
      return false;
    }

    const oldSheet = visibleSheet;
    showFrame(next);
    current = next;
    if (oldSheet !== sheet) {
      sheets.delete(oldSheet);
      preload((sheet + 2) % sheetCount);
    }
    return true;
  }

  function cut() {
    const advanced = advance();
    const now = performance.now();
    // Resume on the next beat after slow downloads or a long browser stall.
    nextCut = !advanced || now - nextCut >= cutIntervalMs ? now + cutIntervalMs : nextCut + cutIntervalMs;
    timer = setTimeout(cut, Math.max(0, nextCut - now));
  }

  function syncTimer() {
    if (reducedMotion.matches || document.hidden) {
      clearTimeout(timer);
      timer = undefined;
      return;
    }
    if (timer !== undefined) return;

    const sheet = Math.floor(current / tilesPerSheet);
    const nextSheet = (sheet + 1) % sheetCount;
    preload(sheet);
    preload(nextSheet);
    if (!sheets.get(sheet)?.images || !sheets.get(nextSheet)?.images) return;

    showFrame(current);
    preload((sheet + 2) % sheetCount);
    nextCut = performance.now() + cutIntervalMs;
    timer = setTimeout(cut, cutIntervalMs);
  }
  reducedMotion.addEventListener('change', syncTimer);
  document.addEventListener('visibilitychange', syncTimer);
  syncTimer();
}
