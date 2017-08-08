extern crate bytes;
extern crate hyper;
extern crate tokio_io;
extern crate websocket;

#[macro_use]
extern crate futures;

use bytes::BytesMut;
use futures::{Future, Poll};
use hyper::{HttpVersion, Method};
use hyper::header::{self, Headers, Raw};
use std::ascii::AsciiExt;
use std::fmt;
use std::iter::{self, FromIterator};
use std::mem;
use tokio_io::{AsyncRead, AsyncWrite};
use websocket::client::async::{Client, ClientNew};
use websocket::result::WebSocketError;
use websocket::server::upgrade::{HyperIntoWsError, Request, WsUpgrade};

pub fn start_handshake(method: &Method,
                       version: &HttpVersion,
                       headers: &Headers)
                       -> Option<HyperWebSocketHandshake> {
    if *method != Method::Get {
        return None;
    }

    if *version == HttpVersion::Http09 || *version == HttpVersion::Http10 {
        return None;
    }

    if let Some(version) = headers.get_raw("sec-websocket-version").and_then(Raw::one) {
        if version != b"13" {
            return None;
        }
    }

    let key = match headers.get_raw("sec-websocket-key").and_then(Raw::one) {
        None => return None,
        Some(key) => key,
    };

    match headers.get::<header::Upgrade>() {
        None => return None,
        Some(&header::Upgrade(ref protocols)) => {
            if !protocols.iter().any(|protocol| protocol.name == header::ProtocolName::WebSocket) {
                return None;
            }
        }
    }

    match headers.get::<header::Connection>() {
        None => return None,
        Some(&header::Connection(ref options)) => {
            let upgrade = options.iter().any(|option| {
                match option {
                    &header::ConnectionOption::ConnectionHeader(ref value) if value.as_ref()
                        .eq_ignore_ascii_case("upgrade") => true,
                    _ => false,
                }
            });
            if !upgrade {
                return None;
            }
        }
    }

    Some(HyperWebSocketHandshake {
        key: key.to_owned(),
        version: version.clone(),
    })
}

pub fn finish_handshake<T>(handshake: HyperWebSocketHandshake,
                           io: T,
                           read_buf: Option<BytesMut>)
                           -> FinishHandshake<T>
    where T: AsyncRead + AsyncWrite + 'static
{
    let mut request: Request = Request {
        version: {
            let old_version = match handshake.version {
                HttpVersion::Http09 => OldHttpVersion::Http09,
                HttpVersion::Http10 => OldHttpVersion::Http10,
                HttpVersion::Http11 => OldHttpVersion::Http11,
                HttpVersion::H2 => OldHttpVersion::Http20,
                HttpVersion::H2c => OldHttpVersion::Http20,
                _ => return FinishHandshake(None),
            };
            // Justification: see the comment on OldHttpVersion below.
            unsafe { mem::transmute(old_version) }
        },
        subject: ("GET".parse().expect("hyper-websocket: Method parse failed"),
                  "*".parse().expect("hyper-websocket: RequestUri parse failed")),
        headers: FromIterator::from_iter(iter::empty()),
    };
    request.headers.set_raw("sec-websocket-key", vec![handshake.key]);
    let upgrade = WsUpgrade {
        headers: FromIterator::from_iter(iter::empty()),
        stream: io,
        request: request,
        buffer: read_buf.unwrap_or(BytesMut::new()),
    };
    FinishHandshake(Some(upgrade.accept()))
}

#[derive(Clone, Debug)]
pub struct HyperWebSocketHandshake {
    key: Vec<u8>,
    version: HttpVersion,
}

pub struct FinishHandshake<T>(Option<ClientNew<T>>);

impl<T> fmt::Debug for FinishHandshake<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("FinishHandshake")
            .field(&self.0.as_ref().map(|_| "..."))
            .finish()
    }
}

impl<T> Future for FinishHandshake<T> {
    type Item = Client<T>;
    type Error = WebSocketError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.0 {
            None => Err(HyperIntoWsError::UnsupportedHttpVersion.into()),
            Some(ref mut future) => {
                let (client, _) = try_ready!(future.poll());
                Ok(client.into())
            }
        }
    }
}

/// This type is structured to match the definition of hyper ^0.10.6's
/// HttpVersion type. It is used to translate from post-0.11 hyper's HttpVersion
/// type to the one form the older range which rust-websocket depends on.
/// Unfortunately Cargo doesn't give us an easy way to just reach in and access
/// this type from whatever version of hyper is selected for rust-websocket, so
/// we're left doing this hackily.
enum OldHttpVersion {
    Http09,
    Http10,
    Http11,
    Http20,
}
