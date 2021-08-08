use std::sync::{Arc, Mutex};
use std::{env, io::Error};

use futures::{future};
use async_std::net::{TcpListener, TcpStream};
use async_std::task;
use futures_util::{TryStreamExt, StreamExt};
extern crate pretty_env_logger;
#[macro_use]
extern crate log;
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

async fn accept(stream: TcpStream) {
    let addr = stream
        .peer_addr()
        .expect("connected streams should have a peer address");
    info!("peer address: {}", addr);

    let ws_stream = async_tungstenite::accept_async(stream)
        .await
        .expect("Error during the websocket handshake occurred");

    info!("new WebSocket connection: {}", addr);

    let rover = Arc::new(Mutex::new(Rover::new()));

    let (_write, read) = ws_stream.split();
    let receive = read.try_for_each(|msg| {
        if let async_tungstenite::tungstenite::Message::Close(_) = msg {
            debug!("received 'close' from {}", addr);
            return future::ok(())
        }

        debug!(
            "received a message from {}: {}",
            addr,
            msg.to_text().unwrap()
        );

        let command = serde_json::from_str(msg.to_text().unwrap());

        match command {
            Ok(command) => {
                let mut rover = rover.lock().unwrap();

                match command {
                    RoverCommand::MotorRun { motor, direction, speed } => {
                        match motor {
                            RoverMotorId::Right => rover.right_motor.set_speed(speed, direction),
                            RoverMotorId::Left => rover.left_motor.set_speed(speed, direction),
                        }
                    }
                    RoverCommand::MotorStop { motor } => {
                        match motor {
                            RoverMotorId::Right => rover.right_motor.stop(),
                            RoverMotorId::Left => rover.left_motor.stop(),
                        }
                    }
                }
            },
            Err(e) => {
                error!("unable to parse command: {}", e);
            }
        };

        future::ok(())
    });

    receive.await;

    info!("WebSocket disconnected: {}", addr);

    rover.lock().unwrap().stop();
}

async fn run() -> Result<(), Error> {
    pretty_env_logger::init_custom_env("ROVER_LOG");

    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        task::spawn(accept(stream));
    }

    Ok(())
}

fn main() -> Result<(), Error> {
    let mut rover = Rover::new();

    rover.stop();

    task::block_on(run())
}
