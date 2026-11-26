import { connectWindowsSetup } from './setup-download.mjs';

const params = new URLSearchParams(location.search);
const language = document.getElementById('language');
const devices = document.getElementById('devices');
const russian = params.get('lang') === 'ru';

if (['q', 'category', 'route'].some((key) => params.has(key))) {
  location.replace(new URL('devices/' + location.search + location.hash, location.href));
} else {
  if (russian) showRussian();
  const setup = connectWindowsSetup(document.getElementById('download'));
  if (params.get('download') === '1') {
    params.delete('download');
    const query = params.toString();
    history.replaceState(null, '', location.pathname + (query ? '?' + query : '') + location.hash);
    setup.then((url) => {
      if (url) {
        location.assign(url);
      } else {
        document.getElementById('download-status').textContent = russian
          ? 'Загрузка не началась. Откройте «Все файлы релиза» и выберите Prismestra-Setup-…-x64.exe.'
          : 'The download did not start. Open All release files and choose Prismestra-Setup-…-x64.exe.';
      }
    });
  }
}

function showRussian() {
  const copy = {
    title: 'Одна сцена для подсветки ПК и комнаты',
    summary: 'Запустите один эффект на клавиатуре, светодиодных лентах и подсветке за монитором. На совместимом LCD можно показать картинку, анимацию или температуры.',
    download: 'Установщик для Windows',
    devices: 'Проверить мои устройства',
    platform: 'Windows 10 версии 1903 или новее либо Windows 11 (x64).',
    'manual-download': 'Все файлы релиза',
    bundled: 'OpenRGB уже входит в Prismestra. Достаточно одного установщика.',
    'first-scene': 'Первая сцена',
    'step-1': 'Запустите установщик и откройте Prismestra. Сцена запустится на устройствах, которые настроились автоматически.',
    'step-2': 'Если какая-то подсветка не загорелась, откройте «Устройства». Там видно, что подключилось и что ещё нужно настроить.',
    'step-3': 'Чтобы сменить эффект, откройте «Сцены», выберите сцену и нажмите «Применить». Подсветка продолжит работать после закрытия редактора.',
    guide: 'Помощь с установкой',
    contribute: 'Как помочь проекту',
  };
  document.documentElement.lang = 'ru';
  document.title = 'Prismestra · Подсветка ПК и комнаты';
  for (const [id, text] of Object.entries(copy)) document.getElementById(id).textContent = text;
  document.getElementById('home').href = '?lang=ru';
  devices.href = 'devices/?lang=ru';
  language.href = '?lang=en';
  language.lang = 'en';
  language.hreflang = 'en';
  language.textContent = 'English';
}
