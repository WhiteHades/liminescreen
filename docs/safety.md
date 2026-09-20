# how it works

the firmware app recursively connects every pci display controller using
UEFI `ConnectController`. this preserves bound drivers while asking them to
create any missing children. an existing graphics interface does not prove
that every output is initialized, and a graphics interface without a device
path must not prevent another gpu from being connected. it then asks
graphics interfaces whether they are running. an interface that
returns `EFI_NOT_STARTED` is started in mode zero. already active graphics modes
are left alone. display errors do not stop the attempt to load official limine.

the target is loaded from the same esp through the standard uefi image loader.
there is no copied menu, cached limine image, kernel change, framebuffer write,
pci configuration write, or direct firmware variable write in the startup app.
if limine cannot be loaded, the app returns the error to firmware.

## memory safety

both the startup app and installer are rust. the installer forbids unsafe code.
the startup app uses the typed `uefi` and `uefi-raw` crates. unavoidable unsafe
firmware calls are isolated in `firmware/src/firmware.rs` with their contracts.

protocol access is temporary and nonexclusive, so opening an interface does
not disconnect an existing console driver. protocol guards are dropped before
connecting controllers or starting another image. pci inspection reads one byte
for the device class. returned ownership counts and pointers are checked before
reading the firmware allocation, and returned pool buffers are freed.

the pci protocol declaration is a layout prefix through `Pci.Read`, matching
the uefi specification. the graphics protocol layout comes from `uefi-raw`.
the app does not dereference an uninitialized graphics mode pointer. a bounded
boot report records display identifiers, pci coordinates, driver ownership,
graphics children, and startup results. only the addon's own `lastboot.txt`
is replaced. report storage uses typed file APIs with no new unsafe blocks.
the report is limited to 16 kib and does not include disk serial numbers.
failure to save it does not prevent loading limine.

firmware must still honor its own pointer, lifetime, and allocation contracts.
rust cannot make a third party firmware driver memory safe or prevent a
firmware call from hanging.

the native installer checks the image format, architecture, and uefi subsystem.
it rejects ambiguous boot entries and unowned destination files. writes use
exclusive temporary files, syncing, and atomic replacement. a shared boot
partition lock avoids racing limine's own updater. installation records and
backups stay outside the public repository.

## verification

`make check` runs formatting, clippy, and native tests for image validation,
entry detection, duplicate handling, and ordering that preserves other systems.

`make vmcheck` uses qemu, ovmf, and mtools with temporary FAT disk images. it checks:

1. the addon loads the downstream boot image.
2. replacing that image works without rebuilding the addon.
3. multiple graphics interfaces remain usable.
4. a system without a graphics interface still reaches the downstream image.
5. a missing downstream image returns an error.
6. the boot report persists, grows, and is replaced correctly by a shorter report.
7. a report write failure still reaches the downstream bootloader.

to also inspect the unchanged official limine menu:

```sh
LIMINE_EFI=/usr/share/limine/BOOTX64.EFI make vmcheck
```

this leaves a screenshot at `.tmp/official-menu.png` for inspection. other vm
files are removed after the run. test instrumentation is built in a separate
temporary target directory and is absent from the production image.

the initial 0.1.0 trial on the development laptop ran successfully as a loader
but did not produce an hdmi signal. a later report showed that a pathless
graphics interface caused the helper to skip the nvidia controller, which owns
the hdmi connector on that machine. the current build removes that skip. vm
tests verify that every display controller gets a connection attempt and
chainloading still works. the next physical test still produced no hdmi signal.

the report dated 2026-09-20 09:04:23 records `NOT_FOUND` for the nvidia display
controller and success for amd. two graphics interfaces were listed: one had
no device path, and the identifiable one followed the amd pci path. this does
not establish the identity of the pathless interface or prove that every
possible nvidia firmware driver is absent. it confirms that this connection
attempt did not solve hdmi output. no identical retry is scheduled automatically.

UEFI describes recursive connection and starting additional children in the
[boot services specification](https://uefi.org/specs/UEFI/2.10_A/07_Services_Boot_Services.html#efi-boot-services-connectcontroller).
