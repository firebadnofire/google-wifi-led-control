use std::env;
use std::process::ExitCode;

use gale_led::{Color, Config, Error, Hardware};

const USAGE: &str = "Usage:
  gale-led apply
  gale-led set '#RRGGBB' [--brightness 0-100]
  gale-led set-rgb RED GREEN BLUE [--brightness 0-100]
  gale-led off
  gale-led status
  gale-led hardware
  gale-led interfaces
  gale-led validate
  gale-led run";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(output) => {
            if let Some(output) = output {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("gale-led: {error}");
            if matches!(error, Error::InvalidInput(_)) {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run(args: Vec<String>) -> Result<Option<String>, Error> {
    let command = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| Error::InvalidInput("a command is required".into()))?;

    match command {
        "apply" => {
            require_arg_count(&args, 1)?;
            let config = Config::load_uci()?;
            Hardware::discover()?.apply_config(&config)?;
            Ok(None)
        }
        "set" => {
            if args.len() < 2 {
                return Err(Error::InvalidInput("set requires a #RRGGBB color".into()));
            }
            let color = Color::from_hex(&args[1])?;
            let brightness = parse_brightness_option(&args[2..])?;
            Hardware::discover()?.set(color, brightness)?;
            Ok(None)
        }
        "set-rgb" => {
            if args.len() < 4 {
                return Err(Error::InvalidInput(
                    "set-rgb requires red, green, and blue values".into(),
                ));
            }
            let color = Color::from_components(
                parse_component("red", &args[1])?,
                parse_component("green", &args[2])?,
                parse_component("blue", &args[3])?,
            )?;
            let brightness = parse_brightness_option(&args[4..])?;
            Hardware::discover()?.set(color, brightness)?;
            Ok(None)
        }
        "off" => {
            require_arg_count(&args, 1)?;
            Hardware::discover()?.off()?;
            Ok(None)
        }
        "status" => {
            require_arg_count(&args, 1)?;
            let config = Config::load_uci()?;
            Ok(Some(Hardware::discover()?.status_json(&config)?))
        }
        "hardware" => {
            require_arg_count(&args, 1)?;
            Ok(Some(Hardware::discover()?.hardware_json()))
        }
        "interfaces" => {
            require_arg_count(&args, 1)?;
            Ok(Some(gale_led::interfaces_json()?))
        }
        "validate" => {
            require_arg_count(&args, 1)?;
            let config = Config::load_uci()?;
            gale_led::validate_runtime(&config)?;
            Hardware::discover()?;
            Ok(None)
        }
        "run" => {
            require_arg_count(&args, 1)?;
            let config = Config::load_uci()?;
            let hardware = Hardware::discover()?;
            gale_led::run(&config, &hardware)?;
            Ok(None)
        }
        "help" | "--help" | "-h" => Ok(Some(USAGE.into())),
        unknown => Err(Error::InvalidInput(format!("unknown command {unknown}"))),
    }
}

fn require_arg_count(args: &[String], expected: usize) -> Result<(), Error> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "{} does not accept additional arguments",
            args[0]
        )))
    }
}

fn parse_component(name: &str, value: &str) -> Result<u16, Error> {
    value.parse::<u16>().map_err(|_| {
        Error::InvalidInput(format!(
            "{name} channel must be an integer between 0 and 255"
        ))
    })
}

fn parse_brightness_option(args: &[String]) -> Result<u8, Error> {
    match args {
        [] => Ok(100),
        [flag, value] if flag == "--brightness" => value
            .parse::<u8>()
            .map_err(|_| {
                Error::InvalidInput("brightness must be an integer between 0 and 100".into())
            })
            .and_then(|brightness| {
                gale_led::validate_brightness(brightness)?;
                Ok(brightness)
            }),
        _ => Err(Error::InvalidInput(
            "expected only --brightness followed by a value from 0 through 100".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_brightness() {
        assert_eq!(parse_brightness_option(&[]).unwrap(), 100);
        assert_eq!(
            parse_brightness_option(&["--brightness".into(), "75".into()]).unwrap(),
            75
        );
        assert!(parse_brightness_option(&["--brightness".into(), "101".into()]).is_err());
        assert!(parse_brightness_option(&["--bright".into(), "75".into()]).is_err());
    }

    #[test]
    fn rejects_negative_and_oversized_components() {
        assert!(parse_component("red", "-1").is_err());
        assert_eq!(parse_component("red", "256").unwrap(), 256);
        assert!(Color::from_components(256, 0, 0).is_err());
    }
}
