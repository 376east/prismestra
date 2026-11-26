const descriptions = {
  rainbow: ['Colors move across the devices.', 'Цвета бегут по устройствам.'],
  breathing: ['All the lights brighten and dim together.', 'Вся подсветка плавно разгорается и затихает.'],
  noise: ['Colors drift slowly across the setup.', 'Оттенки медленно перетекают от одного устройства к другому.'],
};

export function connectScenePreview(russian) {
  const scene = document.getElementById('scene');
  const options = scene.querySelector('.scene-options');
  const buttons = options.querySelectorAll('button[data-effect]');
  const playback = document.getElementById('scene-playback');
  const description = document.getElementById('scene-description');
  const motion = matchMedia('(prefers-reduced-motion: reduce)');
  const playLabel = russian ? 'Смотреть анимацию' : 'Watch animation';
  const pauseLabel = russian ? 'Пауза' : 'Pause';

  function setPlaying(playing) {
    scene.dataset.playing = String(playing);
    playback.textContent = playing ? pauseLabel : playLabel;
  }

  for (const button of buttons) {
    button.addEventListener('click', () => {
      const effect = button.dataset.effect;
      scene.dataset.scene = effect;
      for (const option of buttons) option.setAttribute('aria-pressed', String(option === button));
      description.textContent = descriptions[effect][russian ? 1 : 0];
      setPlaying(true);
    });
  }
  playback.addEventListener('click', () => setPlaying(scene.dataset.playing !== 'true'));
  motion.addEventListener('change', ({ matches }) => {
    if (matches) setPlaying(false);
  });

  setPlaying(!motion.matches);
  options.hidden = playback.hidden = false;
}
