use coglet_proto::scheduler::{MAX_QUEUED_BYTES, PeerScheduler};

#[test]
fn bulk_leaves_one_control_slot_and_permits_release_on_drop() {
    let scheduler = PeerScheduler::default();
    let mut held = Vec::new();
    for _ in 0..15 {
        held.push(scheduler.admit("hub.status", 1).unwrap());
    }
    assert!(scheduler.admit("hub.status", 1).is_err());
    let control = scheduler.admit("hub.stop", 1).unwrap();
    assert!(scheduler.admit("hub.cancel", 1).is_err());
    drop(control);
    drop(held.pop());
    assert!(scheduler.admit("hub.status", 1).is_ok());
}

#[test]
fn classification_shares_total_capacity_and_is_bounded_to_two() {
    let scheduler = PeerScheduler::default();
    let mut a = scheduler.classify().unwrap();
    let mut b = scheduler.classify().unwrap();
    assert!(scheduler.classify().is_err());
    let a = a.admit("hub.status", 1).unwrap();
    let b = b.admit("hub.status", 1).unwrap();
    let mut held = vec![a, b];
    for _ in 0..13 {
        held.push(scheduler.admit("hub.status", 1).unwrap());
    }
    let mut provisional = scheduler.classify().unwrap();
    assert!(scheduler.classify().is_err());
    assert!(provisional.admit("hub.status", 1).is_err());
    drop(provisional);
    let mut provisional = scheduler.classify().unwrap();
    let stop = provisional.admit("hub.stop", 1).unwrap();
    drop(stop);
    assert!(scheduler.classify().unwrap().admit("hub.cancel", 1).is_ok());
}

#[test]
fn separate_rate_buckets_do_not_let_health_or_enrollment_starve_stop() {
    let scheduler = PeerScheduler::default();
    for _ in 0..1000 {
        drop(scheduler.admit("hub.health", 0));
    }
    assert!(scheduler.admit("hub.health", 0).is_err());
    assert!(scheduler.admit("hub.enroll", 0).is_ok());
    assert!(scheduler.admit("hub.stop", 0).is_ok());
    for _ in 0..1000 {
        drop(scheduler.admit("hub.enroll", 0));
    }
    assert!(scheduler.admit("hub.enroll", 0).is_err());
    assert!(scheduler.admit("hub.cancel", 0).is_ok());
}

#[test]
fn decoded_byte_saturation_leaves_control_dispatch_budget() {
    let scheduler = PeerScheduler::default();
    let _bulk = scheduler
        .admit("hub.status", MAX_QUEUED_BYTES - 65536)
        .unwrap();
    assert!(scheduler.admit("hub.status", 1).is_err());
    assert!(scheduler.admit("hub.cancel", 65536).is_ok());
}

#[test]
fn git_and_decoded_bytes_have_independent_caps() {
    let scheduler = PeerScheduler::default();
    let first = scheduler
        .admit("hub.git.push", MAX_QUEUED_BYTES / 2)
        .unwrap();
    let second = scheduler
        .admit("hub.git.fetch", MAX_QUEUED_BYTES / 2 - 65536)
        .unwrap();
    assert!(scheduler.admit("hub.git.push", 0).is_err());
    assert!(scheduler.admit("hub.status", 1).is_err());
    drop(first);
    assert!(scheduler.admit("hub.git.push", 1).is_ok());
    drop(second);
}
