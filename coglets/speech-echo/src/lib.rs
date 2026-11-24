//! Model-free semantic speech fixture: real guest event decoding and ABI emit.
use coglet_guest_rust::{Action, Context, Event, Host, HostError, coglet};
use coglet_types::Value;

// Rust's panic diagnostics retain these WASI imports even though this fixture
// never requests environment or stdout. Bind them locally to deterministic guest
// traps: no ambient WASI host permission, fake I/O success or host abort.
#[cfg(target_arch = "wasm32")]
mod deny_ambient {
    #[unsafe(no_mangle)]
    pub extern "C" fn __imported_wasi_snapshot_preview1_environ_get(_: u32, _: u32) -> u32 {
        std::arch::wasm32::unreachable()
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn __imported_wasi_snapshot_preview1_environ_sizes_get(_: u32, _: u32) -> u32 {
        std::arch::wasm32::unreachable()
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn __imported_wasi_snapshot_preview1_fd_write(
        _: u32,
        _: u32,
        _: u32,
        _: u32,
    ) -> u32 {
        std::arch::wasm32::unreachable()
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn __imported_wasi_snapshot_preview1_proc_exit(_: u32) {
        std::arch::wasm32::unreachable()
    }
}

pub const ID: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn maybe_trap() {
    #[cfg(all(feature = "trap-injection", target_arch = "wasm32"))]
    std::arch::wasm32::unreachable();
}

#[coglet]
pub fn wake<H: Host>(ctx: &mut Context<'_, H>, events: &[Event]) -> Result<(), HostError> {
    for event in events.iter().filter(|e| e.kind == "speech.end") {
        // Deliberately fail before emit. This compiles to a guest unreachable,
        // not a native host panic and not a canned host-side action.
        maybe_trap();
        let text = event
            .payload
            .get("text")
            .and_then(|v| v.as_text())
            .ok_or(HostError::InvalidArgument)?;
        if text.is_empty() {
            continue;
        }
        // One legal final chunk. Never split a UTF-8 scalar at the byte limit.
        let mut end = text.len().min(4096);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let id = ctx.next_id()?;
        let stream = ctx.next_id()?;
        ctx.emit(&Action {
            id,
            coglet: ID.into(),
            capability: "speech.speak_chunk".into(),
            args: [
                ("stream".into(), Value::Text(stream)),
                ("chunk".into(), Value::Integer(0.into())),
                ("text".into(), Value::Text(text[..end].into())),
                ("final".into(), Value::Bool(true)),
            ]
            .into(),
            deadline: ctx
                .logical_time
                .checked_add(30_000_000)
                .ok_or(HostError::InvalidArgument)?,
            correlation_id: event.correlation_id.clone(),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coglet_guest_rust::Operation;
    #[derive(Default)]
    struct Capture(Vec<Action>);
    impl Host for Capture {
        fn call(&mut self, op: Operation, bytes: &[u8]) -> i32 {
            assert_eq!(op, Operation::Emit);
            let action: Action = coglet_types::decode("action", bytes)
                .unwrap()
                .deserialized()
                .unwrap();
            self.0.push(action);
            0
        }
    }
    fn event(kind: &str, text: &str) -> Event {
        Event {
            source: format!("surface:{}", "b".repeat(64)),
            kind: kind.into(),
            seq: 1,
            logical_time: 100,
            payload: [("text".into(), Value::Text(text.into()))].into(),
            correlation_id: "d".repeat(32),
        }
    }
    #[test]
    fn end_echoes_actual_text_and_ignores_other_events_and_empty_finals() {
        let mut host = Capture::default();
        let mut ctx = Context::new(&mut host, 0, 100, 42);
        wake(
            &mut ctx,
            &[
                event("speech.partial", "ignore"),
                event("speech.end", ""),
                event("speech.end", "Hello, 世界"),
            ],
        )
        .unwrap();
        assert_eq!(host.0.len(), 1);
        assert_eq!(host.0[0].args["text"].as_text().unwrap(), "Hello, 世界");
        assert_eq!(host.0[0].args["chunk"], Value::Integer(0.into()));
        assert_eq!(host.0[0].args["final"], Value::Bool(true));
        assert_eq!(host.0[0].capability, "speech.speak_chunk");
        assert_ne!(host.0[0].id, host.0[0].args["stream"].as_text().unwrap());
    }
    #[test]
    fn truncates_only_at_utf8_boundary() {
        let mut host = Capture::default();
        wake(
            &mut Context::new(&mut host, 0, 100, 42),
            &[event("speech.end", &"界".repeat(2000))],
        )
        .unwrap();
        assert_eq!(host.0[0].args["text"].as_text().unwrap().len(), 4095);
    }
}
