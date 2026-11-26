const repository = '376east/prismestra';

export async function getWindowsSetup(fetchRelease = fetch) {
  try {
    const response = await fetchRelease(`https://api.github.com/repos/${repository}/releases/latest`, {
      signal: AbortSignal.timeout(2500),
    });
    if (!response.ok) return null;
    const release = await response.json();
    const version = /^v(\d+\.\d+\.\d+)$/.exec(release.tag_name)?.[1];
    if (!version || release.draft || release.prerelease || !Array.isArray(release.assets)) return null;

    const filename = `Prismestra-Setup-${version}.0-x64.exe`;
    const url = `https://github.com/${repository}/releases/download/${release.tag_name}/${filename}`;
    return release.assets.some((asset) => asset?.name === filename && asset.browser_download_url === url)
      ? url : null;
  } catch {
    return null;
  }
}
