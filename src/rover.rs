use std::cmp::{max};
use std::fmt;

use serde::{Deserialize, Serialize};
use linux_embedded_hal::I2cdev;
use pwm_pca9685::{Address, Channel, Pca9685};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum DCMotorDirection {
    Forward,
    Backward,
}

pub struct DCMotor {
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

    pub fn set_speed(self: &mut Self, speed: u16, direction: DCMotorDirection) {
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

    pub fn stop(self: &mut Self) {
        debug!("DCMotor.stop({:?})", self);
        self.set_pwm_duty_cycle(self.control, 0);
    }
}

#[derive(Debug)]
pub struct Rover {
    pub right_motor: DCMotor,
    pub left_motor: DCMotor,
}

impl Rover {
    pub fn new() -> Self {
        Rover {
            right_motor: DCMotor::new(
                Channel::C0,
                Channel::C1,
                Channel::C2,
            ),
            left_motor: DCMotor::new(
                Channel::C5,
                Channel::C3,
                Channel::C4,
            ),
        }
    }

    pub fn stop(self: &mut Self) {
        trace!("Rover.stop({:?})", self);

        self.right_motor.stop();
        self.left_motor.stop();
    }
}
