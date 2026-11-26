const shotCount = 40;
const cutIntervalMs = 500;
const lookahead = 6;

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

function loadImage(url) {
  const image = new Image();
  image.src = url;
  return image.decode().then(() => image);
}

function connectMontage(shot) {
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
  const upcoming = new Map();
  let current = 1;
  let timer;

  function warmNextShots() {
    for (let offset = 1; offset <= lookahead; offset++) {
      const index = (current + offset - 1) % shotCount + 1;
      if (upcoming.has(index)) continue;
      const name = String(index).padStart(2, '0');
      const photoUrl = new URL(`./shots/${name}.webp`, import.meta.url);
      const maskUrl = new URL(`./shots/${name}-mask.webp`, import.meta.url);
      upcoming.set(index, null);
      Promise.allSettled([loadImage(photoUrl), loadImage(maskUrl)]).then(([photo, mask]) => {
        upcoming.set(index, {
          name,
          photo: photo.status === 'fulfilled' ? photo.value : null,
          mask: mask.status === 'fulfilled' ? mask.value.src : null,
        });
      });
    }
  }

  function cut() {
    const next = current % shotCount + 1;
    const frame = upcoming.get(next);
    if (!frame) return;

    if (frame.photo) {
      frame.photo.className = 'scene-photo';
      frame.photo.alt = '';
      frame.photo.width = 960;
      frame.photo.height = 640;
      shot.querySelector('.scene-photo').replaceWith(frame.photo);
      shot.style.setProperty('--light-mask', frame.mask ? `url("${frame.mask}")` : 'linear-gradient(transparent, transparent)');
      shot.dataset.shot = frame.name;
    }

    upcoming.delete(next);
    current = next;
    warmNextShots();
  }

  function syncTimer() {
    if (reducedMotion.matches || document.hidden) {
      clearInterval(timer);
      timer = undefined;
    } else if (timer === undefined) {
      warmNextShots();
      timer = setInterval(cut, cutIntervalMs);
    }
  }

  reducedMotion.addEventListener('change', syncTimer);
  document.addEventListener('visibilitychange', syncTimer);
  syncTimer();
}
