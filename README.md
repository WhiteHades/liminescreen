<h1 align="center">liminescreen</h1>

<p align="center">a small startup helper for external monitor boot menus</p>

<div align="center">
<pre>
       __________________________
      |                          |
      |          limine          |
      |     linux    windows     |
      |__________________________|
                |    |
             ___|____|___
</pre>
</div>

<p align="center">
  <kbd>rust</kbd> <kbd>uefi</kbd> <kbd>limine</kbd> <kbd>omarchy</kbd>
  <kbd>hdmi</kbd> <kbd>displayport</kbd> <kbd>dual boot</kbd>
</p>

<p align="center">
  <img src="https://img.shields.io/static/v1?label=tests&amp;message=local&amp;color=green" alt="local tests">
  <a href="https://github.com/whitehades/liminescreen/stargazers"><img src="https://img.shields.io/github/stars/whitehades/liminescreen?label=stars" alt="github stars"></a>
</p>

## what it does

want to choose linux or windows on your desk monitor without opening the laptop?

liminescreen asks your firmware to start its available displays, then opens the
limine already on your computer. your menu, themes, boot entries, and normal
limine updates stay with the official bootloader.

it is a separate addon, written in rust. there is no python runtime and no
custom limine fork to maintain.

## start here

this currently targets omarchy and similar x86_64 uefi systems. you need rust
through rustup, make, and the usual linux boot tools. see the [setup guide](docs/setup.md)
if those are missing or your setup is different.

```sh
git clone https://github.com/whitehades/liminescreen
cd liminescreen
make setup
```

setup builds the addon, saves recovery records, and selects it for one next
boot. it does not restart your computer or change your normal boot order.

save your work, connect the monitor, and restart. check that you can see the
boot menu and choose your usual system.

if that works, use it on future boots:

```sh
make enable
```

to return to normal limine startup:

```sh
make disable
```

## will it work on my laptop?

your firmware must provide a graphics driver for the external screen before
linux starts. liminescreen can start an available driver. it cannot add one
that your firmware does not have.

the first laptop test reached limine, but the external monitor still had no
signal. version 0.1.1 retries one initialization case that the first version
skipped and saves a boot report. it is another trial, not a verified hdmi fix.

the virtual machine checks pass, including loading the official limine menu.
this project is not an official limine or omarchy component.

## updates and recovery

the addon opens the current limine file each time. it does not keep an old copy
of your bootloader or menu. a package update hook checks the setup and keeps an
enabled addon selected. your original boot entry remains available.

read [setup and recovery](docs/setup.md) for the file paths and recovery steps.
read [how it works](docs/safety.md) for memory safety, firmware limits, and tests.
