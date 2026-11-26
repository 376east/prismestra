# Schedule test fixture

`schedule-native.bin` contains 4,096 synthetic cases produced by the original C++ `SceneSchedule.h` with Qt Core 6.8.3. The generator, `schedule-oracle.cpp`, builds against that header through `SCENE_SCHEDULE_HEADER`. It can generate the fixture without running an installed lighting app.

The [Rust schedule test](../schedule.rs) compares the most recent PC and door events and the next scheduled event with the reference output. The cases cover 2,048 default schedules (10:00, 17:00, 02:00) and 2,048 distinct randomized schedules from 2025 through 2035. They include one second before and after events, midnight, year changes and leap years. The fixture uses Europe/Moscow. Separate Rust tests cover other time zones, clock rollback, failure, backoff and persistence.

**Format.** Little-endian `SCH1` magic and a `u32` count, then 40 bytes per record: UTC timestamp (`i64`), three minute values (`u16`), PC timestamp and on/off (`i64`, `u8`), door timestamp and on/off (`i64`, `u8`), and the next UTC timestamp (`i64`).

**SHA-256** `1316cd9d0f7a885c0b5f1c6110556cc76494cf3dc84b03f4af2eb4f28408387f`.
