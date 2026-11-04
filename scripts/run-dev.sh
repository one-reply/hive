#!/bin/bash
# Cargo runner on macOS. An unbundled app's menu bar title is its executable's file name, so
# `cargo run -p hive` launches a hard link named `Hive` (in a subdirectory: the file system is
# case-insensitive). Every other binary, tests included, runs as is.
set -euo pipefail

executable="$1"
shift
if [[ "$(basename "$executable")" != hive ]]; then
    exec "$executable" "$@"
fi
dev_dir="$(dirname "$executable")/dev"
mkdir -p "$dev_dir"
ln -f "$executable" "$dev_dir/Hive"
exec "$dev_dir/Hive" "$@"
