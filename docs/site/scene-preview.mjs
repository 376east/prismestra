const shotOrder = [1, 12, 17, 15, 26, 7, 2, 29, 4, 6, 3, 18, 11, 39, 24, 22, 33, 9, 14, 37, 10, 16, 23, 20, 40, 36, 38, 13, 34, 8, 25, 30, 5, 32, 28, 27, 31, 21, 19, 35];
const cutIntervalMs = 500;
const columns = 5;
const rows = 4;
const tilesPerSheet = columns * rows;

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
  let loading = false;
  let sheets;
  let current = 0;
  let visibleSheet = -1;
  let timer;
  let nextCut;

  function showFrame(index) {
    const sheet = Math.floor(index / tilesPerSheet);
    if (sheet !== visibleSheet) {
      const photo = sheets[sheet].photo;
      photo.className = 'scene-photo';
      photo.alt = '';
      shot.querySelector('.scene-photo').replaceWith(photo);
      shot.style.setProperty('--light-mask', `url("${sheets[sheet].mask.src}")`);
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
    shot.dataset.shot = String(shotOrder[index]).padStart(2, '0');
  }

  function cut() {
    current = (current + 1) % shotOrder.length;
    showFrame(current);
    const now = performance.now();
    // Hold a full beat after a long browser stall instead of replaying missed cuts.
    nextCut = now - nextCut >= cutIntervalMs ? now + cutIntervalMs : nextCut + cutIntervalMs;
    timer = setTimeout(cut, Math.max(0, nextCut - now));
  }

  function syncTimer() {
    if (reducedMotion.matches || document.hidden) {
      clearTimeout(timer);
      timer = undefined;
    } else if (sheets && timer === undefined) {
      showFrame(current);
      nextCut = performance.now() + cutIntervalMs;
      timer = setTimeout(cut, cutIntervalMs);
    } else if (!loading) {
      loading = true;
      // Prime the complete loop before the first cut so the clock never waits for a request.
      Promise.all([1, 2].map(async (sheet) => {
        const [photo, mask] = await Promise.all([loadImage(`desk-${sheet}`), loadImage(`desk-${sheet}-mask`)]);
        return { photo, mask };
      })).then((ready) => {
        sheets = ready;
        syncTimer();
      }).catch(() => {
        loading = false;
      });
    }
  }

  reducedMotion.addEventListener('change', syncTimer);
  document.addEventListener('visibilitychange', syncTimer);
  syncTimer();
}
