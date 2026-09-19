#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use uefi::{Status, entry};

#[entry]
fn main() -> Status {
    if let Err(error) = uefi::helpers::init() {
        return error.status();
    }
    log::info!("LIMINESCREEN_TARGET_{}", env!("LIMINESCREEN_TEST_MARKER"));
    Status::SUCCESS
}
