#![no_std]
#![no_main]
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

// Real initialized linear-memory data, not a custom-section size pad.
const fn table() -> [u8; 196608] {
    let mut a = [0; 196608];
    let mut i = 0;
    while i < a.len() {
        a[i] = ((i * 73 + i / 251) % 256) as u8;
        i += 1;
    }
    a
}
static TABLE: [u8; 196608] = table();
static mut PROGRESS: u32 = 0;

#[no_mangle]
pub extern "C" fn handle(n: u32) -> u32 {
    let mut sum = 0u32;
    for i in 0..n {
        sum = sum.wrapping_add(unsafe {
            read_volatile(TABLE.as_ptr().add(i as usize % TABLE.len())) as u32
        });
    }
    sum
}
#[no_mangle]
pub extern "C" fn runaway() {
    loop {
        unsafe {
            let n = read_volatile(addr_of!(PROGRESS));
            write_volatile(addr_of_mut!(PROGRESS), n.wrapping_add(1));
        }
    }
}
#[no_mangle]
pub extern "C" fn progress() -> u32 {
    unsafe { read_volatile(addr_of!(PROGRESS)) }
}
#[no_mangle]
pub extern "C" fn pages() -> u32 {
    core::arch::wasm32::memory_size::<0>() as u32
}
#[no_mangle]
pub extern "C" fn grow(n: u32) -> u32 {
    core::arch::wasm32::memory_grow::<0>(n as usize) as u32
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
