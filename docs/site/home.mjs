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
    setup.then((url) => { if (url) location.assign(url); });
  }
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
    'step-1': 'Запустите скачанный установщик и откройте Prismestra. Она сама ищет устройства и запускает стартовую сцену на совместимой подсветке, которую может настроить автоматически.',
    'step-2': 'Клавиатура загорелась, а лента ещё нет? Откройте «Устройства». Там видно, что подключилось, что ещё ищется и что нужно настроить. После подключения нажмите «Найти устройства», когда поиск доступен.',
    'step-3': 'Чтобы сменить эффект, откройте «Сцены», выберите сцену и нажмите «Применить». После закрытия редактора подсветка продолжит работать.',
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
