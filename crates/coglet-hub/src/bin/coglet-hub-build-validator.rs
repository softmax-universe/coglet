//! Package-local launcher for the frozen resource-bounded validator. No validator
//! logic or authority is duplicated here; install alongside the router service.
fn main() {
    std::process::exit(coglet_proto::validator::helper_main());
}
