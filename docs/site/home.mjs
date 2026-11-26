const params = new URLSearchParams(location.search);
const language = document.getElementById('language');
const devices = document.getElementById('devices');
const russian = params.get('lang') === 'ru';

if (['q', 'category', 'route'].some((key) => params.has(key))) {
  location.replace(new URL('devices/' + location.search + location.hash, location.href));
} else if (russian) {
  const copy = {
    title: 'Одна сцена для подсветки ПК и комнаты',
    summary: 'Продолжите эффект с клавиатуры на подсветке за монитором и на ленте в комнате. На совместимом LCD оставьте медиа и температуры.',
    download: 'Установщик для Windows',
    devices: 'Проверить мои устройства',
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
  const filename = document.querySelector('#get-file code');
  document.getElementById('get-file').replaceChildren('На странице релиза выберите ', filename, ' для Windows 10 или 11.');
  document.getElementById('home').href = '?lang=ru';
  devices.href = 'devices/?lang=ru';
  language.href = '?lang=en';
  language.lang = 'en';
  language.hreflang = 'en';
  language.textContent = 'English';
}
