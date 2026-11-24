use super::*;
fn save_setup(h: &Host) {
    std::fs::write(
        h.dir.join("setup.json"),
        serde_json::to_vec(&h.setup).unwrap(),
    )
    .unwrap();
}
pub(super) fn daemon_reads() {
    for (name, value, good) in [
        ("read-valid", json!({"paused":false}), true),
        ("read-sibling", json!({"count":2}), false),
    ] {
        let mut h = Host::new(name, false, RAW, &[C]);
        let id = "a".repeat(32);
        h.setup["supervisor_reads"] = json!([{"surface":S,"coglet":C,"operation":"game.state","args":{},"correlation_id":id,"timeout_us":1_000_000}]);
        save_setup(&h);
        h.start(true);
        let mut l = h.connect();
        l.ready(&artifact(), &all_ops());
        let read = l.recv("read")["body"].clone();
        assert_eq!(read["capability"], "game.state");
        assert_eq!(read["args"], json!({}));
        // Original binding is durable BEFORE bytes are exposed.
        // WAL is exclusively owned while running; read it after daemon stop below.
        l.send(
            "read_reply",
            json!({"correlation_id":id,"ok":true,"value":value,"snapshot":1200}),
        );
        if good {
            l.barrier();
        } else {
            l.refused();
        }
        h.stop();
        drop(l);
        let state = h.state();
        let record = &state["supervisor_reads"][&id];
        assert_eq!(record["request"]["operation"], "game.state");
        assert_eq!(record["request"]["args"], json!({}));
        assert_eq!(record["attempted"], true);
        if good {
            assert_eq!(record["response"]["value"], value);
        } else {
            assert!(record["response"].is_null());
        }
        assert!(
            state["private_events"].as_object().unwrap().is_empty(),
            "read invented guest event"
        );
        h.start(true);
        let mut l = h.connect();
        l.hello();
        // No automatic resampling, including an attempted read without a result.
        l.barrier();
        h.stop();
        drop(l);
        assert_eq!(h.state()["supervisor_reads"], state["supervisor_reads"]);
    }
}
pub(super) fn resource_matrix() {
    for (operation, args, missing) in [
        ("game.move", json!({"tile":7}), "game.control"),
        ("game.highlight", json!({"tile":7}), "game.overlay"),
        ("game.helper_spawn", json!({}), "game.control"),
        ("game.helper_spawn", json!({}), "game.helper"),
    ] {
        for deny in [false, true] {
            let mut h = Host::new(
                &format!("resource-{operation}-{missing}-{deny}"),
                true,
                RAW,
                &[C],
            );
            h.setup["initial_actions"][0]["capability"] = json!(operation);
            h.setup["initial_actions"][0]["args"] = args.clone();
            save_setup(&h);
            h.start(true);
            let mut l = h.connect();
            l.hello();
            l.send("capabilities", capabilities(&artifact()));
            let mut g = grants(&all_ops(), C);
            if deny {
                g["entries"][0]["resources"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|r| r != missing);
            }
            l.send("grants", g);
            if !deny {
                assert_eq!(l.recv("act")["body"]["action"]["capability"], operation);
            }
            l.barrier();
            h.stop();
            drop(l);
            let state = h.state();
            assert_eq!(
                state["dispatches"]
                    .as_object()
                    .unwrap()
                    .contains_key(&"4".repeat(32)),
                !deny,
                "{operation}/{missing}"
            );
        }
    }
}
pub(super) fn fenced_results() {
    for fence in ["revoked", "replacement", "kill", "reconnect"] {
        let mut h = Host::new(fence, true, RAW, &[C]);
        h.start(true);
        let mut l = h.connect();
        l.ready(&artifact(), &all_ops());
        let act = l.recv("act")["body"]["action"].clone();
        l.send(
            "accept",
            json!({"id":act["id"],"correlation_id":act["correlation_id"],"accepted":true}),
        );
        l.barrier();
        match fence {
            "revoked" | "replacement" => {
                let mut g = grants(&all_ops(), C);
                g["epoch"] = json!(2);
                if fence == "revoked" {
                    g["entries"] = json!([]);
                }
                l.send("grants", g);
            }
            "kill" => l.send(
                "kill",
                json!({"reason":"fixture","correlation_id":"b".repeat(32)}),
            ),
            _ => {
                drop(l);
                l = h.connect();
                l.hello();
                assert_eq!(l.recv("act")["body"]["action"], act);
            }
        }
        l.send("event",json!({"source":S,"kind":"action.outcome","seq":1,"logical_time":1500,"payload":{"id":act["id"],"correlation_id":act["correlation_id"],"status":"succeeded","result":{}},"correlation_id":act["correlation_id"]}));
        l.barrier();
        h.stop();
        drop(l);
        let state = h.state();
        assert_eq!(
            state["dispatches"]["4".repeat(32)]["outcome"]["status"],
            "succeeded",
            "retain late effect audit"
        );
        assert!(
            state["staged_boundary"].is_null(),
            "{fence}: private result leaked to inbox"
        );
        drop(
            RuntimeService::open_with_catalog(
                &h.dir.join("state.wal"),
                &h.setup,
                128,
                catalog(RAW),
            )
            .unwrap(),
        );
    }
}
