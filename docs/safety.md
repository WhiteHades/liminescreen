# how it works

the firmware app connects only pci display controllers without a bound driver.
it then asks graphics interfaces whether they are running. an interface that
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
the app does not dereference an uninitialized graphics mode pointer.

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

`make vmcheck` uses qemu and ovmf with temporary virtual disks. it checks:

1. the addon loads the downstream boot image.
2. replacing that image works without rebuilding the addon.
3. multiple graphics interfaces remain usable.
4. a system without a graphics interface still reaches the downstream image.
5. a missing downstream image returns an error.

to also inspect the unchanged official limine menu:

```sh
LIMINE_EFI=/usr/share/limine/BOOTX64.EFI make vmcheck
```

this leaves a screenshot at `.tmp/official-menu.png` for inspection. other vm
files are removed after the run. test instrumentation is built in a separate
temporary target directory and is absent from the production image.

physical hdmi initialization remains a hardware check. successful vm tests do
not establish that a particular laptop exposes an external firmware driver.
