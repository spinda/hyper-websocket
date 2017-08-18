// This Source Code Form is subject to the terms of the Mozilla Public License,
// v. 2.0. If a copy of the MPL was not distributed with this file, You can
// obtain one at http://mozilla.org/MPL/2.0/.

extern crate futures;
extern crate hyper;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_service;
extern crate tokio_timer;
extern crate websocket;

extern crate hyper_websocket;

use futures::{Future, Sink, Stream};
use futures::future::{self, Either};
use hyper::{Request, Response, StatusCode};
use hyper::server::{Http, UpgradableResponse};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::str;
use std::time::Duration;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::{Core, Handle};
use tokio_service::Service;
use tokio_timer::Timer;
use websocket::ClientBuilder;
use websocket::message::OwnedMessage;
use websocket::result::WebSocketError;

use hyper_websocket::{WebSocketHandshake, WebSocketResponse};

struct TestService;

impl Service for TestService {
    type Request = Request;
    type Response = UpgradableResponse<WebSocketResponse>;
    type Error = hyper::Error;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        match WebSocketHandshake::detect(&req) {
            None => {
                Box::new(future::ok(UpgradableResponse::Response(Response::new()
                    .with_status(StatusCode::Ok)
                    .with_body("Hello World"))))
            }
            Some(handshake) => {
                match req.path() {
                    "/accept" => {
                        Box::new(future::ok(UpgradableResponse::Upgrade(
                                    WebSocketResponse::accept(handshake), None)))
                    }
                    "/reject" => {
                        Box::new(future::ok(UpgradableResponse::Upgrade(
                                    WebSocketResponse::reject(handshake), None)))
                    }
                    "/sleep_then_accept" => {
                        let timer = Timer::default();
                        Box::new(timer.sleep(Duration::from_millis(500))
                            .map_err(|err| io::Error::new(io::ErrorKind::Other, err).into())
                            .and_then(move |_| {
                                Ok(UpgradableResponse::Upgrade(
                                        WebSocketResponse::accept(handshake), None))
                            }))
                    }
                    "/sleep_then_reject" => {
                        let timer = Timer::default();
                        Box::new(timer.sleep(Duration::from_millis(500))
                            .map_err(|err| io::Error::new(io::ErrorKind::Other, err).into())
                            .and_then(move |_| {
                                Ok(UpgradableResponse::Upgrade(
                                        WebSocketResponse::reject(handshake), None))
                            }))
                    }
                    "/sleep" => {
                        let timer = Timer::default();
                        Box::new(timer.sleep(Duration::from_millis(500))
                            .map_err(|err| io::Error::new(io::ErrorKind::Other, err).into())
                            .and_then(move |_| {
                                Box::new(future::ok(UpgradableResponse::Response(Response::new()
                                    .with_status(StatusCode::Ok)
                                    .with_body("zzz"))))
                            }))
                    }
                    _ => {
                        Box::new(future::ok(UpgradableResponse::Response(Response::new()
                            .with_status(StatusCode::NotFound))))
                    }
                }
            }
        }
    }
}

fn start_server(handle: &Handle) -> SocketAddr {
    let server_handle = handle.clone();

    let server_addr = SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 0);
    let listener = TcpListener::bind(&server_addr, handle).expect("listener bind error");
    let server_addr = listener.local_addr().expect("server address retrieval error");

    let server_proto = Http::new();
    let serve = listener.incoming()
        .for_each(move |(tcp, remote_addr)| {
            server_proto.bind_upgradable_connection(&server_handle, tcp, remote_addr, TestService)
                .then(|result| {
                    let maybe_upgrade = result.expect("server http error");
                    let (io, read_buf, ws_res) = match maybe_upgrade {
                        None => return Either::A(future::ok(())),
                        Some(upgrade) => upgrade,
                    };

                    Either::B(ws_res.send(io, read_buf)
                        .then(|result| {
                            let result = result.expect("server websocket response error");
                            let websocket = match result {
                                Err(_) => return Either::A(future::ok(())),
                                Ok(websocket) => websocket,
                            };

                            Either::B(websocket.send(OwnedMessage::Text("Hello".into()))
                                .then(|result| {
                                    let websocket = result.expect("server websocket send error");
                                    websocket.into_future().map_err(|(err, _websocket)| err)
                                })
                                .then(|result| {
                                    let (maybe_msg, _websocket) =
                                        result.expect("server websocket receive error");
                                    assert_eq!(maybe_msg, Some(OwnedMessage::Text("World".into())));
                                    Ok(())
                                }))
                        }))
                })
        })
        .then(|result| {
            result.expect("server accept error");
            Ok(())
        });
    handle.spawn(serve);

    server_addr
}

#[test]
fn test_http() {
    let mut core = Core::new().expect("core creation error");
    let handle = core.handle();
    let server_addr = start_server(&handle);

    let client = hyper::Client::new(&handle);
    let test = client.get(format!("http://{}/foo", server_addr).parse().expect("uri parse error"))
        .then(|result| {
            let res = result.expect("client http error");
            res.body().concat2()
        })
        .and_then(|body| {
            let body_str = str::from_utf8(body.as_ref()).expect("client body decode error");
            assert_eq!(body_str, "Hello World");
            Ok(())
        });
    core.run(test).expect("client body read error");
}

#[test]
fn test_accept() {
    do_test_accept("accept");
}

#[test]
fn test_reject() {
    do_test_wrong_status_code("reject");
}

#[test]
fn test_404() {
    do_test_wrong_status_code("403");
}

