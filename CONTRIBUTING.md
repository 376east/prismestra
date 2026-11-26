# Contributing

## Where help is useful

- **Have a listed device?** Try discovery, a static color and a scene. [Tell us what worked](https://github.com/376east/prismestra/issues/new?template=device.yml) with the model, revision, firmware, connection method and backend version.
- **Want a small code change?** Find a model missing from search under its retail name. Add a verified alias without merging distinct hardware revisions and run `node --test docs/devices/lookup.test.mjs`.
- **Found an app or installer bug?** Share the steps and the expected result, or send a fix with a focused regression check. [Report an app bug](https://github.com/376east/prismestra/issues/new?template=bug.yml)

The [documentation map](docs/README.md) points to the relevant guide, code and tests. The [roadmap](docs/roadmap.md) and [device expansion list](docs/devices/expansion.md) explain where larger work is useful.

## Send your first change

1. Fork the repository, clone your fork and create a branch from `master`.
2. Change one behavior. Follow the [development guide](docs/development.md#check-and-build) for setup and run the checks relevant to your patch.
3. Open a PR against `master`. Explain what failed or was missing, what changes for the user and how you checked it. For hardware claims, name the model, firmware and operation you actually tested.

Review starts with a single problem and a diff the maintainer can verify. Split unrelated fixes, formatting and refactors into separate PRs. For a large change, agree on the approach first and send reviewable steps. Use short English commit messages such as `Preserve matrix holes in OpenRGB discovery`.

Rust and the existing Windows desktop stack are the starting point. Small changes should use existing code and dependencies. For a new runtime dependency, backend, architecture or stack change, discuss the need, alternatives, installer impact and maintenance cost before opening a large PR.

## Code and evidence

Use English for identifiers, comments and engineering documentation. The interface supports English and Russian. Keep translations in the relevant UI catalog or localized strings and preserve tests that exercise them.

Before adding a new backend, check whether OpenRGB, WLED or Home Assistant already exposes the required operation. Keep discovery separate from activation. Treat transport submission and physical device confirmation as different outcomes. Never add fan or pump writes to a lighting adapter.

For a device report, include its exact model, revision, firmware, connection mode, backend version and the operations you observed. Remove tokens, serial numbers, MAC addresses, private network details and personal paths from attached logs. Do not upload firmware, vendor binaries or captures you cannot redistribute.

For a code change, describe the concrete problem and resulting behavior. Keep the patch focused and preserve upstream licenses and fixture provenance. New transport tests should use owned temporary files or loopback endpoints, with bounded I/O and a reliable exit path. Physical tests require an explicitly selected device and a recovery path.

Use the [private security form](https://github.com/376east/prismestra/security/advisories/new) for vulnerabilities.
