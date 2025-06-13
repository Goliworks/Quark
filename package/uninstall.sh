#!/bin/sh
set -e

YN_ERROR_MSG="Please answer yes(y) or no(n)."
SERVICE_NAME='quark'
BINARY_PATH="/usr/sbin/$SERVICE_NAME"
CONFIG_PATH="/etc/$SERVICE_NAME"
LOG_PATH="/var/log/$SERVICE_NAME"

echo "Uninstalling Quark"

# Remove systemd service
if systemctl status "$SERVICE_NAME.service" >/dev/null 2>&1; then
  systemctl stop "$SERVICE_NAME"
  systemctl disable "$SERVICE_NAME"
  rm "/etc/systemd/system/$SERVICE_NAME.service"
  systemctl daemon-reload
fi

# Remove binary
if [ -f "$BINARY_PATH" ]; then
  echo "Removing the binary $BINARY_PATH"
  rm "$BINARY_PATH"
fi

# Remove configufation
while true; do
  read -p "Do you want to remove the configuration directory? (y/n)" yn
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
  *) echo "$YN_ERROR_MSG" ;;
  esac
done

# Remove logs directory
while true; do
  read -p "Do you want to remove the log directory? (y/n)" yn
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
  *) echo "$YN_ERROR_MSG" ;;
  esac
done

# Remove quark user
while true; do
  read -p "Do you want to delete the user 'quark'? (y/n)" yn
  case $yn in
  [Yy]*)
    if grep -q "^$SERVICE_NAME:" /etc/passwd; then
      echo "Deleting user $SERVICE_NAME"
      userdel -r "$SERVICE_NAME"
    else
      echo "The user $SERVICE_NAME has already been deleted"
    fi
    break
    ;;
  [Nn]*) break ;;
  *) echo "$YN_ERROR_MSG" ;;
  esac
done

echo "Quark has been uninstalled"
