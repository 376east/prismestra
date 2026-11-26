import { connectWindowsSetup } from './setup-download.mjs';
import { connectScenePreview } from './scene-preview.mjs?v=preview-controls';

const params = new URLSearchParams(location.search);
const language = document.getElementById('language');
const devices = document.getElementById('devices');
const russian = params.get('lang') === 'ru';

if (['q', 'category', 'route'].some((key) => params.has(key))) {
  location.replace(new URL('devices/' + location.search + location.hash, location.href));
} else {
  if (russian) showRussian();
  connectScenePreview();
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
          ? 'Загрузка не началась. Откройте «Файлы релиза» и выберите установщик Prismestra для x64.'
          : 'The download did not start. Open Release files and choose the Prismestra x64 installer.';
      }
    });
  }
}

function showRussian() {
  const copy = {
    title: 'Вся подсветка играет вместе',
    summary: 'Синхронизируйте подсветку клавиатуры, ПК и светодиодных лент. Выберите готовый эффект или создайте свой.',
    download: 'Скачать для Windows',
    devices: 'Устройства',
    platform: 'Windows 10 (1903+) или 11, x64',
    'manual-download': 'Файлы релиза',
    'preview-label': 'Предпросмотр сцены',
    'effect-rainbow': 'Радуга',
    'effect-breathing': 'Дыхание',
    'effect-noise': 'Мягкий свет',
    'preview-play': 'Смотреть анимацию',
    'preview-pause': 'Пауза',
    'description-rainbow': 'Цвета бегут по устройствам.',
    'description-breathing': 'Вся подсветка плавно разгорается и затихает.',
    'description-noise': 'Оттенки медленно перетекают от одного устройства к другому.',
    'compatibility-title': 'Подойдёт ли моё устройство?',
    'compatibility-summary': 'Найдите свою модель и узнайте, чем Prismestra может управлять и как её подключить.',
    'browse-pc': 'Комплектующие ПК',
    'browse-strips': 'Светодиодные ленты',
    'browse-displays': 'LCD и мониторы',
    'browse-home': 'Умный свет',
    'model-label': 'Модель устройства',
    'model-search': 'Найти устройство',
    bundled: 'В установщик входит OpenRGB для совместимой подсветки ПК.',
    'first-scene': 'Первая сцена',
    'step-title-1': 'Установите Prismestra',
    'step-title-2': 'Проверьте подсветку',
    'step-title-3': 'Выберите сцену',
    'step-1': 'Запустите установщик и откройте Prismestra. Сцена запустится на устройствах, которые настроились автоматически.',
    'step-2': 'Откройте «Устройства», чтобы увидеть, что подключилось и что ещё нужно настроить.',
    'step-3': 'Выберите эффект в разделе «Сцены» и нажмите «Применить». Подсветка продолжит работать после закрытия редактора.',
    guide: 'Помощь с установкой',
    contribute: 'Как помочь проекту',
  };
  document.documentElement.lang = 'ru';
  document.title = 'Prismestra · Подсветка ПК и комнаты';
  for (const [id, text] of Object.entries(copy)) document.getElementById(id).textContent = text;
  document.getElementById('scene-photo').alt = 'ПК, монитор и клавиатура с общей сценой подсветки';
  document.getElementById('model').placeholder = 'Например, Dark Project KD98A';
  document.getElementById('search-language').value = 'ru';
  document.getElementById('browse-types').setAttribute('aria-label', 'Каталог по типу устройства');
  for (const link of document.querySelectorAll('.browse-types a')) {
    const url = new URL(link.href);
    url.searchParams.set('lang', 'ru');
    link.href = url;
  }
  document.getElementById('home').href = '?lang=ru';
  devices.href = 'devices/?lang=ru';
  language.href = '?lang=en';
  language.lang = 'en';
  language.hreflang = 'en';
  language.textContent = 'English';
}
