# Quark

Quark is a fast reverse proxy written in Rust.

## Features

- Easy configuration in TOML file.
- Automatic HTTPS by specifying certificates in the config file.
- Native IPv6 support (alongside IPv4) thanks to dual-stack sockets.
- HTTP/2 by default for HTTPS connections.
- Simple capabilities for serving static files.

## Minimum Support Rust Version

The current MSRV is `1.85`.

## License

Quark is provided under the MIT license. See [LICENSE](https://github.com/Goliworks/Quark/blob/main/LICENSE).
