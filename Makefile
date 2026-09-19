.PHONY: build check vmcheck setup enable disable repair

build:
	cargo build --locked --release -p liminescreen --target x86_64-unknown-uefi
	cargo build --locked --release -p liminescreenctl

check:
	cargo fmt --all --check
	cargo clippy --locked -p liminescreen --target x86_64-unknown-uefi --bin liminescreen -- -D warnings
	cargo clippy --locked -p liminescreenctl --all-targets -- -D warnings
	cargo test --locked -p liminescreenctl

vmcheck:
	cargo test --locked -p liminescreenctl --test uefi -- --ignored --nocapture

setup: build
	sudo target/release/liminescreenctl setup --image "$(CURDIR)/target/x86_64-unknown-uefi/release/liminescreen.efi"

enable:
	sudo /usr/local/bin/liminescreenctl enable

disable:
	sudo /usr/local/bin/liminescreenctl disable

repair:
	sudo /usr/local/bin/liminescreenctl repair
