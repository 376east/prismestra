//! Read-only update discovery for the published Windows Setup channels.
use crate::transport::winhttp_get::{self, Proxy, Request};
use serde::Deserialize;
use std::time::{Duration, Instant};

const REPO: &str = "https://github.com/376east/prismestra";
const BODY_LIMIT: usize = 512 * 1024;
const DEADLINE: Duration = Duration::from_secs(8);
const PAGE_SIZE: usize = 10;
const MAX_PAGES: usize = 4;
const REQUEST: Request<'static> = Request {
    agent: "Prismestra/release-check",
    host: "api.github.com",
    port: 443,
    path: "/repos/376east/prismestra/releases",
    secure: true,
    proxy: Proxy::Automatic,
    headers: "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n",
    max_body: BODY_LIMIT,
    io_timeout_ms: 2_000,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Version {
    core: [u16; 3],
    preview: Option<u16>,
}

fn number(text: &str) -> Option<u16> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    text.parse().ok()
}

fn core(text: &str) -> Option<[u16; 3]> {
    let mut parts = text.split('.');
    let core = [
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    ];
    parts.next().is_none().then_some(core)
}

impl Version {
    pub(crate) fn from_setup(text: &str) -> Option<Self> {
        let (base, revision) = text.rsplit_once('.')?;
        let revision = number(revision)?;
        Some(Self {
            core: core(base)?,
            preview: (revision != 0).then_some(revision),
        })
    }

    pub(crate) fn from_tag(text: &str) -> Option<Self> {
        let tag = text.strip_prefix('v')?;
        let (base, preview) = if let Some((base, n)) = tag.split_once("-preview.") {
            let n = number(n)?;
            if n == 0 {
                return None;
            }
            (base, Some(n))
        } else {
            (tag, None)
        };
        Some(Self {
            core: core(base)?,
            preview,
        })
    }

    pub(crate) fn tag(self) -> String {
        let [major, minor, patch] = self.core;
        match self.preview {
            Some(n) => format!("v{major}.{minor}.{patch}-preview.{n}"),
            None => format!("v{major}.{minor}.{patch}"),
        }
    }

    pub(crate) fn page(self) -> String {
        format!("{REPO}/releases/tag/{}", self.tag())
    }

