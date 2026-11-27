const columns = 4;
const rows = 3;
const frameCount = columns * rows;
const flipMs = 560;

function shuffleFrames(current) {
  const frames = Array.from({ length: frameCount }, (_, index) => index);
  for (let index = frames.length - 1; index > 0; index--) {
    const other = Math.floor(Math.random() * (index + 1));
    [frames[index], frames[other]] = [frames[other], frames[index]];
  }
  if (frames.at(-1) === current) {
    const other = Math.floor(Math.random() * (frames.length - 1));
    [frames[other], frames[frames.length - 1]] = [current, frames[other]];
  }
  return frames;
}

async function loadAtlas(file) {
  const image = new Image();
  image.fetchPriority = 'low';
  image.crossOrigin = 'anonymous';
  image.src = new URL(`./shots/flipbook/${file}.webp?v=hd`, import.meta.url);
  await image.decode();
  return image;
}

export function connectScenePreview() {
  const scene = document.getElementById('scene');
  const frame = scene.querySelector('.scene-frame');
  const shot = scene.querySelector('.scene-shot');
  const motionButton = document.getElementById('scene-motion');
  const pauseLabel = motionButton.textContent;
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
  let frames = [];
  let current = 0;
  let timer;
  let onScreen = false;
  let loading = false;
  let atlas;
  let mounted = false;

  async function prepareMontage() {
    if (loading) return;
    loading = true;
    try {
      atlas = await Promise.all([loadAtlas('rooms'), loadAtlas('lights')]);
    } catch {
      // The poster and its lighting controls stay usable if the atlases fail.
      loading = false;
      return;
    }
    syncMotion();
  }

  function showNext() {
    if (!frames.length) frames = shuffleFrames(current);
    current = frames.pop();
    const column = current % columns;
    const row = Math.floor(current / columns);
    shot.style.setProperty('--photo-x', `${-column / columns * 100}%`);
    shot.style.setProperty('--photo-y', `${-row / rows * 100}%`);
    shot.style.setProperty('--mask-position', `${column / (columns - 1) * 100}% ${row / (rows - 1) * 100}%`);
    shot.dataset.shot = String(current + 1);

    if (!mounted) {
      const [photo, mask] = atlas;
      photo.className = 'scene-photo';
      photo.alt = '';
      shot.querySelector('.scene-photo').replaceWith(photo);
      shot.style.setProperty('--light-mask', `url("${mask.src}")`);
      shot.classList.add('scene-atlas');
      mounted = true;
    }
    // The light layer stays in place, keeping its animation phase across cuts.
    timer = setTimeout(showNext, flipMs);
  }

  function syncMotion() {
    motionButton.hidden = reducedMotion.matches;
    const paused = scene.hasAttribute('data-paused');
    const stopped = paused || !onScreen || document.hidden || reducedMotion.matches;
    scene.toggleAttribute('data-motion-stopped', stopped);
    motionButton.textContent = paused ? motionButton.dataset.resume : pauseLabel;
    if (stopped) {
      clearTimeout(timer);
      timer = undefined;
    } else if (!atlas) {
      void prepareMontage();
    } else if (timer === undefined) {
      timer = setTimeout(showNext, flipMs);
    }
  }

  motionButton.addEventListener('click', () => {
    scene.toggleAttribute('data-paused');
    syncMotion();
  });
  const observer = new IntersectionObserver(([entry]) => {
    onScreen = entry.isIntersecting;
    syncMotion();
  });
  observer.observe(frame);
  reducedMotion.addEventListener('change', syncMotion);
  document.addEventListener('visibilitychange', syncMotion);
  syncMotion();
}
