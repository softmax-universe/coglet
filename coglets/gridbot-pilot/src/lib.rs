use coglet_guest_rust::{Action, Context, Event, Host, HostError, coglet};

/// Supervisor-pinned reference pilot identity (not caller-selectable).
pub const ID: &str = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

use ciborium::Value;
use std::collections::BTreeMap;

fn number(payload: &BTreeMap<String, Value>, key: &str) -> Result<i64, HostError> {
    payload
        .get(key)
        .and_then(|v| v.as_integer())
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(HostError::InvalidArgument)
}

#[coglet]
pub fn wake<H: Host>(ctx: &mut Context<'_, H>, events: &[Event]) -> Result<(), HostError> {
    for event in events {
        if !matches!(
            event.kind.as_str(),
            "sim.gridbot_state" | "sim.cartpole_state"
        ) {
            continue;
        }
        let payload = &event.payload;
        match payload.get("done").and_then(|v| v.as_bool()) {
            Some(true) => continue,
            Some(false) => (),
            None => return Err(HostError::InvalidArgument),
        }
        let (capability, direction) = if event.kind == "sim.cartpole_state" {
            // Small deterministic PD controller; widened arithmetic cannot overflow
            // on even malformed i64 observations. No state or ambient timing.
            let signal = 4 * i128::from(number(payload, "angle")?)
                + i128::from(number(payload, "angular_velocity")?);
            (
                "sim.cartpole_force",
                if signal >= 0 { "right" } else { "left" },
            )
        } else {
            let (x, y, gx, gy) = (
                number(payload, "x")?,
                number(payload, "y")?,
                number(payload, "goal_x")?,
                number(payload, "goal_y")?,
            );
            let direction = if x < gx {
                "east"
            } else if x > gx {
                "west"
            } else if y < gy {
                "south"
            } else if y > gy {
                "north"
            } else {
                continue;
            };
            ("sim.gridbot_move", direction)
        };
        let deadline = ctx
            .logical_time
            .checked_add(10_000_000)
            .ok_or(HostError::InvalidArgument)?;
        let id = ctx.next_id()?;
        ctx.emit(&Action {
            id,
            coglet: ID.into(),
            capability: capability.into(),
            args: BTreeMap::from([("direction".into(), Value::Text(direction.into()))]),
            deadline,
            correlation_id: event.correlation_id.clone(),
        })?;
    }
    Ok(())
}
