#!/bin/bash

QUARK_USER="quark"
QUARK_BIN="quark"
QUARK_BIN_DESTINATION="/usr/sbin"
SOCKET_PATH="/run/quark"
CONFIG_PATH="/etc/quark"
CONFIG_FILE="config.toml"
CONFIG_FILE_EXAMPLE="example.config.toml"
UPDATING=false

echo "Installing Quark"

# Create user.
if ! id "$QUARK_USER" >/dev/null 2>&1; then
  useradd -r -s /usr/sbin/nologin "$QUARK_USER"
  echo "User $QUARK_USER created"
fi

# Create socket directory.
if [ ! -d "$SOCKET_PATH" ]; then
  mkdir -p "$SOCKET_PATH"
  chown "$QUARK_USER":"$QUARK_USER" "$SOCKET_PATH"
  echo "Directory $SOCKET_PATH created"
fi

# Copy the binary to the destination
if [ -f "$QUARK_BIN" ]; then
  if [ -f "$QUARK_BIN_DESTINATION/$QUARK_BIN" ]; then
    UPDATING=true
  fi

  cp "$QUARK_BIN" "$QUARK_BIN_DESTINATION/"
  chown root:root "$QUARK_BIN_DESTINATION/$QUARK_BIN"

  if $UPDATING; then
    echo "'$QUARK_BIN' bin has replaced the previous one in $QUARK_BIN_DESTINATION"
  else
    echo "File $QUARK_BIN copied"
  fi
  chmod 755 "$QUARK_BIN_DESTINATION/$QUARK_BIN"
fi

# Create configuration directory.
if [ ! -d "$CONFIG_PATH" ]; then
  mkdir -p "$CONFIG_PATH"
  chown root:root "$CONFIG_PATH"
fi

# Create default configuration file.
if [ ! -f "$CONFIG_PATH/$CONFIG_FILE" ]; then
  touch "$CONFIG_PATH/$CONFIG_FILE"
  echo "# Configuration file for Quark" >"$CONFIG_PATH/$CONFIG_FILE"
  chown root:root "$CONFIG_PATH/$CONFIG_FILE"
  cmod 600 "$CONFIG_PATH/$CONFIG_FILE"
  echo "Configuration file created"
fi

# Example config file
if [ -f "$CONFIG_FILE_EXAMPLE" ]; then
  cp "$CONFIG_FILE_EXAMPLE" "$CONFIG_PATH/"
  chown root:root "$CONFIG_PATH/$CONFIG_FILE_EXAMPLE"
  cmod 600 "$CONFIG_PATH/$CONFIG_FILE_EXAMPLE"
  echo "Example configuration file created"
fi

# Finish
if $UPDATING; then
  echo "Quark has been updated"
else
  echo "Quark has been installed"
fi