#[test]
fn test_sleep_then_accept() {
    do_test_accept("sleep_then_accept");
}

#[test]
fn test_sleep_then_reject() {
    do_test_wrong_status_code("sleep_then_reject");
}

#[test]
fn test_sleep_then_200() {
    do_test_wrong_status_code("sleep");
}

fn do_test_accept(endpt: &'static str) {
    let mut core = Core::new().expect("core creation error");
    let handle = core.handle();
    let server_addr = start_server(&handle);

    let test = ClientBuilder::new(format!("ws://{}/{}", server_addr, endpt).as_str())
        .expect("client build error")
        .async_connect_insecure(&handle)
        .then(|result| {
            let (websocket, _headers) = result.expect("client connect error");
            websocket.into_future().map_err(|(err, _websocket)| err)
        })
        .then(|result| {
            let (maybe_msg, websocket) = result.expect("client websocket receive error");
            assert_eq!(maybe_msg, Some(OwnedMessage::Text("Hello".into())));
            websocket.send(OwnedMessage::Text("World".into()))
        })
        .and_then(|_websocket| Ok(()));
    core.run(test).expect("client websocket send error");
}

fn do_test_wrong_status_code(endpt: &'static str) {
    let mut core = Core::new().expect("core creation error");
    let handle = core.handle();
    let server_addr = start_server(&handle);

    let expected_msg = "Status code must be Switching Protocols";
    let test = ClientBuilder::new(format!("ws://{}/{}", server_addr, endpt).as_str())
        .expect("client build error")
        .async_connect_insecure(&handle)
        .then(|result| {
            match result {
                Ok(_) => Err("unexpected websocket connection success".into()),
                Err(WebSocketError::ResponseError(msg)) if msg == expected_msg => Ok(()),
                Err(err) => Err(format!("unexpected weboskcet connect error: {}", err)),
            }
        });
    core.run(test).unwrap();
}

#[test]
fn test_manual_exchange() {
    let mut core = Core::new().expect("core creation error");
    let handle = core.handle();
    let server_addr = start_server(&handle);

    // GET /sleep_then_accept HTTP/1.1
    // Host: 127.0.0.1
    // Connection: Upgrade
    // Upgrade: websocket
    // Sec-WebSocket-Version: 13
    // Sec-WebSocket-Key: r3MGDiK57a1jWWkCmkiK5g==
    //
    // <-- World
    let to_server = vec![
        71, 69, 84, 32, 47, 115, 108, 101, 101, 112, 95, 116, 104, 101, 110, 95, 97, 99, 99, 101,
        112, 116, 32, 72, 84, 84, 80, 47, 49, 46, 49, 13, 10, 72, 111, 115, 116, 58, 32, 49, 50,
        55, 46, 48, 46, 48, 46, 49, 13, 10, 67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 58, 32,
        85, 112, 103, 114, 97, 100, 101, 13, 10, 85, 112, 103, 114, 97, 100, 101, 58, 32, 119, 101,
        98, 115, 111, 99, 107, 101, 116, 13, 10, 83, 101, 99, 45, 87, 101, 98, 83, 111, 99, 107,
        101, 116, 45, 86, 101, 114, 115, 105, 111, 110, 58, 32, 49, 51, 13, 10, 83, 101, 99, 45,
        87, 101, 98, 83, 111, 99, 107, 101, 116, 45, 75, 101, 121, 58, 32, 114, 51, 77, 71, 68,
        105, 75, 53, 55, 97, 49, 106, 87, 87, 107, 67, 109, 107, 105, 75, 53, 103, 61, 61, 13, 10,
        13, 10, 129, 133, 1, 89, 33, 49, 86, 54, 83, 93, 101, 13, 10
    ];

    // HTTP/1.1 101 Switching Protocols
    // Sec-WebSocket-Accept: JLE0Vo61YzV3Sfq6kch3QrFZICM=
    // Connection: Upgrade
    // Upgrade: websocket
    // 
    // --> Hello
    let from_server = vec![
        72, 84, 84, 80, 47, 49, 46, 49, 32, 49, 48, 49, 32, 83, 119, 105, 116, 99, 104, 105, 110,
        103, 32, 80, 114, 111, 116, 111, 99, 111, 108, 115, 13, 10, 83, 101, 99, 45, 87, 101, 98,
        83, 111, 99, 107, 101, 116, 45, 65, 99, 99, 101, 112, 116, 58, 32, 74, 76, 69, 48, 86, 111,
        54, 49, 89, 122, 86, 51, 83, 102, 113, 54, 107, 99, 104, 51, 81, 114, 70, 90, 73, 67, 77,
        61, 13, 10, 67, 111, 110, 110, 101, 99, 116, 105, 111, 110, 58, 32, 85, 112, 103, 114, 97,
        100, 101, 13, 10, 85, 112, 103, 114, 97, 100, 101, 58, 32, 119, 101, 98, 115, 111, 99, 107,
        101, 116, 13, 10, 13, 10, 129, 5, 72, 101, 108, 108, 111
    ];
    let from_server_len = from_server.len();

    let test = TcpStream::connect(&server_addr, &handle)
        .then(move |result| {
            let tcp = result.expect("client connect error");
            tokio_io::io::write_all(tcp, to_server)
        })
        .then(move |result| {
            let (tcp, _msg) = result.expect("client send error");
            let buf = vec![0; from_server_len];
            tokio_io::io::read_exact(tcp, buf)
        })
        .and_then(move |(_tcp, msg)| {
            assert_eq!(msg, from_server);
            Ok(())
        });
    core.run(test).expect("client receive error");
}
