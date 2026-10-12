#!/bin/sh
# Check the tools before the build, then run one Cargo command.
set -eu
version=$1
shift
fail() { echo "wasm32: $*" >&2; exit 1; }

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
    fail 'the rustup target wasm32-unknown-unknown is missing; run rustup target add wasm32-unknown-unknown'
fi
runner_version=$(wasm-bindgen-test-runner --version 2>/dev/null) ||
    fail "wasm-bindgen-test-runner is missing; run cargo install wasm-bindgen-cli --version $version --locked"
if [ "$runner_version" != "wasm-bindgen-test-runner $version" ]; then
    fail "wasm-bindgen-test-runner must match wasm-bindgen $version in Cargo.lock; run cargo install wasm-bindgen-cli --version $version --locked --force"
fi
cc=$(command -v clang) || fail 'clang is missing; install clang with the WebAssembly back end'
if ! "$cc" --print-targets 2>/dev/null | grep -q wasm32; then
    fail "clang at $cc has no WebAssembly back end; install clang with the wasm32 target"
fi
ar=$(command -v llvm-ar || true)
if [ -z "$ar" ]; then
    sysroot=$(rustc --print sysroot) || fail 'llvm-ar is missing; run rustup component add llvm-tools'
    host=$(rustc -vV | sed -n 's/^host: //p')
    ar="$sysroot/lib/rustlib/$host/bin/llvm-ar"
fi
if ! "$ar" --version >/dev/null 2>&1; then
    fail 'llvm-ar is missing; install llvm-ar or run rustup component add llvm-tools'
fi
node --version >/dev/null 2>&1 || fail 'Node.js is missing; install the version in .nvmrc'

export CC_wasm32_unknown_unknown="$cc"
export AR_wasm32_unknown_unknown="$ar"
# The runner in .cargo/config.toml uses Node when no test mode is set.
unset WASM_BINDGEN_USE_DENO WASM_BINDGEN_USE_BROWSER \
    WASM_BINDGEN_USE_DEDICATED_WORKER WASM_BINDGEN_USE_SHARED_WORKER \
    WASM_BINDGEN_USE_SERVICE_WORKER WASM_BINDGEN_USE_NODE_EXPERIMENTAL \
    WASM_BINDGEN_TEST_ONLY_WEB
exec "$@"
