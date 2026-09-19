//! The firmware boundary. Protocol guards are dropped before connecting drivers
//! or starting another image. No framebuffer writes, PCI writes, or NVRAM writes.

use crate::report::Report;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::{self, NonNull};
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::device_path::{DevicePath, DevicePathNodeEnum};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::{ProtocolPointer, unsafe_protocol};
use uefi::{Handle, Identify, Status, cstr16};
use uefi_raw::protocol::console::GraphicsOutputProtocol;

type PciRead = unsafe extern "efiapi" fn(*const PciIo, u32, u32, usize, *mut c_void) -> Status;

/// Prefix of EFI_PCI_IO_PROTOCOL through Pci.Read, per UEFI 2.10 section 14.4.
/// The unused entries are pointer-sized; this protocol is borrowed, never allocated.
#[repr(C)]
#[unsafe_protocol("4cf5b200-68b8-4ca5-9eec-b23e3f50029a")]
struct PciIo {
    unused: [usize; 6],
    read: PciRead,
}

#[repr(transparent)]
#[unsafe_protocol(GraphicsOutputProtocol::GUID)]
struct Gop(GraphicsOutputProtocol);

/// GET_PROTOCOL avoids exclusive opens disconnecting existing console drivers.
/// All callers make synchronous calls and retain no reference after this closure.
pub fn with_protocol<P: ProtocolPointer + ?Sized, R>(
    handle: Handle,
    action: impl FnOnce(&mut P) -> R,
) -> Result<R, Status> {
    // SAFETY: No protocol is uninstalled/reconnected inside a caller's closure.
    // Boot services remain active and all protocol use is synchronous on this CPU.
    let mut protocol = unsafe {
        boot::open_protocol::<P>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .map_err(|error| error.status())?;
    Ok(action(&mut protocol))
}

fn is_display(handle: Handle) -> Result<bool, Status> {
    with_protocol::<PciIo, _>(handle, |pci| {
        let mut class = 0u8;
        // SAFETY: UINT8 width=0, one-byte destination, read-only PCI base-class offset.
        let status = unsafe { (pci.read)(pci, 0, 0x0b, 1, ptr::from_mut(&mut class).cast()) };
        if status == Status::SUCCESS {
            Ok(class == 0x03)
        } else {
            Err(status)
        }
    })?
}

fn identity(handle: Handle) -> Result<u32, Status> {
    with_protocol::<PciIo, _>(handle, |pci| {
        let mut identity = 0u32;
        // SAFETY: UINT32 width=2, aligned four-byte destination, read-only ID register.
        let status = unsafe { (pci.read)(pci, 2, 0, 1, ptr::from_mut(&mut identity).cast()) };
        if status == Status::SUCCESS {
            Ok(identity)
        } else {
            Err(status)
        }
    })?
}

fn record(report: &mut Report, args: core::fmt::Arguments<'_>) {
    log::info!("{args}");
    report.line(args);
}

fn record_pci_path(handle: Handle, report: &mut Report) {
    let result = with_protocol::<DevicePath, _>(handle, |path| {
        for node in path.node_iter() {
            // Log only PCI coordinates, not full device paths or disk serials.
            if let Ok(DevicePathNodeEnum::HardwarePci(pci)) = node.as_enum() {
                record(
                    report,
                    format_args!(
                        "  pci path node: device={:02x} function={:x}",
                        pci.device(),
                        pci.function()
                    ),
                );
            }
        }
    });
    if let Err(status) = result {
        record(report, format_args!("  device path: {status:?}"));
    }
}

/// A bound PCI driver does not imply its graphics child has been connected.
/// Inspect paths first so a retry never targets a controller with a known GOP.
fn has_graphics_child(controller: Handle) -> Result<bool, Status> {
    let parent = with_protocol::<DevicePath, _>(controller, |path| {
        let bytes = path.as_bytes();
        if bytes.len() <= 4 || bytes.len() > 4096 {
            return Err(Status::COMPROMISED_DATA);
        }
        let prefix = bytes
            .strip_suffix(&[0x7f, 0xff, 4, 0])
            .ok_or(Status::COMPROMISED_DATA)?;
        Ok(Vec::from(prefix))
    })??;
    let handles = match boot::locate_handle_buffer(SearchType::ByProtocol(&Gop::GUID)) {
        Ok(handles) => handles,
        Err(error) if error.status() == Status::NOT_FOUND => return Ok(false),
        Err(error) => return Err(error.status()),
    };
    let mut unknown = false;
    for &handle in handles.iter() {
        match with_protocol::<DevicePath, _>(handle, |path| path.as_bytes().starts_with(&parent)) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(_) => unknown = true,
        }
    }
    if unknown {
        Err(Status::UNSUPPORTED)
    } else {
        Ok(false)
    }
}

