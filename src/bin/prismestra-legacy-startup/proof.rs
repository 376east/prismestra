use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf, Prefix},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, GetDriveTypeW,
        MOVEFILE_WRITE_THROUGH, MoveFileExW,
    },
    System::{Com::CoCreateGuid, WindowsProgramming::DRIVE_FIXED},
};

const LAUNCHER: &str = "Start Prismestra.ps1";
pub(super) const LAUNCHER_PIN: &str =
    "82def8e5478fa616d691ebda66d0277c8950cc1d092b0797f8e99218ce670318";
const HINT: &str = "legacy-source.json";
const MAX_HINT: usize = 16_384;
const MAX_MANIFEST: usize = 131_072;
const MAX_PROFILE: usize = 4_194_304;
const MAX_IMAGE: usize = 536_870_912;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceHint {
    pub version: u32,
    pub launcher: PathBuf,
    pub executable: PathBuf,
    pub profile: PathBuf,
    pub state: PathBuf,
}

impl SourceHint {
    pub(super) fn root(&self) -> &Path {
        self.launcher
            .parent()
            .expect("verified launcher has a parent")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pointer {
    executable: String,
    commit: String,
    sha256: String,
}

#[derive(Deserialize)]
struct Manifest {
    product: String,
    commit: String,
    executable_sha256: String,
}

#[derive(Deserialize)]
struct Profile {
    version: u32,
    setup: Option<serde_json::Value>,
    lighting: Option<Lighting>,
}

#[derive(Deserialize)]
struct Lighting {
    setup: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct LocalBindings {
    version: u32,
}

#[derive(Deserialize)]
struct Receipt {
    version: u32,
    complete: bool,
    source: ReceiptSource,
    after: ReceiptHashes,
}

#[derive(Deserialize)]
struct ReceiptSource {
    profile: PathBuf,
    state: PathBuf,
}

#[derive(Deserialize)]
struct ReceiptHashes {
    #[serde(rename = "profile.json")]
    profile: String,
    #[serde(rename = "state/lighting-local.json")]
    local: String,
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    left.as_os_str().eq_ignore_ascii_case(right.as_os_str())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn disk_path(path: &Path) -> io::Result<PathBuf> {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return Err(invalid("expected a local disk path"));
    };
    let (Prefix::Disk(disk) | Prefix::VerbatimDisk(disk)) = prefix.kind() else {
        return Err(invalid("expected a local disk path"));
    };
    if parts.next() != Some(Component::RootDir) {
        return Err(invalid("expected an absolute local disk path"));
    }
    let mut result = PathBuf::from(format!("{}:\\", char::from(disk)));
    if unsafe { GetDriveTypeW(wide(&result).as_ptr()) } != DRIVE_FIXED {
        return Err(invalid("expected a fixed local disk"));
    }
    for part in parts {
        let Component::Normal(part) = part else {
            return Err(invalid("unexpected local path component"));
        };
        let mut units = part.encode_wide();
        if units.any(|unit| matches!(unit, 0 | 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
            || matches!(part.encode_wide().last(), Some(32 | 46))
        {
            return Err(invalid("invalid local path component"));
        }
        let base = part.to_string_lossy();
        let base = base
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_uppercase();
        if matches!(
            base.as_str(),
            "CON" | "NUL" | "AUX" | "PRN" | "CONIN$" | "CONOUT$"
        ) || ["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        }) {
            return Err(invalid("device names are not local files"));
        }
        result.push(part);
    }
    Ok(result)
}

pub(super) fn local_path(path: &Path, directory: bool) -> io::Result<PathBuf> {
    let path = disk_path(path)?;
    for entry in path.ancestors() {
        let metadata = fs::symlink_metadata(entry)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || if entry == path && !directory {
                !metadata.is_file()
            } else {
                !metadata.is_dir()
            }
        {
            return Err(invalid("expected a reparse-free local path"));
        }
    }
    disk_path(&fs::canonicalize(&path)?)
}

fn bounded_file(path: &Path, maximum: usize) -> io::Result<File> {
    let path = local_path(path, false)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() > maximum as u64
    {
        return Err(invalid("expected a bounded local file"));
    }
    Ok(file)
}

fn read_json<T: DeserializeOwned>(path: &Path, maximum: usize) -> io::Result<T> {
    let mut contents = Vec::new();
    bounded_file(path, maximum)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut contents)?;
    if contents.len() > maximum {
        return Err(invalid("legacy metadata exceeds size limit"));
    }
    let bytes = contents
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(&contents);
    serde_json::from_slice(bytes).map_err(io::Error::other)
}

fn sha(path: &Path, maximum: usize) -> io::Result<String> {
    let mut file = bounded_file(path, maximum)?.take(maximum as u64 + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if file.limit() == 0 {
        return Err(invalid("legacy file exceeds size limit"));
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn source(path: &Path, pin: &str) -> io::Result<SourceHint> {
    let launcher = local_path(path, false)?;
    if !launcher
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(LAUNCHER))
        || sha(&launcher, MAX_MANIFEST)? != pin
    {
        return Err(invalid("legacy launcher does not match the published pin"));
    }
    let root = launcher
        .parent()
        .ok_or_else(|| invalid("legacy launcher has no parent"))?;
    let pointer: Pointer = read_json(&root.join("live-release.json"), 4096)?;
    let parts: Vec<_> = pointer.executable.split('/').collect();
    let ["versions", prefix, "Prismestra", "Prismestra.exe"] = parts.as_slice() else {
        return Err(invalid("invalid legacy executable pointer"));
    };
    if !(7..=40).contains(&prefix.len())
        || !hex(&pointer.commit, 40)
        || !pointer.commit.starts_with(prefix)
        || !hex(&pointer.sha256, 64)
    {
        return Err(invalid("invalid legacy executable pin"));
    }
    let executable = local_path(&root.join(&pointer.executable), false)?;
    let release = executable
        .parent()
        .ok_or_else(|| invalid("legacy image has no parent"))?;
    let manifest: Manifest = read_json(&release.join("release-manifest.json"), MAX_MANIFEST)?;
    if manifest.product != "Prismestra"
        || manifest.commit != pointer.commit
        || manifest.executable_sha256 != pointer.sha256
        || sha(&executable, MAX_IMAGE)? != pointer.sha256
    {
        return Err(invalid("legacy executable does not match its release"));
    }
    let profile = local_path(&root.join("profile.json"), false)?;
    let state = local_path(&root.join("state"), true)?;
    let doc: Profile = read_json(&profile, MAX_PROFILE)?;
    let local: LocalBindings = read_json(&state.join("lighting-local.json"), MAX_PROFILE)?;
    if doc.version != 1
        || local.version != 1
        || (doc.setup.is_none() && doc.lighting.and_then(|lighting| lighting.setup).is_none())
    {
        return Err(invalid("legacy profile does not match Prismestra"));
    }
    Ok(SourceHint {
        version: 1,
        launcher,
        executable,
        profile,
        state,
    })
}

pub(super) fn import_confirmed(root: &Path, hint: &SourceHint) -> io::Result<bool> {
    let root = local_path(root, true)?;
    if hint.root().starts_with(&root) {
        return Ok(false);
    }
    let receipt: Receipt = read_json(&root.join("installed-migration.json"), MAX_MANIFEST)?;
    Ok(receipt.version == 1
        && receipt.complete
        && local_path(&receipt.source.profile, false)? == hint.profile
        && local_path(&receipt.source.state, true)? == hint.state
        && hex(&receipt.after.profile, 64)
        && hex(&receipt.after.local, 64)
        && sha(&root.join("profile.json"), MAX_PROFILE)? == receipt.after.profile
        && sha(&root.join("state").join("lighting-local.json"), MAX_PROFILE)?
            == receipt.after.local)
}

pub(super) fn write_hint(local_app_data: &Path, hint: &SourceHint) -> io::Result<()> {
    let confirmed = source(&hint.launcher, LAUNCHER_PIN)?;
    if &confirmed != hint {
        return Err(invalid("legacy source changed during preparation"));
    }
    save_hint(local_app_data, hint)
}

fn save_hint(local_app_data: &Path, hint: &SourceHint) -> io::Result<()> {
    let root = local_path(local_app_data, true)?.join("Prismestra");
    if let Err(failure) = fs::create_dir(&root)
        && failure.kind() != io::ErrorKind::AlreadyExists
    {
        return Err(failure);
    }
    let root = local_path(&root, true)?;
    if hint.root().starts_with(&root) {
        return Err(invalid(
            "legacy source is inside the installed profile root",
        ));
    }
    let target = root.join(HINT);
    match read_json::<SourceHint>(&target, MAX_HINT) {
        Ok(previous) if &previous == hint => return Ok(()),
        Ok(_) => return Err(invalid("an existing hint belongs to another source")),
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
        Err(failure) => return Err(failure),
    }
    let bytes = serde_json::to_vec(hint).map_err(io::Error::other)?;
    if bytes.len() > MAX_HINT {
        return Err(invalid("legacy hint exceeds its size limit"));
    }
    let stage = root.join(format!("legacy-source-{}.tmp", random_id()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&stage)?;
    let result = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    let published = result.and_then(|()| {
        if unsafe {
            MoveFileExW(
                wide(&stage).as_ptr(),
                wide(&target).as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    });
    if published.is_err() {
        let _ = fs::remove_file(stage);
    }
    published
}

pub(super) fn random_id() -> io::Result<String> {
    let mut guid = windows_sys::core::GUID::default();
    let result = unsafe { CoCreateGuid(&mut guid) };
    if result < 0 {
        return Err(io::Error::other(format!(
            "CoCreateGuid failed: {result:#x}"
        )));
    }
    let bytes = u64::from_be_bytes(guid.data4);
    Ok(format!(
        "{:08x}{:04x}{:04x}{bytes:016x}",
        guid.data1, guid.data2, guid.data3
    ))
}

/// Read a Windows API's NUL-terminated UTF-16 allocation before freeing it.
pub(super) unsafe fn allocated_path(value: *const u16) -> PathBuf {
    let mut length = 0;
    while unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    OsString::from_wide(unsafe { std::slice::from_raw_parts(value, length) }).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Directory(PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn put(path: &Path, value: serde_json::Value) {
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn launcher_and_migration_proofs_check_bytes_and_keep_existing_hints() {
        let base = Directory(
            std::env::temp_dir().join(format!("prismestra-legacy-{}", random_id().unwrap())),
        );
        let legacy = base.0.join("Legacy source");
        let release = legacy.join("versions/aaaaaaa/Prismestra");
        let installed = base.0.join("Local").join("Prismestra");
        fs::create_dir_all(&release).unwrap();
        fs::create_dir_all(legacy.join("state")).unwrap();
        fs::create_dir_all(installed.join("state")).unwrap();
        let launcher = legacy.join(LAUNCHER);
        fs::write(&launcher, "proof fixture, never executed").unwrap();
        let pin = sha(&launcher, MAX_MANIFEST).unwrap();
        let executable = release.join("Prismestra.exe");
        fs::write(&executable, "image fixture, never executed").unwrap();
        let image_hash = sha(&executable, MAX_IMAGE).unwrap();
        let commit = "a".repeat(40);
        put(
            &legacy.join("live-release.json"),
            json!({
                "executable": "versions/aaaaaaa/Prismestra/Prismestra.exe",
                "commit": commit, "sha256": image_hash,
            }),
        );
        put(
            &release.join("release-manifest.json"),
            json!({
                "product": "Prismestra", "commit": commit, "executable_sha256": image_hash,
            }),
        );
        put(
            &legacy.join("profile.json"),
            json!({"version": 1, "lighting": {"setup": {}}}),
        );
        put(
            &legacy.join("state/lighting-local.json"),
            json!({"version": 1}),
        );
        assert!(source(&launcher, LAUNCHER_PIN).is_err());
        let hint = source(&launcher, &pin).unwrap();
        assert_eq!(hint.launcher, local_path(&launcher, false).unwrap());
        save_hint(&base.0.join("Local"), &hint).unwrap();
        save_hint(&base.0.join("Local"), &hint).unwrap();
        let mut changed = hint.clone();
        changed.version = 2;
        assert!(save_hint(&base.0.join("Local"), &changed).is_err());
        assert_eq!(
            read_json::<SourceHint>(&installed.join(HINT), MAX_HINT).unwrap(),
            hint
        );

        let profile = installed.join("profile.json");
        let local = installed.join("state/lighting-local.json");
        fs::copy(&hint.profile, &profile).unwrap();
        fs::copy(hint.state.join("lighting-local.json"), &local).unwrap();
        let receipt_path = installed.join("installed-migration.json");
        assert!(import_confirmed(&installed, &hint).is_err());
        let mut receipt = json!({
            "version": 1, "complete": true,
            "source": {"profile": hint.profile, "state": hint.state},
            "after": {
                "profile.json": sha(&profile, MAX_PROFILE).unwrap(),
                "state/lighting-local.json": sha(&local, MAX_PROFILE).unwrap(),
            },
        });
        receipt["complete"] = json!("true");
        put(&receipt_path, receipt.clone());
        assert!(import_confirmed(&installed, &hint).is_err());
        receipt["complete"] = json!(true);
        put(&receipt_path, receipt);
        assert!(import_confirmed(&installed, &hint).unwrap());
        fs::write(&local, "changed").unwrap();
        assert!(!import_confirmed(&installed, &hint).unwrap());
        fs::write(&executable, "changed").unwrap();
        assert!(source(&launcher, &pin).is_err());
    }

    #[test]
    fn reject_network_relative_stream_and_device_paths() {
        for value in [
            r"\\server\share\a",
            r"C:a",
            r"C:\a\b:stream",
            r"C:\a\..\b",
            r"C:\a\NUL.txt",
            r"C:\a\COM¹.log",
            r"C:\a\trailing.",
        ] {
            assert!(disk_path(Path::new(value)).is_err(), "{value}");
        }
    }
}
