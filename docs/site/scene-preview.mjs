const descriptions = {
  rainbow: ['Colors move across the devices.', 'Цвета бегут по устройствам.'],
  breathing: ['All the lights brighten and dim together.', 'Вся подсветка плавно разгорается и затихает.'],
  noise: ['Colors drift slowly across the setup.', 'Оттенки медленно перетекают от одного устройства к другому.'],
};

function lightMask(photo, width, height) {
  const mask = document.createElement('canvas');
  mask.width = width;
  mask.height = height;
  const context = mask.getContext('2d');
  if (!context) return null;
  context.drawImage(photo, 0, 0, width, height);
  const pixels = context.getImageData(0, 0, width, height);
  for (let i = 0; i < pixels.data.length; i += 4) {
    const chroma = pixels.data[i + 2] - pixels.data[i + 1];
    pixels.data[i + 3] = Math.max(0, chroma - 6) * 2.65;
    pixels.data[i] = pixels.data[i + 1] = pixels.data[i + 2] = 255;
  }
  context.putImageData(pixels, 0, 0);
  return mask;
}

function paint(context, mask, effect, seconds) {
  const { width, height } = mask;
  const gradient = context.createLinearGradient(0, height * .75, width, height * .35);
  for (let step = 0; step <= 8; step++) {
    const x = step / 8;
    let hue;
    let intensity;
    if (effect === 'breathing') {
      hue = 30;
      intensity = .42 + .37 * Math.cos(seconds * 1.45);
    } else if (effect === 'noise') {
      hue = 185 + 39 * Math.sin(x * 5 - seconds * .85);
      intensity = .54 + .23 * Math.cos(x * 7 - seconds * .82);
    } else {
      hue = (280 + x * 280 - seconds * 68) % 360;
      intensity = .75;
    }
    gradient.addColorStop(x, `hsla(${hue}, 95%, 58%, ${intensity})`);
  }

  context.clearRect(0, 0, width, height);
  context.fillStyle = gradient;
  context.fillRect(0, 0, width, height);
  context.globalCompositeOperation = 'destination-in';
  context.drawImage(mask, 0, 0);
  context.globalCompositeOperation = 'source-over';
}

export async function connectScenePreview(russian) {
  const scene = document.getElementById('scene');
  const photo = document.getElementById('scene-photo');
  const canvas = scene.querySelector('canvas');
  const context = canvas.getContext('2d');
  if (!context) return;

  let mask;
  try {
    await photo.decode();
    mask = lightMask(photo, canvas.width, canvas.height);
  } catch {
    return;
  }
  if (!mask) return;

  const options = scene.querySelector('.scene-options');
  const buttons = options.querySelectorAll('button[data-effect]');
  const playback = document.getElementById('scene-playback');
  const description = document.getElementById('scene-description');
  const motion = matchMedia('(prefers-reduced-motion: reduce)');
  let playing = !motion.matches;
  let visible = true;
  let frame = 0;
  let previous;
  let elapsed = 0;

  function tick(now) {
    if (previous === undefined || now - previous >= 30) {
      elapsed += previous === undefined ? 0 : (now - previous) / 1000;
      previous = now;
      paint(context, mask, scene.dataset.scene, elapsed);
    }
    frame = requestAnimationFrame(tick);
  }

  function updatePlayback() {
    playback.textContent = russian ? (playing ? 'Пауза' : 'Воспроизвести') : (playing ? 'Pause' : 'Play');
    if (playing && visible && !document.hidden) {
      if (!frame) frame = requestAnimationFrame(tick);
    } else {
      cancelAnimationFrame(frame);
      frame = 0;
      previous = undefined;
    }
  }

  for (const button of buttons) {
    button.addEventListener('click', () => {
      scene.dataset.scene = button.dataset.effect;
      for (const option of buttons) option.setAttribute('aria-pressed', String(option === button));
      description.textContent = descriptions[button.dataset.effect][russian ? 1 : 0];
      elapsed = 0;
      paint(context, mask, scene.dataset.scene, elapsed);
    });
  }
  playback.addEventListener('click', () => {
    playing = !playing;
    updatePlayback();
  });
  motion.addEventListener('change', (event) => {
    playing = !event.matches;
    updatePlayback();
  });
  document.addEventListener('visibilitychange', updatePlayback);
  new IntersectionObserver(([entry]) => {
    visible = entry.isIntersecting;
    updatePlayback();
  }).observe(canvas);

  paint(context, mask, scene.dataset.scene, elapsed);
  options.hidden = playback.hidden = false;
  updatePlayback();
}
