const releases = 'https://github.com/376east/prismestra/releases';

export function renderWindowsDownloads(html, release) {
  const version = /^v(\d+\.\d+\.\d+)$/.exec(release?.tag_name)?.[1];
  if (!version || release.draft || release.prerelease || !Array.isArray(release.assets)) {
    throw new Error('Expected a stable Prismestra release');
  }

  const filename = `Prismestra-Setup-${version}.0-x64.exe`;
  const url = `${releases}/download/${release.tag_name}/${filename}`;
  if (!release.assets.some((asset) => asset?.name === filename && asset.browser_download_url === url)) {
    throw new Error(`Missing Windows Setup asset for ${release.tag_name}`);
  }
  return html.replace(`id="download" href="${releases}/latest"`, `id="download" href="${url}"`);
}
