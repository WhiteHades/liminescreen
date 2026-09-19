# setup and recovery

## requirements

the firmware app supports x86_64 uefi. the installer runs on linux and expects
an esp mounted at `/boot`, with the official loader at
`/boot/EFI/limine/limine_x64.efi`. it uses `efibootmgr`, `findmnt`, and `lsblk`.
the rust toolchain file requests the uefi target. build dependencies are locked.

on omarchy, install missing tools with the official package command. use a
rustup installation of rust so the uefi target is available. vm tests also need
`qemu-system-x86` and `edk2-ovmf`.

## installation

run `make setup` from this repository. an administrative password is needed
when the installer writes the esp and creates its firmware entry.

setup adds these files:

| file | purpose |
| --- | --- |
| `/boot/EFI/liminescreen/liminescreen.efi` | the separate startup app |
| `/usr/local/bin/liminescreenctl` | the native rust installer and recovery tool |
| `/var/lib/liminescreen/state.json` | private installation state |
| `/var/lib/liminescreen/backup-*` | firmware records and previous addon versions |
| `/etc/pacman.d/hooks/99-liminescreen.hook` | checks after limine or omarchy updates |

the installer creates a firmware entry called `liminescreen`. it keeps the
normal boot order and schedules only one trial using `BootNext`. an existing
one shot request for another system is left intact. setup does not reboot.

the official limine binary and its menu remain where they were. their own
updaters keep control of them. the installer checks that the limine binary's
checksum is unchanged after installation.

## the physical test

connect hdmi or displayport and turn on the monitor before powering on.
check the external menu, your linux and windows entries, startup without the
external monitor, and the laptop lid both open and closed.

when those checks pass, run `make enable`. this places the addon first in the
normal boot order while keeping every other entry in its previous order.
package updates restore this ordering only while the addon is enabled. use
`make disable` if you want a manual firmware preference to remain in charge.

## normal updates

each boot loads the current official limine image from the same esp. updating
limine needs no addon rebuild. the package hook verifies the addon checksum,
firmware entry, and official target. it reports missing or changed components
instead of guessing a replacement path.

to update liminescreen itself, review and pull this repository, run `make check`
and `make vmcheck`, then run `make setup`. it reuses the firmware entry and saves
the previous addon image and state before replacement. it does not fetch or
rebuild code automatically as root.

## recovery

run `make disable`, or from any linux terminal:

```sh
sudo /usr/local/bin/liminescreenctl disable
```

this removes the addon from normal ordering and clears only its own pending
trial. installed files and recovery records remain available. the original
limine entry can always be selected directly in firmware setup while it remains
installed.

after a trial, firmware should have consumed `BootNext`, so the following boot
uses the original ordering. a firmware hang may require powering off and then
selecting the ordinary limine entry. the saved `firmware.txt` records contain
the previous ordering and entry paths.

secure boot uses the normal firmware signature policy. this project neither
disables that policy nor bypasses signature checks. sign the addon with a
trusted key if your machine requires it.

firmware updates can change graphics behavior. a future distribution change
that moves the official limine path also needs review. those cases are not
covered by a promise of compatibility with every future version.
