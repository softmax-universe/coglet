use coglet_runtime::{SurfaceBinding, SurfaceSession, cbor};
use serde_json::{Value, json};
fn send(session: &mut SurfaceSession, kind: &str, body: Value) -> Result<(), String> {
    match session.send(&cbor(&json!({"type":kind,"body":body}))?, 0) {
        0 => Ok(()),
        code => Err(format!("surface send status {code}")),
    }
}
#[test]
fn sandbox_operations_require_singleton_supervisor_binding() {
    let surface = format!("surface:{}", "a".repeat(64));
    for name in ["gridbot", "cartpole"] {
        for count in [0, 1, 2] {
            let pilots = ["b", "c"]
                .into_iter()
                .take(count)
                .map(|v| format!("coglet:{}", v.repeat(64)))
                .collect();
            let mut session =
                SurfaceSession::new(SurfaceBinding::new(surface.clone(), pilots).unwrap());
            send(&mut session, "hello", json!({"surface":surface,"offer":{"versions":["0.1"]},"clock":0,"correlation_id":"1".repeat(32)})).unwrap();
            let event = format!("sim.{name}_state");
            let action = format!(
                "sim.{name}_{}",
                if name == "gridbot" { "move" } else { "force" }
            );
            let result = send(
                &mut session,
                "capabilities",
                json!({"revision":1,"clock":0,"correlation_id":"2".repeat(32),"entries":[{
                    "name":format!("sim.{name}"),"events":[event.clone()],"reads":[],"actions":[action.clone()],"exclusive":[],
                    "schemas":{event:format!("https://coglet.dev/spec/messages/sandbox.schema.json#/$defs/{name}-state"),action:format!("https://coglet.dev/spec/messages/sandbox.schema.json#/$defs/{name}-{}",if name=="gridbot" {"move"}else{"force"})}
                }]}),
            );
            assert_eq!(
                result.is_ok(),
                count == 1,
                "{name} permitted pilot count {count}: {result:?}"
            );
        }
    }
}
