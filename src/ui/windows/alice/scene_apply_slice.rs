//! Draft native slice. Driven by fixtures until saved-scene dispatch and local
//! assignment editing are wired in the product.
use super::{Change, Lamp, ON, RANGE, Value, apply_payload_with, command, lamps, tests::fixture};
use serde_json::json;
use std::{cell::Cell, num::NonZeroU32};

enum Source {
    Manual(NonZeroU32),
    Preview,
    Restore,
    Startup,
}

struct Approval<'a> {
    scene_id: &'a str,
    lamp: &'a Lamp,
    allow_power_on: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    LocalOnly,
    Duplicate,
    Lamp(Result<(), String>),
}

#[derive(Default)]
struct SceneApply {
    last_manual: Option<NonZeroU32>,
}

impl SceneApply {
    fn apply(
        &mut self,
        source: Source,
        scene_id: &str,
        approval: Option<&Approval<'_>>,
        brightness: u8,
        authorize: impl FnOnce() -> Result<(), String>,
        apply_local: impl FnOnce() -> Result<(), String>,
        request: impl FnMut(&str, Option<&[u8]>) -> Result<Value, String>,
    ) -> Result<Outcome, String> {
        let Source::Manual(serial) = source else {
            return Ok(Outcome::LocalOnly);
        };
        if self.last_manual.is_some_and(|last| serial <= last) {
            return Ok(Outcome::Duplicate);
        }
        self.last_manual = Some(serial);
        apply_local()?;
        let Some(approval) = approval.filter(|approval| scene_id == approval.scene_id) else {
            return Ok(Outcome::LocalOnly);
        };
        if !(1..=100).contains(&brightness) {
            return Ok(Outcome::Lamp(Err(
                "Яркость лампы вне диапазона пилота.".into()
            )));
        }
        let result = authorize().and_then(|()| {
            apply_payload_with(approval.lamp, request, |data, caps| {
                command(
                    approval.lamp,
                    data,
                    caps,
                    Change::Brightness(brightness),
                    approval.allow_power_on,
                )
            })
        });
        Ok(Outcome::Lamp(result))
    }
}

fn manual(id: u32) -> Source {
    Source::Manual(NonZeroU32::new(id).unwrap())
}

#[test]
fn ordinary_apply_checks_local_success_and_exact_approval_before_any_network() {
    let catalog = fixture();
    let lamp = &lamps(&catalog).unwrap()[1];
    let approval = Approval {
        scene_id: "scene-1",
        lamp,
        allow_power_on: false,
    };
    let mut dispatcher = SceneApply::default();
    let local_count = Cell::new(0);
    let local = || {
        local_count.set(local_count.get() + 1);
        Ok(())
    };
    for source in [Source::Preview, Source::Restore, Source::Startup] {
        assert_eq!(
            dispatcher.apply(
                source,
                "scene-1",
                Some(&approval),
                40,
                || unreachable!(),
                || unreachable!(),
                |_, _| unreachable!()
            ),
            Ok(Outcome::LocalOnly)
        );
    }
    for (serial, scene, approved) in [(1, "scene-1", None), (2, "scene-2", Some(&approval))] {
        assert_eq!(
            dispatcher.apply(
                manual(serial),
                scene,
                approved,
                40,
                || unreachable!(),
                local,
                |_, _| unreachable!()
            ),
            Ok(Outcome::LocalOnly)
        );
    }
    assert_eq!(local_count.get(), 2);
    assert_eq!(
        dispatcher.apply(
            manual(3),
            "scene-1",
            Some(&approval),
            40,
            || unreachable!(),
            || Err("local apply failed".into()),
            |_, _| unreachable!()
        ),
        Err("local apply failed".into())
    );
    assert_eq!(
        dispatcher.apply(
            manual(3),
            "scene-1",
            Some(&approval),
            40,
            || unreachable!(),
            || unreachable!(),
            |_, _| unreachable!()
        ),
        Ok(Outcome::Duplicate)
    );
    assert_eq!(
        dispatcher.apply(
            manual(4),
            "scene-1",
            Some(&approval),
            40,
            || Err("access expired".into()),
            local,
            |_, _| unreachable!()
        ),
        Ok(Outcome::Lamp(Err("access expired".into())))
    );
    assert_eq!(local_count.get(), 3);
    assert_eq!(
        dispatcher.apply(
            manual(4),
            "scene-1",
            Some(&approval),
            40,
            || unreachable!(),
            || unreachable!(),
            |_, _| unreachable!()
        ),
        Ok(Outcome::Duplicate)
    );
}

#[test]
fn one_id_and_one_post_for_success_or_ambiguous_reply_no_post_before_preflight() {
    let catalog = fixture();
    let lamp = &lamps(&catalog).unwrap()[1];
    for (on, power, problem, expected_posts) in [
        (true, false, "", 1),
        (false, true, "", 1),
        (false, false, "off", 0),
        (true, false, "before", 0),
        (true, false, "after", 1),
        (true, false, "rejected", 1),
        (false, true, "partial", 1),
        (true, false, "wrong_id", 1),
    ] {
        let approval = Approval {
            scene_id: "scene-1",
            lamp,
            allow_power_on: power,
        };
        let mut dispatcher = SceneApply::default();
        let locally_applied = Cell::new(false);
        let mut posts = 0;
        let outcome = dispatcher.apply(manual(1), "scene-1", Some(&approval), 40, || Ok(()),
            || { locally_applied.set(true); Ok(()) }, |path, body| {
            assert!(locally_applied.get());
            match path {
                "/v1.0/user/info" if problem == "before" => Err("unavailable".into()),
                "/v1.0/user/info" => Ok(catalog.clone()),
                "/v1.0/devices/b" => Ok(json!({"status":"ok","id":"b","room":"r","state":"online",
                    "capabilities":[{"type":ON,"state":{"value":on}},
                        {"type":RANGE,"state":{"instance":"brightness","value":70}}]})),
                "/v1.0/devices/actions" => {
                    posts += 1;
                    let mut actions = Vec::new();
                    if !on {
                        actions.push(json!({"type":ON,"state":{"instance":"on","value":true}}));
                    }
                    actions.push(json!({"type":RANGE,"state":{"instance":"brightness","value":40,"relative":false}}));
                    assert_eq!(serde_json::from_slice::<Value>(body.unwrap()).unwrap(),
                        json!({"devices":[{"id":"b","actions":actions}]}));
                    if problem == "after" {
                        return Err("lost reply".into());
                    }
                    let mut results = Vec::new();
                    if !on && problem != "partial" {
                        results.push(json!({"type":ON,"state":{"instance":"on","action_result":{"status":"DONE"}}}));
                    }
                    let status = if problem == "rejected" { "ERROR" } else { "DONE" };
                    results.push(json!({"type":RANGE,"state":{"instance":"brightness","action_result":{"status":status}}}));
                    let id = if problem == "wrong_id" { "a" } else { "b" };
                    Ok(json!({"status":"ok","devices":[{"id":id,"capabilities":results}]}))
                }
                _ => panic!("accessed a different lamp or group: {path}"),
            }
        }).unwrap();
        assert_eq!(posts, expected_posts, "{problem}");
        assert_eq!(
            matches!(outcome, Outcome::Lamp(Ok(()))),
            problem.is_empty(),
            "{problem}"
        );
        assert_eq!(
            dispatcher.apply(
                manual(1),
                "scene-1",
                Some(&approval),
                40,
                || unreachable!(),
                || unreachable!(),
                |_, _| unreachable!()
            ),
            Ok(Outcome::Duplicate)
        );
    }
}
