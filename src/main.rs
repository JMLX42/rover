use std::{thread, time::Duration};
use std::cmp::{max};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::process;
use std::{env, io::Error};

use futures::prelude::*;
use futures::{
    future, pin_mut,
};

use async_std::net::{TcpListener, TcpStream};
use async_std::task;
use futures_util::{TryStreamExt, StreamExt};

use linux_embedded_hal::I2cdev;
use pwm_pca9685::{Address, Channel, Pca9685};
extern crate pretty_env_logger;
#[macro_use]
extern crate log;
use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum DCMotorDirection {
    Forward,
    Backward,
}

struct DCMotor {
    pwm: Pca9685<I2cdev>,
    control: Channel,
    forward: Channel,
    backward: Channel,
}

impl fmt::Debug for DCMotor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DCMotor")
            .field("control", &self.control)
            .field("forward", &self.forward)
            .field("backward", &self.backward)
            .finish()
    }
}

impl DCMotor {
    fn new(
        control: Channel,
        forward: Channel,
        backward: Channel,
    ) -> Self {
        trace!("creating i2c device");
        let dev = I2cdev::new("/dev/i2c-1").unwrap();
        let address = Address::default();
        trace!("creating PCA9685 device");
        let mut pwm = Pca9685::new(dev, address).unwrap();        
        // This corresponds to a frequency of ~100 Hz.
        pwm.set_prescale(240).unwrap();
        // It is necessary to enable the device.
        pwm.enable().unwrap();

        DCMotor {
            pwm,
            control,
            forward,
            backward,
        }
    }

    fn set_pwm_duty_cycle(self: &mut Self, channel: Channel, pulse: u16) {
        let off = max(
            0,
            // 100f32 because we assume the freq is set to 100hz
            (f32::from(pulse) * (4096f32 / 100f32) - 1f32).round() as u16
        );
        
        trace!("set_channel_on_off({:?}, 0, {})", channel, off);
        self.pwm.set_channel_on_off(channel, 0, off).unwrap();
        }

        fn set_level(self: &mut Self, channel: Channel, value: u16) {
        if value == 1 {
            trace!("set_channel_on_off({:?}, 0, 4095)", channel);
            self.pwm.set_channel_on_off(channel, 0, 4095).unwrap();
        } else {
            trace!("set_channel_on_off({:?}, 0, 0)", channel);
            self.pwm.set_channel_on_off(channel, 0, 0).unwrap();
        }
    }

    fn set_speed(self: &mut Self, speed: u16, direction: DCMotorDirection) {
        debug!("DCMotor.set_speed({:?}, {}, {:?})", self, speed, direction);
        
        self.set_pwm_duty_cycle(self.control, speed);

        match direction {
            DCMotorDirection::Forward => {
                self.set_level(self.forward, 1);
                self.set_level(self.backward, 0);
            },
            DCMotorDirection::Backward => {
                self.set_level(self.forward, 0);
                self.set_level(self.backward, 1);
            },
        };
    }

    fn stop(self: &mut Self) {
        debug!("DCMotor.stop({:?})", self);
        self.set_pwm_duty_cycle(self.control, 0);
    }
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

    let motor1 = Arc::new(Mutex::new(DCMotor::new(
        Channel::C0,
        Channel::C1,
        Channel::C2,
    )));
    let motor2 = Arc::new(Mutex::new(DCMotor::new(
        Channel::C5,
        Channel::C3,
        Channel::C4,
    )));

    {
        let mut motor1 = motor1.lock().unwrap();
        let mut motor2 = motor2.lock().unwrap();

        motor1.stop();
        motor2.stop();
    }

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
                let mut motor1 = motor1.lock().unwrap();
                let mut motor2 = motor2.lock().unwrap();

                match command {
                    RoverCommand::MotorRun { motor, direction, speed } => {
                        match motor {
                            RoverMotorId::Right => motor1.set_speed(speed, direction),
                            RoverMotorId::Left => motor2.set_speed(speed, direction),
                        }
                    }
                    RoverCommand::MotorStop { motor } => {
                        match motor {
                            RoverMotorId::Right => motor1.stop(),
                            RoverMotorId::Left => motor2.stop(),
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

    {
        let mut motor1 = motor1.lock().unwrap();
        let mut motor2 = motor2.lock().unwrap();

        motor1.stop();
        motor2.stop();
    }
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
    task::block_on(run())
}