    pub(crate) fn channel(self) -> &'static str {
        if self.preview.is_some() {
            "Preview"
        } else {
            "Stable"
        }
    }

    fn same_channel(self, other: Self) -> bool {
        self.preview.is_some() == other.preview.is_some()
    }

    pub(crate) fn is_update_for(self, current: Self) -> bool {
        (self.same_channel(current) || self.is_stable_migration_from(current))
            && self.order() > current.order()
    }

    pub(crate) fn is_stable_migration_from(self, current: Self) -> bool {
        // A stable Setup has revision zero, so it only upgrades Windows'
        // four-part installed version when the three-part version increases.
        self.preview.is_none() && current.preview.is_some() && self.core > current.core
    }

    fn setup_asset(self) -> String {
        let [major, minor, patch] = self.core;
        format!(
            "Prismestra-Setup-{major}.{minor}.{patch}.{}-x64.exe",
            self.preview.unwrap_or(0)
        )
    }

    fn order(self) -> ([u16; 3], u32) {
        (
            self.core,
            self.preview.map_or(u32::from(u16::MAX) + 1, u32::from),
        )
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResultState {
    Available(Version),
    NoNewer,
    Unavailable,
}

fn eligible_releases(releases: Vec<Release>) -> impl Iterator<Item = Version> {
    releases.into_iter().filter_map(|release| {
        let version = Version::from_tag(&release.tag_name)?;
        if release.draft
            || release.prerelease != version.preview.is_some()
            || release.html_url != version.page()
        {
            return None;
        }
        let name = version.setup_asset();
        let mut assets = release.assets.iter().filter(|asset| asset.name == name);
        let asset = assets.next()?;
        if assets.next().is_some() || asset.state != "uploaded" || asset.size == 0 {
            return None;
        }
        let digest = asset.digest.as_deref()?.strip_prefix("sha256:")?;
        (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(version)
    })
}

fn check_with(
    current: Version,
    mut fetch: impl FnMut(&Request<'_>, Instant) -> Result<Vec<u8>, winhttp_get::Error>,
) -> ResultState {
    let deadline = Instant::now() + DEADLINE;
    let mut newest: Option<Version> = None;
    let mut newest_stable = None;
    let mut exhausted = false;
    for page in 1..=MAX_PAGES {
        if Instant::now() >= deadline {
            return ResultState::Unavailable;
        }
        let path = format!("{}?per_page={PAGE_SIZE}&page={page}", REQUEST.path);
        let request = Request {
            path: &path,
            ..REQUEST
        };
        let Ok(body) = fetch(&request, deadline) else {
            return ResultState::Unavailable;
        };
        if Instant::now() >= deadline || body.len() > BODY_LIMIT {
            return ResultState::Unavailable;
        }
        let Ok(releases) = serde_json::from_slice::<Vec<Release>>(&body) else {
            return ResultState::Unavailable;
        };
        if releases.len() > PAGE_SIZE {
            return ResultState::Unavailable;
        }
        exhausted = releases.len() < PAGE_SIZE;
        for version in eligible_releases(releases) {
            let slot = if version.same_channel(current) {
                &mut newest
            } else if version.is_stable_migration_from(current) {
                &mut newest_stable
            } else {
                continue;
            };
            if slot.is_none_or(|item| version.order() > item.order()) {
                *slot = Some(version);
            }
        }
        if exhausted {
            break;
        }
    }
    if newest.is_none() && exhausted {
        newest = newest_stable;
    }
    match newest {
        Some(version) if version.is_update_for(current) => ResultState::Available(version),
        Some(_) => ResultState::NoNewer,
        None => ResultState::Unavailable,
    }
}

pub(crate) fn check(current: Version) -> ResultState {
    check_with(current, |request, deadline| {
        winhttp_get::get(request, deadline, &mut || false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn release(tag: &str) -> Value {
        let version = Version::from_tag(tag).unwrap();
        json!({
            "tag_name": tag,
            "html_url": version.page(),
            "draft": false,
            "prerelease": version.preview.is_some(),
            "assets": [{
                "name": version.setup_asset(),
                "state": "uploaded",
                "size": 1,
                "digest": format!("sha256:{}", "a".repeat(64)),
            }]
        })
    }

    fn check_fixture(current: &str, values: Vec<Value>) -> ResultState {
        check_with(
            Version::from_setup(current).unwrap(),
            |request, deadline| {
                assert_eq!(request.host, "api.github.com");
                assert!(request.secure && request.port == 443);
                assert_eq!(request.proxy, Proxy::Automatic);
                assert_eq!(
                    request.path,
                    "/repos/376east/prismestra/releases?per_page=10&page=1"
                );
                assert!(deadline > Instant::now());
                Ok(serde_json::to_vec(&values).unwrap())
            },
        )
    }

    #[test]
    fn setup_and_tag_follow_the_numeric_release_contract() {
        assert_eq!(
            Version::from_setup("0.1.0.7").unwrap().tag(),
            "v0.1.0-preview.7"
        );
        assert_eq!(Version::from_setup("1.2.3.0").unwrap().tag(), "v1.2.3");
        for invalid in [
            "v01.1.0",
            "v0.1.0-preview.07",
            "v0.1.0-preview.0",
            "v0.1.0-pre.1",
            "v0.1.0+build",
            "v0.1.0-preview.65536",
            "v0.1.0/../release",
        ] {
            assert!(Version::from_tag(invalid).is_none(), "{invalid}");
        }
        assert!(Version::from_setup("0.1.0.07").is_none());
    }

    #[test]
    fn selects_numerically_newer_windows_release_on_the_same_channel() {
        assert_eq!(
            check_fixture(
                "0.1.0.7",
                vec![
                    release("v0.1.0-preview.9"),
                    release("v0.1.0-preview.10"),
                    release("v1.0.0"),
                    release("v0.1.0-preview.6")
                ]
            ),
            ResultState::Available(Version::from_tag("v0.1.0-preview.10").unwrap())
        );
        assert_eq!(
            check_fixture(
                "0.1.0.7",
                vec![release("v0.1.0-preview.6"), release("v0.1.0-preview.7")]
            ),
            ResultState::NoNewer
        );
        assert_eq!(
            check_fixture(
                "1.0.0.0",
                vec![release("v2.0.0-preview.3"), release("v1.0.0")]
            ),
            ResultState::NoNewer
        );
    }

    #[test]
    fn finds_stable_on_the_next_page_after_ten_previews() {
        let preview_page: Vec<_> = (1..=10)
            .map(|n| release(&format!("v3.0.0-preview.{n}")))
            .collect();
        let mut calls = Vec::new();
        let result = check_with(
            Version::from_setup("1.0.0.0").unwrap(),
            |request, deadline| {
                assert_eq!(request.host, "api.github.com");
                assert!(request.secure && request.port == 443);
                assert_eq!(request.proxy, Proxy::Automatic);
                calls.push((request.path.to_owned(), deadline));
                let page = match request.path {
                    "/repos/376east/prismestra/releases?per_page=10&page=1" => preview_page.clone(),
                    "/repos/376east/prismestra/releases?per_page=10&page=2" => {
                        vec![release("v1.0.0"), release("v2.0.0")]
                    }
                    _ => panic!("unexpected page {}", request.path),
                };
                Ok(serde_json::to_vec(&page).unwrap())
            },
        );
        assert_eq!(
            result,
            ResultState::Available(Version::from_tag("v2.0.0").unwrap())
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, calls[1].1);
    }

    #[test]
    fn offers_stable_migration_only_without_eligible_previews() {
        let stable = Version::from_tag("v0.2.0").unwrap();
        let mut removed_preview = release("v0.1.0-preview.30");
        removed_preview["assets"] = json!([]);
        assert_eq!(
            check_fixture("0.1.0.30", vec![removed_preview, release("v0.2.0")]),
            ResultState::Available(stable)
        );
        assert!(stable.is_update_for(Version::from_setup("0.1.0.30").unwrap()));
        assert_eq!(
            check_fixture(
                "0.1.0.30",
                vec![release("v0.1.0-preview.7"), release("v0.2.0")]
            ),
            ResultState::NoNewer
        );
        assert_eq!(
            check_fixture("0.3.0.1", vec![release("v0.2.0")]),
            ResultState::Unavailable
        );
        assert_eq!(
            check_fixture("0.2.0.1", vec![release("v0.2.0")]),
            ResultState::Unavailable
        );
    }

    #[test]
    fn caps_the_number_of_requests_when_every_page_is_full() {
        let full_page: Vec<_> = (1..=10)
            .map(|n| release(&format!("v3.0.0-preview.{n}")))
            .collect();
        let body = serde_json::to_vec(&full_page).unwrap();
        let mut paths = Vec::new();
        let result = check_with(Version::from_setup("2.0.0.0").unwrap(), |request, _| {
            paths.push(request.path.to_owned());
            Ok(body.clone())
        });
        assert_eq!(result, ResultState::Unavailable);
        assert_eq!(
            paths,
            [1, 2, 3, 4]
                .map(|n| format!("/repos/376east/prismestra/releases?per_page=10&page={n}"))
        );
    }

    #[test]
    fn refuses_drafts_external_links_bad_assets_and_inconsistent_flags() {
        let mut external = release("v0.1.0-preview.10");
        external["html_url"] = json!("https://example.test/Setup.exe");
        let mut draft = release("v0.1.0-preview.10");
        draft["draft"] = json!(true);
        let mut unfinished = release("v0.1.0-preview.10");
        unfinished["assets"][0]["state"] = json!("new");
        let mut no_digest = release("v0.1.0-preview.10");
        no_digest["assets"][0]["digest"] = json!("sha256:bad");
        let mut mismatch = release("v0.1.0-preview.10");
        mismatch["prerelease"] = json!(false);
        assert_eq!(
            check_fixture(
                "0.1.0.7",
                vec![external, draft, unfinished, no_digest, mismatch]
            ),
            ResultState::Unavailable
        );
    }

    #[test]
    fn network_errors_deadlines_and_invalid_json_are_unavailable() {
        let current = Version::from_setup("0.1.0.7").unwrap();
        for error in [
            winhttp_get::Error::WinHttp(12007),
            winhttp_get::Error::Deadline,
            winhttp_get::Error::Http(302),
        ] {
            assert_eq!(
                check_with(current, |_, _| Err(error)),
                ResultState::Unavailable
            );
        }
        assert_eq!(
            check_with(current, |_, _| Ok(b"not JSON".to_vec())),
            ResultState::Unavailable
        );
        assert_eq!(
            check_with(current, |_, _| Ok(vec![0; BODY_LIMIT + 1])),
            ResultState::Unavailable
        );
    }
}
