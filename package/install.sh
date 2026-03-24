#!/bin/sh
set -e

QUARK_USER="quark"
QUARK_BIN="quark"
QUARK_DEFAULT_UID=635
MAX_UID=999
CONFIG_PATH="/etc/quark"
CONFIG_FILE="config.toml"
CONFIG_FILE_EXAMPLE="config.example.toml"
NOSTART_PARAM="$1" #no-start or nothing;
YN_ERROR_MSG="\e[33mPlease answer yes(y) or no(n).\e[0m"
UPDATING=false
CURRENT_DIR=$(pwd)
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PLATFORM=$(uname -s | tr '[:upper:]' '[:lower:]')

# Platform-specific variables
if [ "$PLATFORM" = "freebsd" ]; then
  ROOT_GROUP="wheel"
  SERVICE_FILE="quark"
  SERVICE_DESTINATION="/usr/local/etc/rc.d"
  QUARK_BIN_DESTINATION="/usr/local/sbin"
  SOCKET_PATH="/var/run/quark"
else
  ROOT_GROUP="root"
  SERVICE_FILE="quark.service"
  SERVICE_DESTINATION="/etc/systemd/system"
  QUARK_BIN_DESTINATION="/usr/sbin"
  SOCKET_PATH="/run/quark"
fi

cd "$SCRIPT_DIR" || exit 1

printf "\e[33mInstalling Quark\e[0m\n"

# Get free UID (default is 245 for quark user).
quark_uid=$QUARK_DEFAULT_UID
while [ $quark_uid -le $MAX_UID ]; do
  if getent passwd $quark_uid >/dev/null; then
    quark_uid=$((quark_uid + 1))
  else
    break
  fi
done

if [ $quark_uid -gt $MAX_UID ]; then
  printf "\e[31mNo free UID available between $QUARK_DEFAULT_UID and $MAX_UID\e[0m\n"
  exit 1
fi

# Get nologin shell for system user.
for shell in /usr/sbin/nologin /sbin/nologin /bin/false; do
  if [ -x "$shell" ]; then
    NOLOGIN_SHELL="$shell"
    break
  fi
done

[ -n "$NOLOGIN_SHELL" ] || {
  printf "\e[31mUnable to find nologin shell\e[0m\n" >&2
  exit 1
}

# Create user.
if ! id "$QUARK_USER" >/dev/null 2>&1; then
  if [ "$PLATFORM" = "freebsd" ]; then
    pw useradd "$QUARK_USER" -u "$quark_uid" -s "$NOLOGIN_SHELL" -d /nonexistent
  else
    useradd -r -s "$NOLOGIN_SHELL" -u "$quark_uid" "$QUARK_USER"
  fi
  echo "User $QUARK_USER created with UID $quark_uid"
  echo "Using nologin shell : $NOLOGIN_SHELL"
fi

# Create socket directory.
if [ ! -d "$SOCKET_PATH" ]; then
  mkdir -p "$SOCKET_PATH"
  chown "$QUARK_USER":"$QUARK_USER" "$SOCKET_PATH"
  echo "Directory $SOCKET_PATH created"
fi

# Check a quark service is already active.
is_quark_running() {
  if [ "$PLATFORM" = "freebsd" ]; then
    service quark status 2>/dev/null | grep -q "is running"
  else
    systemctl is-active --quiet quark
  fi
}

if is_quark_running; then
  echo "\e[33mThe Quark service is already running.\e[0m"
  echo "To proceed with the installation, this script will temporarily stop the service."
  echo "Once the setup is complete, the service will be restarted automatically."
  echo "Please note: your server will only be unavailable for a few seconds."
  while true; do
    read -p "Do you agree to continue with this process? (y/n) : " yn
    case $yn in
    [Yy]*)
      echo "Stopping the Quark service"
      if [ "$PLATFORM" = "freebsd" ]; then
        service quark stop
      else
        systemctl stop quark
      fi
      # check if the service is stopped before continuing
      while is_quark_running; do
        sleep 1
      done
      echo "The Quark service has been stopped"
      break
      ;;
    [Nn]*)
      printf "\e[31mQuark installation aborted\n"
      exit 1
      ;;
    *) printf "$YN_ERROR_MSG\n" ;;
    esac
  done
fi

# Copy the binary to the destination
if [ -f "$QUARK_BIN" ]; then
  if [ -f "$QUARK_BIN_DESTINATION/$QUARK_BIN" ]; then
    UPDATING=true
  fi

  cp "$QUARK_BIN" "$QUARK_BIN_DESTINATION/"
  chown root:$ROOT_GROUP "$QUARK_BIN_DESTINATION/$QUARK_BIN"

  if $UPDATING; then
    printf "\e[33m'$QUARK_BIN' bin has replaced the previous one in $QUARK_BIN_DESTINATION.\e[0m\n"
  else
    echo "File $QUARK_BIN copied"
  fi
  chmod 755 "$QUARK_BIN_DESTINATION/$QUARK_BIN"
else
  printf "\e[31mError : $QUARK_BIN binary not found in $PWD\e[0m\n"
  exit 1
fi

# Create configuration directory.
if [ ! -d "$CONFIG_PATH" ]; then
  mkdir -p "$CONFIG_PATH"
  chown root:$ROOT_GROUP "$CONFIG_PATH"
fi

# Create default configuration file.
if [ ! -f "$CONFIG_PATH/$CONFIG_FILE" ]; then
  touch "$CONFIG_PATH/$CONFIG_FILE"
  echo "# Configuration file for Quark" >"$CONFIG_PATH/$CONFIG_FILE"
  chown root:$ROOT_GROUP "$CONFIG_PATH/$CONFIG_FILE"
  chmod 600 "$CONFIG_PATH/$CONFIG_FILE"
  echo "Configuration file created"
fi

# Example config file
if [ -f "$CONFIG_FILE_EXAMPLE" ]; then
  cp "$CONFIG_FILE_EXAMPLE" "$CONFIG_PATH/"
  chown root:$ROOT_GROUP "$CONFIG_PATH/$CONFIG_FILE_EXAMPLE"
  chmod 600 "$CONFIG_PATH/$CONFIG_FILE_EXAMPLE"
  echo "Example configuration file created"
fi

# Create systemd service
cp "service/$SERVICE_FILE" "$SERVICE_DESTINATION/"

if [ "$PLATFORM" = "freebsd" ]; then
  chmod 755 "$SERVICE_DESTINATION/$SERVICE_FILE"
  chown root:wheel "$SERVICE_DESTINATION/$SERVICE_FILE"
  sysrc quark_enable="YES"
  if [ "$NOSTART_PARAM" != "no-start" ]; then
    echo "Starting the Quark service"
    service quark start
    echo "The Quark service has been started"
  fi
else
  chown root:root "$SERVICE_DESTINATION/$SERVICE_FILE"
  chmod 644 "$SERVICE_DESTINATION/$SERVICE_FILE"
  systemctl daemon-reload
  systemctl enable quark
  if [ "$NOSTART_PARAM" != "no-start" ]; then
    echo "Starting the Quark service"
    systemctl restart quark
    echo "The Quark service has been started"
  fi
fi

# Finish
if $UPDATING; then
  printf "\e[32mQuark has been updated.\e[0m\n"
else
  printf "\e[32mQuark has been installed.\e[0m\n"
fi

cd "$CURRENT_DIR"
