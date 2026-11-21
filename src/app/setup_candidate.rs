//! Verification only. The caller supplies metadata from the official release
//! and a locally configured publisher certificate pin.
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::Path,
    slice,
};
use windows_sys::Win32::{
    Foundation::TRUST_E_NOSIGNATURE,
    Security::WinTrust::*,
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    },
};

const MAX_SETUP_SIZE: u64 = 1024 * 1024 * 1024;
type Hash = [u8; 32];

#[derive(Debug)]
pub enum Rejection {
    Io(io::Error),
    InvalidSize,
    NotRegularFile,
    HashMismatch,
    Unsigned,
    UntrustedSignature(i32),
    SignerUnavailable,
    PublisherUnconfigured,
    PublisherMismatch,
}

/// Returns the verified file handle at offset zero, held without write or
/// delete sharing. A successful verification does not authorize execution.
pub fn verify_download(
    path: &Path,
    size: u64,
    sha256: Hash,
    publisher_pin: Option<Hash>,
) -> Result<File, Rejection> {
    verify_with(path, size, sha256, publisher_pin, authenticode_signer)
}

fn verify_with(
    path: &Path,
    size: u64,
    sha256: Hash,
    publisher_pin: Option<Hash>,
    signer: impl FnOnce(&Path, &File) -> Result<Hash, Rejection>,
) -> Result<File, Rejection> {
    if size == 0 || size > MAX_SETUP_SIZE {
        return Err(Rejection::InvalidSize);
    }
    let path = std::path::absolute(path).map_err(Rejection::Io)?;
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(Rejection::Io)?;
    let metadata = file.metadata().map_err(Rejection::Io)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(Rejection::NotRegularFile);
    }
    if metadata.len() != size {
        return Err(Rejection::InvalidSize);
    }
    let (observed, digest) = hash((&mut file).take(size + 1)).map_err(Rejection::Io)?;
    if observed != size {
        return Err(Rejection::InvalidSize);
    }
    if digest != sha256 {
        return Err(Rejection::HashMismatch);
    }
    file.seek(SeekFrom::Start(0)).map_err(Rejection::Io)?;
    let signer = signer(&path, &file)?;
    if signer != publisher_pin.ok_or(Rejection::PublisherUnconfigured)? {
        return Err(Rejection::PublisherMismatch);
    }
    file.seek(SeekFrom::Start(0)).map_err(Rejection::Io)?;
    Ok(file)
}

fn hash(mut reader: impl Read) -> io::Result<(u64, Hash)> {
    let mut digest = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok((size, digest.finalize().into()));
        }
        digest.update(&buffer[..count]);
        size += count as u64;
    }
}

fn authenticode_signer(path: &Path, file: &File) -> Result<Hash, Rejection> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path.as_ptr(),
        hFile: file.as_raw_handle(),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_MOTW
            | WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT
            | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    };
    let mut policy = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut policy,
            std::ptr::from_mut(&mut data).cast(),
        )
    };
    // The signer certificate is borrowed from the verification state. Hash it
    // before closing the state on both successful and unsuccessful checks.
    let result = match status {
        0 => signer_certificate(&data),
        TRUST_E_NOSIGNATURE => Err(Rejection::Unsigned),
        status => Err(Rejection::UntrustedSignature(status)),
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &mut policy,
            std::ptr::from_mut(&mut data).cast(),
        )
    };
    result
}

fn signer_certificate(data: &WINTRUST_DATA) -> Result<Hash, Rejection> {
    if data.hWVTStateData.is_null() {
        return Err(Rejection::SignerUnavailable);
    }
    unsafe {
        let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
        if provider.is_null() {
            return Err(Rejection::SignerUnavailable);
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
        if signer.is_null() {
            return Err(Rejection::SignerUnavailable);
        }
        let certificate = WTHelperGetProvCertFromChain(signer, 0)
            .as_ref()
            .and_then(|entry| entry.pCert.as_ref())
            .ok_or(Rejection::SignerUnavailable)?;
        if certificate.pbCertEncoded.is_null() || certificate.cbCertEncoded == 0 {
            return Err(Rejection::SignerUnavailable);
        }
        let der = slice::from_raw_parts(
            certificate.pbCertEncoded,
            certificate.cbCertEncoded as usize,
        );
        Ok(Sha256::digest(der).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_exact_size_hash_and_locally_pinned_publisher() {
        let dir = std::env::temp_dir().join(format!(
            "prismestra-setup-candidate-{}-{}",
            std::process::id(),
            crate::clock::monotonic_us()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("setup.exe");
        let bytes = b"synthetic Setup contents";
        std::fs::write(&path, bytes).unwrap();
        let digest = Sha256::digest(bytes).into();
        let size = bytes.len() as u64;
        let pin = [7; 32];
        let trusted = |_: &Path, _: &File| Ok(pin);
        assert!(matches!(
            verify_with(&path, size + 1, digest, Some(pin), |_, _| unreachable!()),
            Err(Rejection::InvalidSize)
        ));
        assert!(matches!(
            verify_with(&path, size, [0; 32], Some(pin), |_, _| unreachable!()),
            Err(Rejection::HashMismatch)
        ));
        assert!(matches!(
            verify_with(&path, size, digest, None, trusted),
            Err(Rejection::PublisherUnconfigured)
        ));
        assert!(matches!(
            verify_with(&path, size, digest, Some([9; 32]), trusted),
            Err(Rejection::PublisherMismatch)
        ));
        let mut verified = verify_with(&path, size, digest, Some(pin), trusted).unwrap();
        let mut read = Vec::new();
        verified.read_to_end(&mut read).unwrap();
        assert_eq!(read, bytes);
        drop(verified);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn windows_rejects_an_unsigned_pe_even_with_exact_release_digest() {
        // Cargo's PE test harness is built unsigned in local tests and CI.
        let path = std::env::current_exe().unwrap();
        let (size, digest) = hash(File::open(&path).unwrap()).unwrap();
        assert!(matches!(
            verify_download(&path, size, digest, Some([7; 32])),
            Err(Rejection::Unsigned)
        ));
    }
}
