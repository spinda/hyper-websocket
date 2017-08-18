// This Source Code Form is subject to the terms of the Mozilla Public License,
// v. 2.0. If a copy of the MPL was not distributed with this file, You can
// obtain one at http://mozilla.org/MPL/2.0/.

extern crate bytes;
extern crate hyper;
extern crate tokio_io;
extern crate websocket;

#[macro_use]
extern crate futures;

use bytes::BytesMut;
use futures::{Future, Poll};
use futures::sink::Send;
use hyper::{HttpVersion, Method};
use hyper::header::{self, Headers, Raw};
use std::ascii::AsciiExt;
use std::fmt;
use std::iter::{self, FromIterator};
use std::mem;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::codec::Framed;
use websocket::client::async::{Client, ClientNew};
use websocket::codec::http::HttpServerCodec;
use websocket::result::WebSocketError;
use websocket::server::upgrade::{HyperIntoWsError, Request, WsUpgrade};

#[derive(Clone, Debug)]
pub struct WebSocketHandshake {
    key: Vec<u8>,
    version: HttpVersion,
}

impl WebSocketHandshake {
    pub fn detect<B>(req: &hyper::Request<B>) -> Option<Self> {
        WebSocketHandshake::detect_from_parts(req.method(), req.version(), req.headers())
    }

    pub fn detect_from_parts(method: &Method,
                             version: HttpVersion,
                             headers: &Headers)
                             -> Option<Self> {
        if *method != Method::Get {
            return None;
        }

        if version == HttpVersion::Http09 || version == HttpVersion::Http10 {
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
                if !protocols.iter()
                    .any(|protocol| protocol.name == header::ProtocolName::WebSocket) {
                    return None;
                }
            }
        }

        match headers.get::<header::Connection>() {
            None => return None,
            Some(&header::Connection(ref options)) => {
                let upgrade = options.iter().any(|option| {
                    match *option {
                        header::ConnectionOption::ConnectionHeader(ref value) if value.as_ref()
                            .eq_ignore_ascii_case("upgrade") => true,
                        _ => false,
                    }
                });
                if !upgrade {
                    return None;
                }
            }
        }

        Some(WebSocketHandshake {
            key: key.to_owned(),
            version: version,
        })
    }

    pub fn accept<T>(self, io: T, read_buf: BytesMut) -> AcceptWebSocketHandshake<T>
        where T: AsyncRead + AsyncWrite + 'static
    {
        AcceptWebSocketHandshake(self.build_ws_upgrade(io, read_buf).map(WsUpgrade::accept))
    }

    pub fn reject<T>(self, io: T, read_buf: BytesMut) -> RejectWebSocketHandshake<T>
        where T: AsyncRead + AsyncWrite + 'static
    {
        RejectWebSocketHandshake(self.build_ws_upgrade(io, read_buf).map(WsUpgrade::reject))
    }

    pub fn respond<T>(self, io: T, read_buf: BytesMut, accept: bool) -> SendWebSocketResponse<T>
        where T: AsyncRead + AsyncWrite + 'static
    {
        SendWebSocketResponse(if accept {
            Ok(self.accept(io, read_buf))
        } else {
            Err(self.reject(io, read_buf))
        })
    }

    fn build_ws_upgrade<T>(self, io: T, read_buf: BytesMut) -> Option<WsUpgrade<T, BytesMut>>
        where T: AsyncRead + AsyncWrite
    {
        let mut request: Request = Request {
            version: {
                let old_version = match self.version {
                    HttpVersion::Http09 => OldHttpVersion::Http09,
                    HttpVersion::Http10 => OldHttpVersion::Http10,
                    HttpVersion::Http11 => OldHttpVersion::Http11,
                    HttpVersion::H2 | HttpVersion::H2c => OldHttpVersion::Http20,
                    _ => return None,
                };
                // Justification: see the comment on OldHttpVersion below.
                unsafe { mem::transmute(old_version) }
            },
            subject: ("GET".parse().expect("hyper-websocket: Method parse failed"),
                      "*".parse().expect("hyper-websocket: RequestUri parse failed")),
            headers: FromIterator::from_iter(iter::empty()),
        };
        request.headers.set_raw("sec-websocket-key", vec![self.key]);
        Some(WsUpgrade {
            headers: FromIterator::from_iter(iter::empty()),
            stream: io,
            request: request,
            buffer: read_buf,
        })
    }
}

pub struct AcceptWebSocketHandshake<T>(Option<ClientNew<T>>);

impl<T> fmt::Debug for AcceptWebSocketHandshake<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("AcceptWebSocketHandshake")
            .field(&self.0.as_ref().map(|_| "..."))
            .finish()
    }
}

impl<T> Future for AcceptWebSocketHandshake<T> {
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

pub struct RejectWebSocketHandshake<T: AsyncWrite>(Option<Send<Framed<T, HttpServerCodec>>>);

impl<T> fmt::Debug for RejectWebSocketHandshake<T>
    where T: AsyncWrite
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("RejectWebSocketHandshake")
            .field(&self.0.as_ref().map(|_| "..."))
            .finish()
    }
}

impl<T> Future for RejectWebSocketHandshake<T>
    where T: AsyncWrite
{
    type Item = T;
    type Error = WebSocketError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.0 {
            None => Err(HyperIntoWsError::UnsupportedHttpVersion.into()),
            Some(ref mut future) => {
                let framed = try_ready!(future.poll());
                Ok(framed.into_inner().into())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct WebSocketResponse {
    pub handshake: WebSocketHandshake,
    pub accept: bool,
}

impl WebSocketResponse {
    pub fn accept(handshake: WebSocketHandshake) -> Self {
        WebSocketResponse {
            handshake: handshake,
            accept: true,
        }
    }

    pub fn reject(handshake: WebSocketHandshake) -> Self {
        WebSocketResponse {
            handshake: handshake,
            accept: false,
        }
    }

    pub fn send<T>(self, io: T, read_buf: BytesMut) -> SendWebSocketResponse<T>
        where T: AsyncRead + AsyncWrite + 'static
    {
        self.handshake.respond(io, read_buf, self.accept)
    }
}

pub struct SendWebSocketResponse<T: AsyncWrite>(Result<AcceptWebSocketHandshake<T>,
                                                       RejectWebSocketHandshake<T>>);

impl<T> fmt::Debug for SendWebSocketResponse<T>
    where T: AsyncWrite
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("SendWebSocketResponse")
            .field(&self.0.as_ref().map(|_| "..."))
            .finish()
    }
}

impl<T> Future for SendWebSocketResponse<T>
    where T: AsyncWrite
{
    type Item = Result<Client<T>, T>;
    type Error = WebSocketError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.0 {
            Ok(ref mut future) => Ok(Ok(try_ready!(future.poll())).into()),
            Err(ref mut future) => Ok(Err(try_ready!(future.poll())).into()),
        }
    }
}

/// This type is structured to match the definition of hyper ^0.10.6's
/// `HttpVersion` type. It is used to translate from post-0.11 hyper's
/// `HttpVersion` type to the one form the older range which rust-websocket
/// depends on. Unfortunately Cargo doesn't give us an easy way to just reach in
/// and access this type from whatever version of hyper is selected for
/// rust-websocket, so we're left doing this hackily.
#[cfg_attr(feature = "cargo-clippy", allow(enum_variant_names))]
enum OldHttpVersion {
    Http09,
    Http10,
    Http11,
    Http20,
}
