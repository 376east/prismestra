# Report a Surplife E1 strip test

Use a test strip you control and an isolated Prismestra profile. Start the build provided for testing and select that controller in Connections. Leave its configured LED count and firmware as they are. If another lighting application owns the strip, arrange access to that strip before applying output. [Connection setup](universal-connections.md)

## What to test

1. **Connect.** Record the model, firmware, build and Windows version. In the Surplife E1 connection, check that the reported point count agrees with the test profile. Apply the connection. Record discovery and activation separately.
2. **Check two adjacent points.** With a known marker input for the test profile, light index 0 red and index 1 green, with all other points dark. Expect two adjacent physical lights, each with its own color. For a strip configured with 153 points, then light index 76 blue and index 152 amber. Expect one light near the middle and one at the end. Use valid indices for other lengths and record them. If the build was supplied without a marker input, mark the index check `not run`.
3. **Check recovery.** While the test pattern is running, interrupt only the test controller's connection. Change the active scene while the controller is unavailable, then reconnect it. Record whether the controller returns and shows the new scene. Label this a link interruption. Report a mid-frame interruption separately only if a trace shows an accepted fragment, a failed fragment and no later fragments in that frame. The offline fixture can reproduce that exact boundary.
4. **Check dark output and Stop separately.** Apply an all-black frame and check that the strip goes dark. Return to color, use Stop and record the physical state left on the strip. Record negotiated ATT MTU if available. Leave it `unknown` if the build does not expose it. Physical recovery and output at the minimum ATT MTU of 23 remain unconfirmed. [Transport and historical observations](surplife-e1-pixels.md)

The marker check verifies the indices you observed. A controller's reported count and successful packet submission establish software results only. [Compatibility evidence levels](compatibility.md#what-counts-as-support)

## What to send

Copy the fields below into a [device report](https://github.com/376east/prismestra/issues/new?template=device.yml). Use `unknown` for unreported firmware or MTU and `not run` for untested steps.

```text
Controller model / hardware revision / firmware =
Prismestra build or commit / Windows version =
Connection = BLE, Surplife E1
Reported points / profile points / ATT MTU =
Discovery / activation =
Indices and colors tried / physical positions observed =
Black frame / Stop =
Interruption type (link or traced mid-frame) / reconnect / scene after reconnect =
```

For evidence, copy only the stage, result and relevant counts into a short excerpt. Keep the BLE address, serial number, computer name, personal paths, profile, screenshots of Connections and raw captures out of the report. Use `controller A` if you need to refer to the same device across rows. A fixture example is shown below. It contains no physical observation.

```text
evidence=offline_fixture
capability=EA81 result=accepted points=153
output=E104 fragment=0 result=accepted
output=E104 fragment=1 result=link_lost remaining_writes=0 completed_frames=0
retry=1 result=unexpected_controller capability_queries=0 color_writes=0
retry=2 result=point_count_mismatch expected_points=153 reported_points=152 color_writes=0
retry=3 result=restored points=153 first_frame_fragments=5 scene=current
```

In a Windows source checkout, the test below checks that a failed fragment stops the old frame and a fresh connection submits all 153 positions in order. It uses a fixture link.

```powershell
cargo test --locked --lib surplife_reconnect_sends_all_153_distinct_colors_from_a_fresh_frame -- --test-threads=1
```
