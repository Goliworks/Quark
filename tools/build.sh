#!/bin/bash
set -e

TMP_PACKAGE_DIR="tmp_package"
RELEASE_PATH="target/release/quark"

echo "Building Quark"
cargo build --release

if [ $? -eq 0 ]; then
  echo "Quark built successfully"
else
  echo "Quark build failed"
  exit 1
fi

if [ -f "$RELEASE_PATH" ]; then
  mkdir -p "$TMP_PACKAGE_DIR"
  cp "$RELEASE_PATH" "$TMP_PACKAGE_DIR/quark"
else
  echo "Quark binary not found in $PWD"
  exit 1
fi

echo "Packaging Quark"

# Get version from Cargo.toml
VERSION=$(awk '
  /^\[package\]/ { in_package = 1; next }
  /^\[/ { in_package = 0 }
  in_package && /^version[[:space:]]*=/ {
    match($0, /"[^\"]+"/)
    print substr($0, RSTART+1, RLENGTH-2)
    exit
  }
' Cargo.toml)

# Create package
cp -r package/* "$TMP_PACKAGE_DIR/"

mkdir -p dist

tar -czvf "dist/quark-$VERSION.tar.gz" -C "$TMP_PACKAGE_DIR" .
rm -rf "$TMP_PACKAGE_DIR"

echo "Quark packaged successfully"
