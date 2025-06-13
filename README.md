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

## Minimum Supported Rust Version

The current MSRV is `1.85`.

## License

Quark is provided under the MIT license. See [LICENSE](https://github.com/Goliworks/Quark/blob/main/LICENSE).
