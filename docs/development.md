# Repository and development workflow

This is the single current Prismestra source repository. It must be a self-contained Git checkout, not depend on the object database of a retired experiment. Use Git history for previous source versions; do not create dated copies of the product tree.

## Where things belong

| Path | Purpose |
|---|---|
| `src/` | Rust engine, adapters, device setup and Windows editors; web editor assets live in `src/ui/windows/web/` |
| `resources/` | Runtime assets and fonts used by the application |
| `components/openrgb/` | Pinned upstream/dependency revisions, local patch, source/license distribution inventory and notices |
| `vendor/openrgb2/` | The explicitly vendored Rust SDK dependency, tracked with its notices |
| `scripts/` | Maintained packaging/setup/verification tools, with tests beside them |
| `tests/`, `examples/` | Integration tests and named engineering stands; examples are not the ordinary launch path |
| `docs/` | Product contracts, compatibility, current status and sanitized dated evidence |
| `.github/workflows/` | CI checks on the actual source revision |
| `target/`, `work/` | Ignored build cache and generated test fixtures; safe to regenerate, not product source |
| `.local/` | Ignored workstation pointers/notes, never credentials or profiles to publish |

Installed releases, profiles, private captures, rollback backups and raw acceptance results are local artifacts. They are not a second source checkout and must not be copied wholesale into Git. For the maintained product's actual state, read [status](status.md), not a filename containing `final` in an experimental directory.

## Check and build

Pull requests and pushes to `master` run formatting and embedded editor checks. Start full validation only at the product owner’s explicit request. In GitHub Actions, dispatch CI with `software` for the full software suite or `setup` to also build and test the installer. Run the following commands when checking a full build locally on Windows with the MSVC Rust toolchain:

```powershell
cargo fmt --all -- --check
cargo test --locked -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked --bin pc-control-ui
```

The final binary is `target/release/pc-control-ui.exe`. The user-facing product name is Prismestra. CLI preview and activation remain separate; see [portable launch](portable-launch.md). Tests that inspect Windows process/listener tables need ordinary native Windows access. A sandbox denial is recorded separately from a passed native rerun.

## Package a specific revision

Use a clean committed checkout and an output directory **outside the repository**:

```powershell
python scripts/package_release.py --expected-commit COMMIT_SHA --output C:/PrismestraArtifacts/release
```

This includes the source/dependency/license material and portable launchers. Never package a workstation profile or credentials. Check the resulting archive/manifests and exact packaged EXE; a development build is not release validation.

## Build and package the PC component

Use the pinned source/dependency locks and patch in `components/openrgb`. The build and static/runtime contracts are in [component verification](openrgb-component-verification.md). The evaluated C++ code is reused upstream source with an isolated patch, not another hand-maintained source tree hidden elsewhere.

Maintained entrypoints are `scripts/components/build_openrgb_dependencies.ps1`, `build_openrgb.ps1`, `verify_openrgb_component.py`, `test_actual_openrgb.py` and `test_parent_pipe.py`. Execution gates are opt-in and distinct from static packaging. A component package must carry matching source archives, notices, pins, patch and recipes; use the distribution inventory in `components/openrgb/distribution-assets.json` and its `licenses/` files. Source archives are external build inputs, not repeated multi-megabyte Git attachments. Do not install a kernel driver as a side effect of building or verifying a package.

Package the already verified component with one maintained command:

```powershell
python scripts/components/package_openrgb_component.py `
  --runtime-dir C:/PrismestraArtifacts/component/runtime `
  --inventory C:/PrismestraArtifacts/component/static-inventory.json `
  --source-lock components/openrgb/source-lock.json `
  --dependency-lock components/openrgb/dependency-lock.json `
  --patch components/openrgb/prismestra-pc.patch `
  --source-archives-dir C:/PrismestraArtifacts/component/source `
  --licenses-dir components/openrgb/licenses `
  --recipes-dir scripts/components `
  --asset-manifest components/openrgb/distribution-assets.json `
  --verification-doc docs/openrgb-component-verification.md `
  --output C:/PrismestraArtifacts/Prismestra-OpenRGB-component.zip
```

The output must be new. The packager checks input pins, required sources/notices, path safety and archive payload hashes/CRC; it does not run a binary, install a driver, approve licenses or prove source-to-binary reproducibility. Its tests use synthetic files and no hardware.

## Installations and private task configuration

The shipped portable setup is `scripts/portable/Configure OpenRGB.ps1` and `Launch Prismestra.ps1`. An optional authorized helper must use [the protocol2 clock contract](startup-recovery.md); `scripts/windows/Helper Clock.ps1` provides the launcher-side clock check without creating a task or starting a process.

The maintained general installer is described in [Windows installation](windows-installer.md); its lifecycle and privilege tests are separate release gates. Workstation-specific activation/rollback receipts are evidence, not a supported installer to copy for another user's PC. Do not promote old scripts containing fixed paths, PIDs, application versions or permission grants. Keep running installations unchanged when reorganizing source files; update a release through its separately checked update path.
