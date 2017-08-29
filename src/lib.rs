// This Source Code Form is subject to the terms of the Mozilla Public License,
// v. 2.0. If a copy of the MPL was not distributed with this file, You can
// obtain one at http://mozilla.org/MPL/2.0/.

#![cfg_attr(feature = "strict", deny(warnings))]
#![cfg_attr(feature = "strict", deny(missing_debug_implementations))]
#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy))]

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
use websocket::server::upgrade::{Request, WsUpgrade};

#[derive(Clone, Debug)]
pub struct WsHandshake {
    key: Vec<u8>,
}

impl WsHandshake {
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn into_key(self) -> Vec<u8> {
        self.key
    }

    pub fn detect<B>(req: &hyper::Request<B>) -> Option<Self> {
        WsHandshake::detect_from_parts(req.method(), req.version(), req.headers())
    }

    pub fn detect_from_parts(
        method: &Method,
        version: HttpVersion,
        headers: &Headers,
    ) -> Option<Self> {
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
                let contains_websocket = !protocols
                    .iter()
                    .any(|protocol| protocol.name == header::ProtocolName::WebSocket);
                if contains_websocket {
                    return None;
                }
            }
        }

        match headers.get::<header::Connection>() {
            None => return None,
            Some(&header::Connection(ref options)) => {
                let upgrade = options.iter().any(|option| match *option {
                    header::ConnectionOption::ConnectionHeader(ref value)
                        if value.as_ref().eq_ignore_ascii_case("upgrade") =>
                    {
                        true
                    }
                    _ => false,
                });
                if !upgrade {
                    return None;
                }
            }
        }

        Some(WsHandshake {
            key: key.to_owned(),
        })
    }

    pub fn start<T>(self, io: T, read_buf: BytesMut) -> WsStart<T> {
        WsStart::new(self, io, read_buf)
    }

    pub fn accept<T>(self, io: T, read_buf: BytesMut) -> AcceptWsHandshake<T>
    where
        T: AsyncRead + AsyncWrite + 'static,
    {
        AcceptWsHandshake(self.build_ws_upgrade(io, read_buf).accept())
    }

    pub fn reject<T>(self, io: T, read_buf: BytesMut) -> RejectWsHandshake<T>
    where
        T: AsyncRead + AsyncWrite + 'static,
    {
        RejectWsHandshake(self.build_ws_upgrade(io, read_buf).reject())
    }

    pub fn respond<T>(self, io: T, read_buf: BytesMut, accept: bool) -> SendWsResponse<T>
    where
        T: AsyncRead + AsyncWrite + 'static,
    {
        SendWsResponse(if accept {
            Ok(self.accept(io, read_buf))
        } else {
            Err(self.reject(io, read_buf))
        })
    }

    fn build_ws_upgrade<T>(self, io: T, read_buf: BytesMut) -> WsUpgrade<T, BytesMut>
    where
        T: AsyncRead + AsyncWrite,
    {
        let mut request: Request = Request {
            // Justification: see the comment on `OldHttpVersion` below.
            version: unsafe { mem::transmute(OldHttpVersion::Http11) },
            subject: (
                "GET".parse().expect("hyper-websocket: Method parse failed"),
                "*".parse().expect("hyper-websocket: RequestUri parse failed"),
            ),
            headers: FromIterator::from_iter(iter::empty()),
        };
        request.headers.set_raw("sec-websocket-key", vec![self.key]);
        WsUpgrade {
            headers: FromIterator::from_iter(iter::empty()),
            stream: io,
            request: request,
            buffer: read_buf,
        }
    }
}

pub struct AcceptWsHandshake<T>(ClientNew<T>);

impl<T> fmt::Debug for AcceptWsHandshake<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("AcceptWsHandshake").field(&"...").finish()
    }
}

impl<T> Future for AcceptWsHandshake<T> {
    type Item = Client<T>;
    type Error = WebSocketError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let (client, _) = try_ready!(self.0.poll());
        Ok(client.into())
    }
}

pub struct RejectWsHandshake<T: AsyncWrite>(Send<Framed<T, HttpServerCodec>>);

impl<T> fmt::Debug for RejectWsHandshake<T>
where
    T: AsyncWrite,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("RejectWsHandshake").field(&"...").finish()
    }
}

impl<T> Future for RejectWsHandshake<T>
where
    T: AsyncWrite,
{
    type Item = T;
    type Error = WebSocketError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let framed = try_ready!(self.0.poll());
        Ok(framed.into_inner().into())
    }
}

#[derive(Clone, Debug)]
pub struct WsResponse {
    pub handshake: WsHandshake,
    pub accept: bool,
}

impl WsResponse {
    pub fn accept(handshake: WsHandshake) -> Self {
        WsResponse {
            handshake: handshake,
            accept: true,
        }
    }

    pub fn reject(handshake: WsHandshake) -> Self {
        WsResponse {
            handshake: handshake,
            accept: false,
        }
    }

    pub fn send<T>(self, io: T, read_buf: BytesMut) -> SendWsResponse<T>
    where
        T: AsyncRead + AsyncWrite + 'static,
    {
        self.handshake.respond(io, read_buf, self.accept)
    }
}

pub struct SendWsResponse<T: AsyncWrite>(Result<AcceptWsHandshake<T>, RejectWsHandshake<T>>);

impl<T> fmt::Debug for SendWsResponse<T>
where
    T: AsyncWrite,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("SendWsResponse").field(&self.0.as_ref().map(|_| "...")).finish()
    }
}

impl<T> Future for SendWsResponse<T>
where
    T: AsyncWrite,
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

#[derive(Clone, Debug)]
pub struct WsStart<T> {
    handshake: WsHandshake,
    io: T,
    read_buf: BytesMut,
}

impl<T> WsStart<T> {
    pub fn new(handshake: WsHandshake, io: T, read_buf: BytesMut) -> Self {
        WsStart {
            handshake: handshake,
            io: io,
            read_buf: read_buf,
        }
    }

    pub fn into_parts(self) -> (WsHandshake, T, BytesMut) {
        (self.handshake, self.io, self.read_buf)
    }
}

impl<T> WsStart<T>
where
    T: AsyncRead + AsyncWrite + 'static,
{
    pub fn accept(self) -> AcceptWsHandshake<T> {
        self.handshake.accept(self.io, self.read_buf)
    }

    pub fn reject(self) -> RejectWsHandshake<T> {
        self.handshake.reject(self.io, self.read_buf)
    }

    pub fn respond(self, accept: bool) -> SendWsResponse<T> {
        self.handshake.respond(self.io, self.read_buf, accept)
    }
}

/// This type is structured to match the definition of hyper ^0.10.6's
/// `HttpVersion` type. It is used to translate from post-0.11 hyper's
/// `HttpVersion` type to the one form the older range which rust-websocket
/// depends on. Unfortunately Cargo doesn't give us an easy way to just reach in
/// and access this type from whatever version of hyper is selected for
/// rust-websocket, so we're left doing this hackily.
#[cfg_attr(feature = "clippy", allow(enum_variant_names))]
enum OldHttpVersion {
    #[allow(dead_code)]
    Http09,
    #[allow(dead_code)]
    Http10,
    Http11,
    #[allow(dead_code)]
    Http20,
}
