# Prismestra project rules

## Research before a new mechanism

When a bug, compatibility gap or architectural obstacle appears, first check existing solutions before adding another driver, workaround, dependency or fork.

- Inspect the current implementation and reproduce the actual failure. Separate transport readiness, discovery, resource ownership and physical output.
- Search relevant upstream code, documentation, issues, releases and comparable open-source projects. Community answers, including Stack Overflow, are leads; verify applicability against primary sources and the versions involved.
- Reuse an existing supported API, adapter or upstream fix when it meets the requirement. Record the source URL, revision, license, what is reused and the remaining gap. Open access to code is not permission to redistribute it.
- Prefer a small isolated upstream patch over maintaining an unrelated copy of a whole product. Explain why a new mechanism or fork is needed and how upstream updates will be tracked.
- Stop researching when the decision has enough evidence; do not turn a bounded bug fix into an exhaustive survey. If no usable solution is found, record the search scope and uncertainty rather than claiming none exists.

## User experience and coexistence

- Define the ordinary user's install/run/update/uninstall path before exposing a new hardware dependency. A fresh Windows machine without OpenRGB is an explicit release scenario, not equivalent to the developer's configured PC.
- Treat existing monitoring, fan-control and vendor applications as user-owned. Never automatically stop them, release their mutexes or bypass bus coordination. A one-time restart permission does not authorize later restarts.
- Recover only owned processes and resources. Isolate an unavailable device or bus so that independent paths can continue; do not advertise universal coexistence or hardware support without evidence.
- Two writers may compete for the same lighting controls. Use supported sharing interfaces where available; otherwise explain the affected device and offer an explicit ownership choice without requiring users to abandon unrelated functions.
- Keep a backend's provenance visible: upstream project, pinned source/binary revision, local changes, licenses, build instructions and shipped files. Do not silently replace or reuse a separately installed backend.
- Distinguish the shipped implementation from the intended experience. Software fixtures, a configured local kit, clean-machine installation and physical acceptance are different evidence levels.
