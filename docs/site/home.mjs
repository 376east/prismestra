import { getWindowsSetup } from './setup-download.mjs';

const params = new URLSearchParams(location.search);
const language = document.getElementById('language');
const devices = document.getElementById('devices');
const russian = params.get('lang') === 'ru';

if (['q', 'category', 'route'].some((key) => params.has(key))) {
  location.replace(new URL('devices/' + location.search + location.hash, location.href));
} else {
  connectDownload();
  if (russian) showRussian();
}

function showRussian() {
  const copy = {
    title: 'Одна сцена для подсветки ПК и комнаты',
    summary: 'Продолжите эффект с клавиатуры на подсветке за монитором и на ленте в комнате. На совместимом LCD оставьте медиа и температуры.',
    download: 'Установщик для Windows',
    devices: 'Проверить мои устройства',
    platform: 'Windows 10 или 11 (64-разрядная версия).',
    'manual-download': 'Все файлы релиза',
    bundled: 'В установщик входит OpenRGB. Он в фоне управляет совместимой подсветкой компьютера.',
    'first-scene': 'Первая сцена',
    'step-1': 'Установите Prismestra и откройте «Устройства», чтобы увидеть найденное оборудование.',
    'step-2': 'Выберите сцену. Если хотите привязать эффект к расположению устройств, расставьте их на карте.',
    'step-3': 'Закройте редактор, когда закончите. Сцена продолжит работать в трее.',
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

function connectDownload() {
  const download = document.getElementById('download');
  let pending = true;
  const setup = getWindowsSetup().then((url) => {
    if (url) download.href = url;
    pending = false;
    return download.href;
  });
  download.addEventListener('click', async (event) => {
    if (!pending || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    location.assign(await setup);
  });
}
