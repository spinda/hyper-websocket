# hyper-websocket *(unstable; unreleased)*

> Upgrade hyper HTTP requests to WebSocket connections (server-side)

<!-- [![Crates.io](https://img.shields.io/crates/v/hyper-websocket.svg)](https://crates.io/crates/hyper-websocket) -->
[![Linux/OSX Build Status](https://img.shields.io/travis/spinda/hyper-websocket/master.svg)](https://travis-ci.org/spinda/hyper-websocket)
[![Windows Build Status](https://img.shields.io/appveyor/ci/spinda/hyper-websocket/master.svg)](https://ci.appveyor.com/project/spinda/hyper-websocket)

[Documentation](https://www.spinda.net/files/mozilla/cdp/doc/hyper_websocket/index.html)

## Usage

First, add this to your `Cargo.toml`:

```toml
[dependencies.hyper-websocket]
git = "https://github.com/spinda/hyper-websocket"
```

Next, add this to your crate:

```rust
extern crate hyper_websocket;

use hyper_websocket;
```

## License

[MPL-2.0](/LICENSE)

Helpful resources:

- [Mozilla's MPL-2.0 FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)
- [MPL-2.0 on TLDRLegal](https://tldrlegal.com/license/mozilla-public-license-2.0-\(mpl-2\))

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you shall be licensed as above, without any
additional terms or conditions.