fn driver_is_bound(handle: Handle) -> Result<bool, Status> {
    let system = uefi::table::system_table_raw().ok_or(Status::NOT_READY)?;
    // SAFETY: Called only during this application's boot-services lifetime.
    let services =
        unsafe { NonNull::new(system.as_ref().boot_services) }.ok_or(Status::NOT_READY)?;
    let mut entries = ptr::null();
    let mut count = 0;
    // SAFETY: Valid handle/GUID and stack output pointers. Firmware owns the
    // returned pool allocation, which is released below with FreePool exactly once.
    let status = unsafe {
        (services.as_ref().open_protocol_information)(
            handle.as_ptr(),
            &PciIo::GUID,
            &mut entries,
            &mut count,
        )
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    let buffer = NonNull::new(entries.cast_mut());
    let bound = if count == 0 {
        Ok(false)
    } else if count > 4096 || buffer.is_none() {
        Err(Status::COMPROMISED_DATA)
    } else {
        // SAFETY: UEFI OpenProtocolInformation guarantees count initialized entries
        // in the returned allocation. The pointer is non-null and count is bounded.
        let entries = unsafe { core::slice::from_raw_parts(entries, count) };
        Ok(entries
            .iter()
            .any(|entry| entry.attributes & (0x08 | 0x10) != 0))
    };
    if let Some(buffer) = buffer {
        // SAFETY: This is the allocation returned by OpenProtocolInformation.
        let _ = unsafe { boot::free_pool(buffer.cast()) };
    }
    bound
}

fn start_if_needed(gop: &mut Gop, report: &mut Report) -> Status {
    let mut size = 0;
    let mut info = ptr::null();
    // SAFETY: Use mode zero without dereferencing Mode (it can be null before
    // initialization). QueryMode receives valid stack output pointers.
    let status = unsafe { (gop.0.query_mode)(&gop.0, 0, &mut size, &mut info) };
    record(report, format_args!("  query mode zero: {status:?}"));
    if status == Status::SUCCESS {
        if let Some(info) = NonNull::new(info.cast_mut()) {
            // SAFETY: Successful QueryMode allocated this pool buffer. We never
            // read it or retain it, and do not change an already working mode.
            let _ = unsafe { boot::free_pool(info.cast()) };
        }
        return Status::SUCCESS;
    }
    if status == Status::NOT_STARTED {
        // SAFETY: Same live GOP interface; the UEFI-defined startup mode is zero.
        let result = unsafe { (gop.0.set_mode)(&mut gop.0, 0) };
        record(
            report,
            format_args!("  start inactive graphics interface: {result:?}"),
        );
        return result;
    }
    status
}

pub fn activate_displays(report: &mut Report) {
    if let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) {
        record(
            report,
            format_args!("pci protocol handles: {}", handles.len()),
        );
        for &handle in handles.iter() {
            match is_display(handle) {
                Ok(true) => {
                    match identity(handle) {
                        Ok(id) => record(
                            report,
                            format_args!(
                                "display: vendor={:04x} device={:04x}",
                                id & 0xffff,
                                id >> 16
                            ),
                        ),
                        Err(status) => record(
                            report,
                            format_args!("display: identity unavailable: {status:?}"),
                        ),
                    }
                    record_pci_path(handle, report);
                    let bound = driver_is_bound(handle);
                    record(report, format_args!("  firmware driver bound: {bound:?}"));
                    let graphics = has_graphics_child(handle);
                    record(
                        report,
                        format_args!("  existing graphics child: {graphics:?}"),
                    );
                    if bound.is_ok() && graphics == Ok(false) {
                        // Recursive ConnectController can start missing children of a
                        // managed controller. It does not call DisconnectController.
                        let result = boot::connect_controller(handle, &[], None, true);
                        record(
                            report,
                            format_args!("  connect missing display interface: {result:?}"),
                        );
                    } else {
                        record(
                            report,
                            format_args!(
                                "  connection skipped: graphics already present or unknown ownership/path"
                            ),
                        );
                    }
                }
                Err(status) => record(report, format_args!("pci class read failed: {status:?}")),
                Ok(false) => {}
            }
        }
    } else {
        record(report, format_args!("no pci protocol handles available"));
    }
    match boot::locate_handle_buffer(SearchType::ByProtocol(&Gop::GUID)) {
        Ok(handles) => {
            record(
                report,
                format_args!(
                    "liminescreen: {} firmware graphics interfaces",
                    handles.len()
                ),
            );
            for &handle in handles.iter() {
                record_pci_path(handle, report);
                let result = with_protocol::<Gop, _>(handle, |gop| start_if_needed(gop, report));
                record(
                    report,
                    format_args!("  graphics startup status: {result:?}"),
                );
            }
        }
        Err(error) => record(
            report,
            format_args!("no firmware graphics interfaces: {:?}", error.status()),
        ),
    }
    // Display errors must not prevent chainloading the normal bootloader.
}

/// Write only the add-on's own rolling report, then close every handle before Limine.
pub fn save_report(text: &str) -> Result<(), Status> {
    let device = with_protocol::<LoadedImage, _>(boot::image_handle(), |image| image.device())?
        .ok_or(Status::NOT_FOUND)?;
    with_protocol::<SimpleFileSystem, _>(device, |filesystem| {
        let mut root = filesystem.open_volume().map_err(|error| error.status())?;
        let mut directory = root
            .open(
                cstr16!("\\EFI\\liminescreen"),
                FileMode::ReadWrite,
                FileAttribute::empty(),
            )
            .map_err(|error| error.status())?
            .into_directory()
            .ok_or(Status::UNSUPPORTED)?;
        match directory.open(
            cstr16!("lastboot.txt"),
            FileMode::ReadWrite,
            FileAttribute::empty(),
        ) {
            Ok(file) => file
                .into_regular_file()
                .ok_or(Status::UNSUPPORTED)?
                .delete()
                .map_err(|error| error.status())?,
            Err(error) if error.status() == Status::NOT_FOUND => {}
            Err(error) => return Err(error.status()),
        }
        let mut file = directory
            .open(
                cstr16!("lastboot.txt"),
                FileMode::CreateReadWrite,
                FileAttribute::empty(),
            )
            .map_err(|error| error.status())?
            .into_regular_file()
            .ok_or(Status::UNSUPPORTED)?;
        file.write(text.as_bytes())
            .map_err(|error| error.status())?;
        file.flush().map_err(|error| error.status())?;
        log::info!("liminescreen: boot report saved");
        Ok(())
    })?
}
