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
}
