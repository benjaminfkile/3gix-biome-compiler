#!/bin/sh
# Builds the biome module: the library's cdylib for
# wasm32-unknown-unknown, copied to target/module/biome.wasm, and prints
# its size and SHA-256 (the module hash the hub stores it under,
# client-compute.md section 2.1).
set -eu

cd "$(dirname "$0")/.."

# --compress-relocations makes the linker write every call_indirect table
# index as the single zero byte WebAssembly 1.0 requires; padded, it is a
# reference-types encoding the core library's module host refuses ("zero
# byte expected").
cargo rustc -q --locked --release -p biome-module --lib --crate-type cdylib \
    --target wasm32-unknown-unknown -- -C link-arg=--compress-relocations
mkdir -p target/module
cp target/wasm32-unknown-unknown/release/biome_module.wasm target/module/biome.wasm
size=$(wc -c < target/module/biome.wasm | tr -d ' ')
hash=$(sha256sum target/module/biome.wasm | cut -d ' ' -f 1)
echo "target/module/biome.wasm $size bytes sha256 $hash"
