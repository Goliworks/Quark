<picture>
  <source srcset="https://github.com/user-attachments/assets/a604dcec-2839-4d70-93cc-4c0a9495ddeb" media="(prefers-color-scheme: dark)">
  <source srcset="https://github.com/user-attachments/assets/0a796426-e75d-428d-9730-84633788e667" media="(prefers-color-scheme: light)">
  <img src="https://github.com/user-attachments/assets/89ef7f38-e1df-48b0-ba13-3c4f9f7e4e4f" alt="Logo">
</picture>

<hr/>

A fast reverse proxy written in Rust.

## Features

- Easy configuration in TOML file.
- Automatic HTTPS configuration simply by specifying certificates in the config file.
- Native IPv6 support (alongside IPv4) thanks to dual-stack sockets.
- HTTP/2 by default for HTTPS connections.
- Simple capabilities for serving static files.

## Installation and configuration

Download the latest release or build it with `./tools/build.sh`, extract the archive and run `./install.sh`.
The server will start immediately on port 80 after installation.
Then, edit `/etc/quark/config.toml` and restart the server with `systemctl restart quark`.

You can use `config.example.toml` as a template.

The server’s log files are stored in `/var/log/quark/`

You can remove Quark from your machine by running `./uninstall.sh.`

## Quick usage

Run the binary with the following command:

`./quark --config /path/to/your/config_file.toml --logs /path/to/logs/directory`

If you run the binary without any parameters, the server will attempt to use the default paths.

## Minimum Supported Rust Version

The current MSRV is `1.85`.

## License

Quark is provided under the MIT license. See [LICENSE](https://github.com/Goliworks/Quark/blob/main/LICENSE).
