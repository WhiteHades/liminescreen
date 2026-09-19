//! The firmware boundary. Protocol guards are dropped before connecting drivers
//! or starting another image. No framebuffer writes, PCI writes, or NVRAM writes.

use core::ffi::c_void;
use core::ptr::{self, NonNull};
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::proto::{ProtocolPointer, unsafe_protocol};
use uefi::{Handle, Identify, Status};
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
        status == Status::SUCCESS && class == 0x03
    })
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

fn start_if_needed(gop: &mut Gop) -> Status {
    let mut size = 0;
    let mut info = ptr::null();
    // SAFETY: Use mode zero without dereferencing Mode (it can be null before
    // initialization). QueryMode receives valid stack output pointers.
    let status = unsafe { (gop.0.query_mode)(&gop.0, 0, &mut size, &mut info) };
    if status == Status::SUCCESS {
        if let Some(info) = NonNull::new(info.cast_mut()) {
            // SAFETY: Successful QueryMode allocated this pool buffer. We never
            // read it or retain it, and do not change an already working mode.
            let _ = unsafe { boot::free_pool(info.cast()) };
        }
        return Status::SUCCESS;
    }
    if status == Status::NOT_STARTED {
        log::info!("liminescreen: starting inactive graphics interface in mode zero");
        // SAFETY: Same live GOP interface; the UEFI-defined startup mode is zero.
        return unsafe { (gop.0.set_mode)(&mut gop.0, 0) };
    }
    status
}

pub fn activate_displays() {
    if let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&PciIo::GUID)) {
        for &handle in handles.iter() {
            if is_display(handle) == Ok(true) && driver_is_bound(handle) == Ok(false) {
                let result = boot::connect_controller(handle, &[], None, true);
                log::info!("liminescreen: connect unclaimed display: {result:?}");
            }
        }
    }
    if let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&Gop::GUID)) {
        log::info!(
            "liminescreen: {} firmware graphics interfaces",
            handles.len()
        );
        for &handle in handles.iter() {
            let result = with_protocol::<Gop, _>(handle, start_if_needed);
            log::info!("liminescreen: graphics status: {result:?}");
        }
    }
    // Display errors must not prevent chainloading the normal bootloader.
}
