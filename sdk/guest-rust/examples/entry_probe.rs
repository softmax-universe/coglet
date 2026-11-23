#![no_main]
use coglet_guest_rust::{Context, Event, Host, HostError, coglet};
#[coglet]
fn wake<H: Host>(ctx: &mut Context<'_, H>, _events: &[Event]) -> Result<(), HostError> {
    let id = ctx.next_id()?;
    ctx.timer(&id, ctx.logical_time)
}
