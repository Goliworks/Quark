#!/bin/sh
set -e

TMP_PACKAGE_DIR="tmp_package"
TARGET_PARAM="$1" #arm64 or nothing (default x86_64)
TARGET="x86_64-unknown-linux-gnu"
PACKAGE_SUFFIX="x86_64-linux"
RELEASE_PATH="target/$TARGET/release"
BIN_NAME="quark"
CURRENT_DIR=$(pwd)
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

cd "$SCRIPT_DIR/.." || exit 1

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

# Set the target system from the parameter.
if [ "$TARGET_PARAM" = "arm64" ]; then
  TARGET="aarch64-unknown-linux-gnu"
  PACKAGE_SUFFIX="arm64-linux"
fi

# Build Quark with cargo.
printf "\e[33mBuilding Quark\e[0m\n"
echo "Target: $TARGET"
cargo build --release --target "$TARGET"

if [ $? -eq 0 ]; then
  printf "\e[32mQuark built successfully\e[0m\n"
else
  printf "\e[31mQuark build failed\e[0m\n"
  exit 1
fi

PACKAGE_NAME="$BIN_NAME-$VERSION-$PACKAGE_SUFFIX"
TMP_PACKAGE_PATH="$TMP_PACKAGE_DIR/$PACKAGE_NAME"

# Create a temporary directory for the package.
RELEASE_PATH="target/$TARGET/release"
if [ -f "$RELEASE_PATH/quark" ]; then
  echo "Creating temporary directory $TMP_PACKAGE_DIR"
  mkdir -p "$TMP_PACKAGE_DIR/$PACKAGE_NAME"
  cp "$RELEASE_PATH/$BIN_NAME" "$TMP_PACKAGE_PATH/$BIN_NAME"
else
  echo "Quark binary not found in $PWD/$RELEASE_PATH"
  exit 1
fi

# Create package
printf "\e[33mPackaging Quark\e[0m\n"

cp -r package/* "$TMP_PACKAGE_PATH/"
cp LICENSE "$TMP_PACKAGE_PATH/"

mkdir -p dist

PACKAGE_PATH="dist/$PACKAGE_NAME.tar.gz"

cd "$TMP_PACKAGE_DIR"
tar -czvf "../$PACKAGE_PATH" "$PACKAGE_NAME"
cd ..
rm -rf "$TMP_PACKAGE_DIR"

echo "Delete temporary directory $TMP_PACKAGE_DIR"

printf "\e[32mQuark packaged successfully\e[0m\n"
echo "Package path: $PACKAGE_PATH"

cd "$CURRENT_DIR"
