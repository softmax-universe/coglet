use coglet_guest_rust::{Context, Event, Host, HostError, LogEntry, coglet};

#[coglet]
pub fn wake<H: Host>(ctx: &mut Context<'_, H>, events: &[Event]) -> Result<(), HostError> {
    let previous = match ctx.read_file("/data/count")? {
        Some(bytes) => std::str::from_utf8(&bytes)
            .map_err(|_| HostError::InvalidArgument)?
            .parse::<u64>()
            .map_err(|_| HostError::InvalidArgument)?,
        None => 0,
    };
    let count = previous
        .checked_add(events.len() as u64)
        .ok_or(HostError::ResourceExhausted)?;
    ctx.write_file("/data/count", count.to_string().as_bytes())?;
    ctx.log(&LogEntry {
        level: "info".into(),
        message: format!("count={count}"),
        fields: Default::default(),
    })
}
