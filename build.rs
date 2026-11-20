fn main() {
    println!("cargo:rerun-if-changed=resources/ui.manifest");
    println!("cargo:rerun-if-changed=resources/prismestra.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("resources/ui.manifest");
        // Scope visual styles to the UI executable; CLI tools and libraries keep
        // their existing activation context and privilege requirements.
        println!("cargo:rustc-link-arg-bin=pc-control-ui=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=pc-control-ui=/MANIFESTINPUT:{}",
            manifest.display()
        );
        let res = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
            .join("prismestra-icon.res");
        std::fs::write(
            &res,
            icon_resources(include_bytes!("resources/prismestra.ico")),
        )
        .expect("write Prismestra icon resources");
        println!("cargo:rustc-link-arg-bin=pc-control-ui={}", res.display());
    }
}

// MSVC links .res files directly. The committed ICO supplies the original image sizes
// without requiring rc.exe to be on PATH on every Rust build host.
fn icon_resources(ico: &[u8]) -> Vec<u8> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    let read_u32 = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().unwrap()) as usize;
    assert_eq!(&ico[..4], &[0, 0, 1, 0], "expected an ICO file");
    let count = u16::from_le_bytes(ico[4..6].try_into().unwrap());
    assert!(count > 0, "expected at least one icon image");
    let mut group = ico[..6].to_vec();
    let mut res = Vec::new();
    resource(&mut res, 0, 0, &[]); // Required empty .res file header.
    for (index, entry) in ico[6..6 + usize::from(count) * 16]
        .chunks_exact(16)
        .enumerate()
    {
        let id = u16::try_from(index + 1).unwrap();
        let size = read_u32(&entry[8..12]);
        let offset = read_u32(&entry[12..16]);
        resource(&mut res, RT_ICON, id, &ico[offset..offset + size]);
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&id.to_le_bytes()); // Resources refer to an ID instead of an offset.
    }
    resource(&mut res, RT_GROUP_ICON, 1, &group);
    res
}

fn resource(res: &mut Vec<u8>, kind: u16, name: u16, data: &[u8]) {
    res.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    res.extend_from_slice(&32u32.to_le_bytes()); // Header size with numeric type and name.
    for word in [0xffff, kind, 0xffff, name] {
        res.extend_from_slice(&word.to_le_bytes());
    }
    res.extend_from_slice(&[0; 16]); // Data version, flags, neutral language, version, characteristics.
    res.extend_from_slice(data);
    res.resize(res.len().next_multiple_of(4), 0);
}
