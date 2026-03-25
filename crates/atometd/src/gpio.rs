use tokio::fs;
use tokio::time::{Duration, sleep};

pub enum GpioDirection {
    In,
    Out,
}

pub struct Gpio {
    pub number: u32,
    pub direction: GpioDirection,
}

impl Gpio {
    pub async fn init(&self) -> std::io::Result<()> {
        let path_str = format!("/sys/class/gpio/gpio{}", self.number);
        if tokio::fs::try_exists(&path_str).await.unwrap_or(false) {
            return Ok(());
        }

        echo("/sys/class/gpio/export", &self.number.to_string()).await?;
        let dir_str = match self.direction {
            GpioDirection::In => "in",
            GpioDirection::Out => "out",
        };
        echo(&format!("{}/direction", path_str), dir_str).await
    }

    pub async fn set_value(&self, value: u32) -> std::io::Result<()> {
        echo(
            &format!("/sys/class/gpio/gpio{}/value", self.number),
            &value.to_string(),
        )
        .await
    }
}

async fn echo(path: &str, text: &str) -> std::io::Result<()> {
    fs::write(path, text.as_bytes()).await
}

// ATOM Cam2 GPIO definitions
pub const GPIO_LED_ORANGE: Gpio = Gpio {
    number: 0x26,
    direction: GpioDirection::Out,
};
pub const GPIO_LED_BLUE: Gpio = Gpio {
    number: 0x27,
    direction: GpioDirection::Out,
};
pub const GPIO_LED_IR: Gpio = Gpio {
    number: 0x2F,
    direction: GpioDirection::Out,
};
pub const GPIO_IRCUT_TRIG1: Gpio = Gpio {
    number: 0x34,
    direction: GpioDirection::Out,
};
pub const GPIO_IRCUT_TRIG2: Gpio = Gpio {
    number: 0x35,
    direction: GpioDirection::Out,
};
pub const GPIO_BUTTON: Gpio = Gpio {
    number: 0x33,
    direction: GpioDirection::In,
};

pub async fn gpio_init() -> std::io::Result<()> {
    GPIO_LED_ORANGE.init().await?;
    GPIO_LED_BLUE.init().await?;
    GPIO_LED_IR.init().await?;
    GPIO_IRCUT_TRIG1.init().await?;
    GPIO_IRCUT_TRIG2.init().await?;
    GPIO_BUTTON.init().await?;

    // Cycle IR cut filter
    ircut_on().await?;
    ircut_off().await?;

    Ok(())
}

pub async fn ircut_on() -> std::io::Result<()> {
    GPIO_IRCUT_TRIG2.set_value(0).await?;
    sleep(Duration::from_millis(100)).await;
    GPIO_IRCUT_TRIG1.set_value(0).await
}

pub async fn ircut_off() -> std::io::Result<()> {
    GPIO_IRCUT_TRIG2.set_value(1).await?;
    sleep(Duration::from_millis(100)).await;
    GPIO_IRCUT_TRIG1.set_value(1).await
}

pub async fn irled_on() -> std::io::Result<()> {
    GPIO_LED_IR.set_value(1).await
}

pub async fn irled_off() -> std::io::Result<()> {
    GPIO_LED_IR.set_value(0).await
}

#[derive(Copy, Clone, Debug)]
pub enum Led {
    Orange,
    Blue,
}

pub async fn led_on(led: Led) -> std::io::Result<()> {
    match led {
        Led::Orange => GPIO_LED_ORANGE.set_value(0).await,
        Led::Blue => GPIO_LED_BLUE.set_value(0).await,
    }
}

pub async fn led_off(led: Led) -> std::io::Result<()> {
    match led {
        Led::Orange => GPIO_LED_ORANGE.set_value(1).await,
        Led::Blue => GPIO_LED_BLUE.set_value(1).await,
    }
}
