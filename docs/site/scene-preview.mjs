export function connectScenePreview() {
  const play = document.getElementById('scene-play');
  for (const effect of document.querySelectorAll('.scene-choice[name="effect"]')) {
    effect.addEventListener('click', () => { play.checked = true; });
  }
}
