use coglet_guest_rust::{Action, Context, Event, Host, HostError, coglet};

const ID: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[coglet]
pub fn wake<H: Host>(ctx: &mut Context<'_, H>, events: &[Event]) -> Result<(), HostError> {
    for event in events.iter().filter(|e| e.kind == "test.echo") {
        let deadline = ctx
            .logical_time
            .checked_add(1_000_000)
            .ok_or(HostError::InvalidArgument)?;
        let id = ctx.next_id()?;
        ctx.emit(&Action {
            id,
            coglet: ID.into(),
            capability: "test.echo".into(),
            args: event.payload.clone(),
            deadline,
            correlation_id: event.correlation_id.clone(),
        })?;
    }
    Ok(())
}
