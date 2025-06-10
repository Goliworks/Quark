#!/bin/bash
set -e

TMP_PACKAGE_DIR="tmp_package"
TARGET_PARAM="$1" #arm64 or nothing (default x86_64)
TARGET="x86_64-unknown-linux-gnu"
PACKAGE_SUFFIX="x86_64-linux"
RELEASE_PATH="target/$TARGET/release"
BIN_NAME="quark"

if [ "$TARGET_PARAM" == "arm64" ]; then
  TARGET="aarch64-unknown-linux-gnu"
  PACKAGE_SUFFIX="arm64-linux"
fi

echo "Building Quark"
echo "Target: $TARGET"
cargo build --release --target "$TARGET"

if [ $? -eq 0 ]; then
  echo "Quark built successfully"
else
  echo "Quark build failed"
  exit 1
fi

RELEASE_PATH="target/$TARGET/release"
if [ -f "$RELEASE_PATH/quark" ]; then
  mkdir -p "$TMP_PACKAGE_DIR"
  cp "$RELEASE_PATH/$BIN_NAME" "$TMP_PACKAGE_DIR/$BIN_NAME"
else
  echo "Quark binary not found in $PWD/$RELEASE_PATH"
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

PACKAGE_PATH="dist/$BIN_NAME-$VERSION-$PACKAGE_SUFFIX.tar.gz"

tar -czvf "$PACKAGE_PATH" -C "$TMP_PACKAGE_DIR" .
rm -rf "$TMP_PACKAGE_DIR"

echo "Quark packaged successfully"
echo "Package path: $PACKAGE_PATH"
