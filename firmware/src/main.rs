#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

mod firmware;
mod report;

use alloc::vec::Vec;
use uefi::boot::{self, LoadImageSource};
use uefi::proto::BootPolicy;
use uefi::proto::device_path::{DevicePath, build};
use uefi::proto::loaded_image::LoadedImage;
use uefi::{Status, cstr16, entry};

fn chainload() -> Result<(), Status> {
    let device =
        firmware::with_protocol::<LoadedImage, _>(boot::image_handle(), |image| image.device())?
            .ok_or(Status::NOT_FOUND)?;

    let mut storage = Vec::new();
    firmware::with_protocol::<DevicePath, _>(device, |path| {
        let mut builder = build::DevicePathBuilder::with_vec(&mut storage);
        for node in path.node_iter() {
            builder = builder.push(&node).map_err(|_| Status::OUT_OF_RESOURCES)?;
        }
        let target = builder
            .push(&build::media::FilePath {
                path_name: cstr16!("\\EFI\\limine\\limine_x64.efi"),
            })
            .map_err(|_| Status::OUT_OF_RESOURCES)?
            .finalize()
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        boot::load_image(
            boot::image_handle(),
            LoadImageSource::FromDevicePath {
                device_path: target,
                boot_policy: BootPolicy::ExactMatch,
            },
        )
        .map_err(|error| error.status())
    })?
    .and_then(|image| {
        log::info!("liminescreen: starting installed Limine");
        boot::start_image(image).map_err(|error| error.status())
    })
}

#[entry]
fn main() -> Status {
    if let Err(error) = uefi::helpers::init() {
        return error.status();
    }
    log::info!(
        "liminescreen {}: preparing firmware displays",
        env!("CARGO_PKG_VERSION")
    );
    let mut report = report::Report::new();
    report.line(format_args!(
        "liminescreen {} boot report",
        env!("CARGO_PKG_VERSION")
    ));
    report.line(format_args!(
        "firmware time: {:?}",
        uefi::runtime::get_time()
    ));
    firmware::activate_displays(&mut report);
    report.line(format_args!(
        "next: chainload the installed official limine"
    ));
    if let Err(status) = firmware::save_report(&report.finish()) {
        log::warn!("liminescreen: boot report could not be saved: {status:?}");
    }
    match chainload() {
        Ok(()) => Status::SUCCESS,
        Err(status) => {
            log::error!("liminescreen: cannot start installed Limine: {status:?}");
            status // Firmware retains the normal Limine boot entry as fallback.
        }
    }
}
