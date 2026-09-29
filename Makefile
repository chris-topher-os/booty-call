.PHONY: build build-control agent-linux agent-windows clean

build:
	cargo build --release

build-control:
	cargo build --release -p control

# Debian target box (static, no glibc dependency).
# Requires: rustup target add x86_64-unknown-linux-musl
#           linker x86_64-linux-gnu-gcc (see ~/.cargo/config.toml notes in README)
agent-linux:
	cargo build --release -p agent --target x86_64-unknown-linux-musl

# Windows gaming partition.
# Requires: rustup target add x86_64-pc-windows-gnu + mingw (gcc-mingw-w64-x86-64)
agent-windows:
	cargo build --release -p agent --target x86_64-pc-windows-gnu

clean:
	cargo clean
