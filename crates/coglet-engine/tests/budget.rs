use coglet_engine::{Error, StepBudget};
use std::time::{Duration, Instant};

#[test]
fn explicit_charge_is_cumulative_and_zero_is_not_unlimited() {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut b = StepBudget::new(7, 10, deadline).unwrap();
    b.charge_compute(3).unwrap();
    b.charge_compute(4).unwrap();
    assert_eq!(b.compute_used(), 7);
    assert_eq!(b.before_guest_call(), Err(Error::ComputeExhausted));
    assert_eq!(b.charge_compute(1), Err(Error::ComputeExhausted));
    assert!(StepBudget::new(0, 10, deadline).is_err());
    assert!(StepBudget::new(1, 0, deadline).is_err());
}
