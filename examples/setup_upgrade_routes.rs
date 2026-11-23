//! Hosted Setup fixture. Exercises the real profile/catalog readers and
//! resolver without starting the installed app, sensor providers, or OpenRGB.
use pc_control::app::{
    document::Document,
    sensor_bindings::{Catalog, Scope, Target},
    sensor_pci::Pci,
    sensors::{Source, Value},
};
use std::{fs, io::Write, path::Path};

const PROFILE: &str = "profile.json";
const CATALOG: &str = "state/sensor-sources.json";
const ROUTES: u32 = 5;

fn routes() -> impl Iterator<Item = (String, String, Target)> {
    (0..ROUTES).map(|i| {
        (
            format!("ci_sensor_{i}"),
            format!("physical/ci-sensor-{i}"),
            Target::Nvidia {
                pci: Pci {
                    device_id: ((0x2803 + i) << 16) | 0x10de,
                    subsystem_id: 0x12345678,
                    revision_id: 0xa1,
                    extended_device_id: 0x77,
                },
            },
        )
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| error.to_string())
}

fn seed(root: &Path) -> Result<(), String> {
    let profile = root.join(PROFILE);
    let mut doc = Document::decode(include_bytes!(
        "../scripts/installer/upgrade-profile.fixture.json"
    ))?;
    let mut catalog = Catalog::default();
    let mut bindings = Vec::new();
    for (input, id, target) in routes() {
        doc.temperatures.sources.push(Source {
            key: format!("view_{input}"),
            value: Value::Sensor {
                input: input.clone(),
            },
        });
        catalog.set_source(&id, target)?;
        bindings.push((input, id));
    }
    catalog.set_routes(
        &Scope::lighting(&profile.to_string_lossy(), &doc)?,
        &bindings,
    )?;
    let catalog_bytes = catalog.encode()?;
    write_new(&profile, &doc.encode()?)?;
    write_new(&root.join(CATALOG), &catalog_bytes)
}

fn verify(root: &Path) -> Result<serde_json::Value, String> {
    let profile = root.join(PROFILE);
    let document = Document::load(&profile)?;
    let expected_inputs = routes().map(|(input, _, _)| input).collect::<Vec<_>>();
    let actual_inputs = document
        .temperatures
        .sources
        .iter()
        .map(|source| match &source.value {
            Value::Sensor { input } => Ok(input.clone()),
            _ => Err("Expected direct sensor inputs in the Setup fixture"),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if actual_inputs != expected_inputs {
        return Err("The upgraded profile does not expose the exact five fixture inputs".into());
    }
    let scope = Scope::lighting(&profile.to_string_lossy(), &document)?;
    let catalog = Catalog::load(&root.join(CATALOG))?;
    let resolved = catalog.prepare(&scope, &actual_inputs)?;
    if resolved.len() != ROUTES as usize
        || !resolved
            .iter()
            .zip(routes())
            .all(|(actual, (input, id, target))| {
                actual.input == input && actual.source_id == id && actual.target == target
            })
    {
        return Err(
            "The upgraded profile did not resolve all five exact sensor assignments".into(),
        );
    }
    let Scope::Lighting { digest, .. } = scope;
    Ok(serde_json::json!({
        "scope_sha256": digest,
        "routes": resolved.iter().map(|route| serde_json::json!({
            "input": route.input, "source": route.source_id, "revision": route.source_revision
        })).collect::<Vec<_>>(),
    }))
}

fn run() -> Result<(), String> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let [operation, root] = args.as_slice() else {
        return Err("Expected seed or verify and a synthetic profile directory".into());
    };
    // Installed bootstrap resolves the user root before joining profile.json.
    let root = Path::new(root)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    match operation.to_str() {
        Some("seed") => seed(&root),
        Some("verify") => {
            println!("{}", verify(&root)?);
            Ok(())
        }
        _ => Err("Unknown Setup upgrade fixture operation".into()),
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
