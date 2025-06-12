#!/bin/bash
set -e

QUARK_USER="quark"
QUARK_BIN="quark"
QUARK_DEFAULT_UID=635
MAX_UID=999
QUARK_BIN_DESTINATION="/usr/sbin"
SOCKET_PATH="/run/quark"
CONFIG_PATH="/etc/quark"
CONFIG_FILE="config.toml"
CONFIG_FILE_EXAMPLE="config.example.toml"
SERVICE_FILE="quark.service"
SERVICE_DESTINATION="/etc/systemd/system"
NOSTART_PARAM="$1" #no-start or nothing;
UPDATING=false

echo "Installing Quark"

# Get free UID (default is 245 for quark user).
quark_uid=$QUARK_DEFAULT_UID
while [ $quark_uid -le $MAX_UID ]; do
  if getent passwd $quark_uid >/dev/null; then
    quark_uid=$((quark_uid + 1))
  else
    echo "Using quark UID : $quark_uid"
    break
  fi
done

if [ $quark_uid -gt $MAX_UID ]; then
  echo "No free UID available between $QUARK_DEFAULT_UID and $MAX_UID"
  exit 1
fi

# Get nologin shell for system user.
for shell in /usr/sbin/nologin /sbin/nologin /bin/false; do
  if [ -x "$shell" ]; then
    NOLOGIN_SHELL="$shell"
    echo "Using nologin shell : $NOLOGIN_SHELL"
    break
  fi
done

[ -n "$NOLOGIN_SHELL" ] || {
  echo "Unable to find nologin shell" >&2
  exit 1
}

# Create user.
if ! id "$QUARK_USER" >/dev/null 2>&1; then
  useradd -r -s "$NOLOGIN_SHELL" -u "$quark_uid" "$QUARK_USER"
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
else
  echo "Error : $QUARK_BIN binary not found in $PWD"
  exit 1
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
  chmod 600 "$CONFIG_PATH/$CONFIG_FILE"
  echo "Configuration file created"
fi

# Example config file
if [ -f "$CONFIG_FILE_EXAMPLE" ]; then
  cp "$CONFIG_FILE_EXAMPLE" "$CONFIG_PATH/"
  chown root:root "$CONFIG_PATH/$CONFIG_FILE_EXAMPLE"
  chmod 600 "$CONFIG_PATH/$CONFIG_FILE_EXAMPLE"
  echo "Example configuration file created"
fi

# Create systemd service
cp "$SERVICE_FILE" "$SERVICE_DESTINATION/"
chown root:root "$SERVICE_DESTINATION/$SERVICE_FILE"
chmod 644 "$SERVICE_DESTINATION/$SERVICE_FILE"
systemctl daemon-reload
systemctl enable quark
if [ "$NOSTART_PARAM" != "no-start" ]; then
  systemctl restart quark
fi

# Finish
if $UPDATING; then
  echo "Quark has been updated"
else
  echo "Quark has been installed"
fi
