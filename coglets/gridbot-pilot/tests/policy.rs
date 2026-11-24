use coglet_gridbot_pilot::wake;
use coglet_guest_rust::{Context, Event, Host, Operation};
use serde_json::json;

#[derive(Default)]
struct Capture(Vec<serde_json::Value>);
impl Host for Capture {
    fn call(&mut self, operation: Operation, bytes: &[u8]) -> i32 {
        assert_eq!(operation, Operation::Emit);
        self.0
            .push(coglet_types::decode_typed("common::action", bytes).unwrap());
        0
    }
}
fn event(kind: &str, payload: serde_json::Value) -> Event {
    serde_json::from_value(json!({"source": format!("surface:{}", "a".repeat(64)),
        "kind": kind, "payload": payload, "seq": 0, "logical_time": 10,
        "correlation_id": "11111111111111111111111111111111"}))
    .unwrap()
}

#[test]
fn gridbot_moves_toward_goal_with_sdk_id_and_deadline() {
    for (x, y, direction) in [
        (0, 0, "east"),
        (3, 0, "west"),
        (2, 0, "south"),
        (2, 3, "north"),
    ] {
        let mut host = Capture::default();
        wake(
            &mut Context::new(&mut host, 7, 10, 42),
            &[event(
                "sim.gridbot_state",
                json!({"x":x,"y":y,"goal_x":2,"goal_y":2,"done":false}),
            )],
        )
        .unwrap();
        assert_eq!(host.0.len(), 1);
        let act = &host.0[0];
        assert_eq!(act["capability"], "sim.gridbot_move");
        assert_eq!(act["args"], json!({"direction":direction}));
        assert_eq!(act["deadline"], 10_000_010);
        assert_eq!(act["id"].as_str().unwrap().len(), 32);
    }
}

#[test]
fn cartpole_controller_responds_to_angle_and_rate() {
    for (angle, rate, direction) in [
        (10_000, 0, "right"),
        (-10_000, 0, "left"),
        (0, 10_000, "right"),
        (0, -10_000, "left"),
    ] {
        let mut host = Capture::default();
        wake(
            &mut Context::new(&mut host, 2, 10, 42),
            &[event(
                "sim.cartpole_state",
                json!({"angle":angle,"angular_velocity":rate,"done":false}),
            )],
        )
        .unwrap();
        assert_eq!(host.0.len(), 1);
        assert_eq!(host.0[0]["capability"], "sim.cartpole_force");
        assert_eq!(host.0[0]["args"], json!({"direction":direction}));
    }
}

#[test]
fn terminal_and_outcome_wakes_do_not_loop() {
    let mut host = Capture::default();
    wake(
        &mut Context::new(&mut host, 1, 0, 0),
        &[
            event("sim.gridbot_state", json!({"done":true})),
            event("sim.cartpole_state", json!({"done":true})),
            event("action.outcome", json!({})),
        ],
    )
    .unwrap();
    assert!(host.0.is_empty());
}
