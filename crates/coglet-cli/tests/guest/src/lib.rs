#![no_std]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn abi_alloc(_: u32) -> u32 {
    8192
}
#[unsafe(no_mangle)]
pub extern "C" fn handle(_: u32, _: u32, _: u64, _: u64, _: u64) -> i32 {
    0
}
