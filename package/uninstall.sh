#!/bin/sh
set -e

PLATFORM=$(uname -s | tr '[:upper:]' '[:lower:]')
YN_ERROR_MSG="\e[33mPlease answer yes(y) or no(n).\e[0m"
SERVICE_NAME='quark'
CONFIG_PATH="/etc/$SERVICE_NAME"
LOG_PATH="/var/log/$SERVICE_NAME"
CURRENT_DIR=$(pwd)
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

if [ "$PLATFORM" = "freebsd" ]; then
  BINARY_PATH="/usr/local/sbin/$SERVICE_NAME"
  SERVICE_FILE="/usr/local/etc/rc.d/$SERVICE_NAME"
  SOCKET_PATH="/var/run/$SERVICE_NAME"
else
  BINARY_PATH="/usr/sbin/$SERVICE_NAME"
  SERVICE_FILE="/etc/systemd/system/$SERVICE_NAME.service"
  SOCKET_PATH="/run/$SERVICE_NAME"
fi

cd "$SCRIPT_DIR" || exit 1

printf "\e[33mUninstalling Quark\e[0m\n"

# Remove systemd service
if [ "$PLATFORM" = "freebsd" ]; then
  if service "$SERVICE_NAME" status 2>/dev/null | grep -q "is running"; then
    service "$SERVICE_NAME" stop
  fi
  if [ -f "$SERVICE_FILE" ]; then
    sysrc -x "${SERVICE_NAME}_enable"
    rm "$SERVICE_FILE"
  fi
else
  if systemctl status "$SERVICE_NAME.service" >/dev/null 2>&1; then
    systemctl stop "$SERVICE_NAME"
    systemctl disable "$SERVICE_NAME"
    rm "$SERVICE_FILE"
    systemctl daemon-reload
    systemctl reset-failed "$SERVICE_NAME"
  fi
fi

# Remove binary
if [ -f "$BINARY_PATH" ]; then
  echo "Removing the binary $BINARY_PATH"
  rm "$BINARY_PATH"
fi

# Remove configuration
while true; do
  read -p "Do you want to remove the configuration directory? (y/n) : " yn
  case $yn in
  [Yy]*)
    if [ -d "$CONFIG_PATH" ]; then
      echo "Removing configuration $CONFIG_PATH"
      rm -rf "$CONFIG_PATH"
    else
      echo "$CONFIG_PATH has already been removed"
    fi
    break
    ;;
  [Nn]*) break ;;
  *) printf "$YN_ERROR_MSG\n" ;;
  esac
done

# Remove logs directory
while true; do
  read -p "Do you want to remove the log directory? (y/n) : " yn
  case $yn in
  [Yy]*)
    if [ -d "$LOG_PATH" ]; then
      echo "Removing logs $LOG_PATH"
      rm -rf "$LOG_PATH"
    else
      echo "$LOG_PATH has already been removed"
    fi
    break
    ;;
  [Nn]*) break ;;
  *) printf "$YN_ERROR_MSG\n" ;;
  esac
done

# Remove quark user
while true; do
  read -p "Do you want to delete the user 'quark'? (y/n) : " yn
  case $yn in
  [Yy]*)
    if grep -q "^$SERVICE_NAME:" /etc/passwd; then
      echo "Deleting user $SERVICE_NAME"
      if [ "$PLATFORM" = "freebsd" ]; then
        pw userdel "$SERVICE_NAME"
      else
        userdel -r "$SERVICE_NAME"
      fi
    else
      echo "The user $SERVICE_NAME has already been deleted"
    fi
    break
    ;;
  [Nn]*) break ;;
  *) printf "$YN_ERROR_MSG\n" ;;
  esac
done

# Remove socket
if [ -d "$SOCKET_PATH" ]; then
  echo "Removing socket directory $SOCKET_PATH"
  rm -rf "$SOCKET_PATH"
fi

printf "\e[32mQuark has been uninstalled\e[0m\n"

cd "$CURRENT_DIR"
