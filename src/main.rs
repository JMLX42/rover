use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use pretty_env_logger;
#[macro_use]
extern crate log;
use hyper::{header, upgrade, StatusCode, Body, Request, Response, Server, server::conn::AddrStream};
use hyper::service::{make_service_fn, service_fn};
use tokio_tungstenite::WebSocketStream;
use futures::{future};
use futures_util::{TryStreamExt, StreamExt};
use tungstenite::{handshake, error::Error};
use serde::{Deserialize, Serialize};

mod rover;

use rover::{Rover, DCMotorDirection};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum RoverMotorId {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum RoverCommand {
    MotorRun { motor: RoverMotorId, direction: DCMotorDirection, speed: u16 },
    MotorStop { motor: RoverMotorId },
}

fn handle_message(
    addr: SocketAddr,
    msg: tungstenite::Message,
    rover: Arc<Mutex<Rover>>,
) -> Result<(), ()> {
    if let tungstenite::Message::Close(_) = msg {
        debug!("received 'close' from {}", addr);
        return Ok(())
    }

    debug!(
        "received a message from {}: {}",
        addr,
        msg.to_text().unwrap()
    );

    let command = serde_json::from_str(msg.to_text().unwrap());

    match command {
        Ok(command) => {
            match command {
                RoverCommand::MotorRun { motor, direction, speed } => {
                    let mut rover = rover.lock().unwrap();

                    match motor {
                        RoverMotorId::Right => rover.right_motor.set_speed(speed, direction),
                        RoverMotorId::Left => rover.left_motor.set_speed(speed, direction),
                    }
                }
                RoverCommand::MotorStop { motor } => {
                    let mut rover = rover.lock().unwrap();

                    match motor {
                        RoverMotorId::Right => rover.right_motor.stop(),
                        RoverMotorId::Left => rover.left_motor.stop(),
                    }
                }
            }
        },
        Err(e) => {
            // ! FIXME: return as future error
            error!("unable to parse command: {}", e);
        }
    };

    Ok(())
}

async fn handle_request(
    mut request: Request<Body>,
    remote_addr: SocketAddr,
    rover: Arc<Mutex<Rover>>,
) -> Result<Response<Body>, Infallible> {
    match (request.uri().path(), request.headers().contains_key(header::UPGRADE)) {
        //if the request is ws_echo and the request headers contains an Upgrade key
        ("/websocket", true) => {
            //assume request is a handshake, so create the handshake response
            let response = 
            match handshake::server::create_response_with_body(&request, || Body::empty()) {
                Ok(response) => {
                    //in case the handshake response creation succeeds,
                    //spawn a task to handle the websocket connection
                    tokio::spawn(async move {
                        //using the hyper feature of upgrading a connection
                        match upgrade::on(&mut request).await {
                            //if successfully upgraded
                            Ok(upgraded) => {
                                //create a websocket stream from the upgraded object
                                let ws_stream = WebSocketStream::from_raw_socket(
                                    //pass the upgraded object
                                    //as the base layer stream of the Websocket
                                    upgraded,
                                    tokio_tungstenite::tungstenite::protocol::Role::Server,
                                    None,
                                ).await;

                                info!("new WebSocket connection: {}", remote_addr);

                                //we can split the stream into a sink and a stream
                                let (_ws_write, ws_read) = ws_stream.split();
                                let receive = ws_read.try_for_each(|msg| {
                                    handle_message(remote_addr, msg, rover.clone());

                                    future::ok(())
                                });

                                match receive.await {
                                    Ok(_) => {
                                        rover.lock().unwrap().stop();
                                    },
                                    Err(Error::ConnectionClosed) => {
                                        rover.lock().unwrap().stop();
                                        info!("connection closed normally")
                                    },
                                    Err(e) => {
                                        rover.lock().unwrap().stop();
                                        error!("error: {:?}", e)
                                    },
                                }
                            },
                            Err(e) =>
                                error!(
                                    "error when trying to upgrade connection \
                                    from address {} to websocket connection: \
                                    {}",
                                    remote_addr,
                                    e
                                ),
                        }
                    });
                    //return the response to the handshake request
                    response
                },
                Err(error) => {
                    //probably the handshake request is not up to spec for websocket
                    error!(
                        "Failed to create websocket response \
                        to request from address {}: {}",
                        remote_addr,
                        error,
                    );
                    let mut res = Response::new(Body::from(format!("failed to create websocket: {}", error)));
                    *res.status_mut() = StatusCode::BAD_REQUEST;
                    return Ok(res);
                }
            };
        
            Ok::<_, Infallible>(response)
        },
        ("/websocket", false) => {
            //handle the case where the url is /websocket, but does not have an Upgrade field
            Ok(Response::new(Body::from(format!(
                "Getting even warmer, \
                try connecting to this url \
                using a websocket client.\n"
            ))))
        },
        (url, false) => {
            info!("serving URL {}", &url);

            let mut path = std::path::PathBuf::new();
            path.push(r".");
            path.push(&url[1..]);

            if path.exists() && path.is_file() {
                let contents = std::fs::read_to_string(&path)
                    .expect("something went wrong reading the file");

                debug!("serving static file {:?}", &path);
                Ok(Response::new(Body::from(contents)))
            } else {
                warn!("static file {:?} does not exist", &path);
                Ok(
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap()
                )
            }
        },
        (_, true) => {
            //handle any other url with an Upgrade header field
            Ok(Response::new(Body::from(format!(
                "Getting warmer, but I'm \
                only letting you connect \
                via websocket over on \
                /websocket, try that url.\n"
            ))))
        }
    }
}

async fn shutdown_signal(rover: Arc<Mutex<Rover>>) {
    // Wait for the CTRL+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");

    rover.lock().unwrap().stop();
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    pretty_env_logger::init_custom_env("ROVER_LOG");

    let rover = Arc::new(Mutex::new(Rover::new()));

    rover.lock().unwrap().stop();

    // hyper server boilerplate code from https://hyper.rs/guides/server/hello-world/
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));

    info!("listening on {} for http or websocket connections", addr);

    // A `Service` is needed for every connection, so this
    // creates one from our `handle_request` function.
    let make_svc = make_service_fn(|conn: & AddrStream| {
        let remote_addr = conn.remote_addr();
        let rover = rover.clone();

        async move {
            // service_fn converts our function into a `Service`
            Ok::<_, Infallible>(service_fn(move |request: Request<Body>|
                handle_request(request, remote_addr, rover.clone())
            ))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);
    let graceful = server.with_graceful_shutdown(shutdown_signal(rover.clone()));

    // Run this server for... forever!
    if let Err(e) = graceful.await {
        error!("server error: {}", e);
    }
}
