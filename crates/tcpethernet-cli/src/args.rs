//! Argument parsing, std-only. Kept separate from `main` so it is unit-tested.

use std::net::Ipv4Addr;
use std::time::Duration;

pub const USAGE: &str = "\
usage:
  tcpethernet discover [TARGET] [--window MS]
      ListIdentity over UDP. TARGET is an IPv4 address or broadcast
      (default 255.255.255.255). Window defaults to 2000 ms.
  tcpethernet identity HOST
      Read Identity attributes 1-7 over an explicit session.
  tcpethernet get HOST CLASS.INSTANCE.ATTRIBUTE
      Get_Attribute_Single; prints the bytes as hex. Numbers may be 0x-hex.
  tcpethernet io HOST --config N --out N --in N --out-len N --in-len N
                 [--rpi MS] [--seconds S] [--input-only] [--set HEX]
      Forward_Open a Class 1 connection, optionally write output bytes,
      print each new input for S seconds (default 5), Forward_Close.
      Binds UDP 2222 locally; the target sends input there.
  tcpethernet help

  --port N (any command): encapsulation port instead of 44818.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Discover {
        target: Ipv4Addr,
        window: Duration,
    },
    Identity {
        host: Ipv4Addr,
    },
    Get {
        host: Ipv4Addr,
        class: u16,
        instance: u16,
        attribute: u16,
    },
    Io(IoArgs),
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoArgs {
    pub host: Ipv4Addr,
    pub config: u16,
    pub out: u16,
    pub input: u16,
    pub out_len: u16,
    pub in_len: u16,
    pub rpi: Duration,
    pub seconds: u64,
    pub input_only: bool,
    pub set: Option<Vec<u8>>,
}

pub fn number(s: &str) -> Result<u16, String> {
    let r = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .map_or_else(|| s.parse::<u16>(), |hex| u16::from_str_radix(hex, 16));
    r.map_err(|_| format!("not a number 0..=65535: {s:?}"))
}

fn ip(s: &str) -> Result<Ipv4Addr, String> {
    s.parse().map_err(|_| format!("not an IPv4 address: {s:?}"))
}

pub fn hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    if !clean.len().is_multiple_of(2) {
        return Err(format!("hex needs an even number of digits: {s:?}"));
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).map_err(|_| format!("bad hex: {s:?}")))
        .collect()
}

fn path(s: &str) -> Result<(u16, u16, u16), String> {
    let parts: Vec<&str> = s.split('.').collect();
    let [c, i, a] = parts.as_slice() else {
        return Err(format!("expected CLASS.INSTANCE.ATTRIBUTE, got {s:?}"));
    };
    Ok((number(c)?, number(i)?, number(a)?))
}

fn flag_value<'a>(args: &'a [String], i: &mut usize, name: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| format!("{name} needs a value"))
}

/// Split out the global `--port N` flag, then parse the command.
///
/// # Errors
/// A message naming the bad argument.
pub fn parse_with_port(args: &[String]) -> Result<(Cmd, u16), String> {
    let mut port = 44818;
    let mut rest = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--port" {
            let v = flag_value(args, &mut i, "--port")?;
            port = number(v)?;
        } else {
            rest.push(args[i].clone());
        }
        i += 1;
    }
    Ok((parse(&rest)?, port))
}

/// Parse everything after the program name.
///
/// # Errors
/// A message naming the bad argument.
pub fn parse(args: &[String]) -> Result<Cmd, String> {
    let Some(cmd) = args.first() else {
        return Ok(Cmd::Help);
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "help" | "-h" | "--help" => Ok(Cmd::Help),
        "discover" => {
            let mut target = Ipv4Addr::BROADCAST;
            let mut window = Duration::from_millis(2000);
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--window" => {
                        let v = flag_value(rest, &mut i, "--window")?;
                        window =
                            Duration::from_millis(v.parse().map_err(|_| format!("bad --window: {v:?}"))?);
                    }
                    other if !other.starts_with('-') => target = ip(other)?,
                    other => return Err(format!("unknown flag {other}")),
                }
                i += 1;
            }
            Ok(Cmd::Discover { target, window })
        }
        "identity" => match rest {
            [h] => Ok(Cmd::Identity { host: ip(h)? }),
            _ => Err("identity takes exactly one HOST".into()),
        },
        "get" => match rest {
            [h, p] => {
                let (class, instance, attribute) = path(p)?;
                Ok(Cmd::Get {
                    host: ip(h)?,
                    class,
                    instance,
                    attribute,
                })
            }
            _ => Err("get takes HOST CLASS.INSTANCE.ATTRIBUTE".into()),
        },
        "io" => parse_io(rest).map(Cmd::Io),
        other => Err(format!("unknown command {other:?}")),
    }
}

fn parse_io(rest: &[String]) -> Result<IoArgs, String> {
    let host = ip(rest.first().ok_or("io needs HOST")?)?;
    let (mut config, mut out, mut input, mut out_len, mut in_len) = (None, None, None, None, None);
    let mut rpi = Duration::from_millis(100);
    let mut seconds = 5;
    let mut input_only = false;
    let mut set = None;
    let mut i = 1;
    while i < rest.len() {
        let flag = rest[i].as_str();
        match flag {
            "--config" => config = Some(number(flag_value(rest, &mut i, flag)?)?),
            "--out" => out = Some(number(flag_value(rest, &mut i, flag)?)?),
            "--in" => input = Some(number(flag_value(rest, &mut i, flag)?)?),
            "--out-len" => out_len = Some(number(flag_value(rest, &mut i, flag)?)?),
            "--in-len" => in_len = Some(number(flag_value(rest, &mut i, flag)?)?),
            "--rpi" => {
                let v = flag_value(rest, &mut i, flag)?;
                rpi = Duration::from_millis(v.parse().map_err(|_| format!("bad --rpi: {v:?}"))?);
            }
            "--seconds" => {
                let v = flag_value(rest, &mut i, flag)?;
                seconds = v.parse().map_err(|_| format!("bad --seconds: {v:?}"))?;
            }
            "--input-only" => input_only = true,
            "--set" => set = Some(hex_bytes(flag_value(rest, &mut i, flag)?)?),
            other => return Err(format!("unknown flag {other}")),
        }
        i += 1;
    }
    let need = |v: Option<u16>, n: &str| v.ok_or_else(|| format!("io needs {n}"));
    let args = IoArgs {
        host,
        config: need(config, "--config")?,
        out: need(out, "--out")?,
        input: need(input, "--in")?,
        out_len: if input_only {
            0
        } else {
            need(out_len, "--out-len")?
        },
        in_len: need(in_len, "--in-len")?,
        rpi,
        seconds,
        input_only,
        set,
    };
    if let Some(s) = &args.set {
        if args.input_only {
            return Err("--set makes no sense with --input-only".into());
        }
        if s.len() != usize::from(args.out_len) {
            return Err(format!(
                "--set is {} bytes, --out-len is {}",
                s.len(),
                args.out_len
            ));
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn discover_defaults_and_flags() {
        assert_eq!(
            parse(&a(&["discover"])).unwrap(),
            Cmd::Discover {
                target: Ipv4Addr::BROADCAST,
                window: Duration::from_millis(2000)
            }
        );
        assert_eq!(
            parse(&a(&["discover", "10.0.0.255", "--window", "500"])).unwrap(),
            Cmd::Discover {
                target: Ipv4Addr::new(10, 0, 0, 255),
                window: Duration::from_millis(500)
            }
        );
        assert!(parse(&a(&["discover", "--window"])).is_err());
        assert!(parse(&a(&["discover", "nope"])).is_err());
    }

    #[test]
    fn get_accepts_hex_and_decimal() {
        assert_eq!(
            parse(&a(&["get", "127.0.0.1", "0xF5.1.5"])).unwrap(),
            Cmd::Get {
                host: Ipv4Addr::LOCALHOST,
                class: 0xF5,
                instance: 1,
                attribute: 5
            }
        );
        assert!(parse(&a(&["get", "127.0.0.1", "1.1"])).is_err());
        assert!(parse(&a(&["get", "127.0.0.1", "1.1.70000"])).is_err());
    }

    #[test]
    fn io_requires_points_and_checks_set_length() {
        let ok = parse(&a(&[
            "io",
            "10.0.0.5",
            "--config",
            "151",
            "--out",
            "150",
            "--in",
            "100",
            "--out-len",
            "4",
            "--in-len",
            "4",
            "--set",
            "01:02:03:04",
            "--rpi",
            "20",
        ]))
        .unwrap();
        let Cmd::Io(io) = ok else { panic!() };
        assert_eq!(io.set, Some(vec![1, 2, 3, 4]));
        assert_eq!(io.rpi, Duration::from_millis(20));
        assert!(parse(&a(&["io", "10.0.0.5", "--config", "151"])).is_err());
        assert!(
            parse(&a(&[
                "io",
                "10.0.0.5",
                "--config",
                "1",
                "--out",
                "2",
                "--in",
                "3",
                "--out-len",
                "4",
                "--in-len",
                "4",
                "--set",
                "0102"
            ]))
            .is_err()
        );
        let io = parse(&a(&[
            "io",
            "10.0.0.5",
            "--config",
            "151",
            "--out",
            "152",
            "--in",
            "100",
            "--in-len",
            "32",
            "--input-only",
        ]))
        .unwrap();
        let Cmd::Io(io) = io else { panic!() };
        assert_eq!(io.out_len, 0);
    }

    #[test]
    fn port_flag_anywhere() {
        let (cmd, port) = parse_with_port(&a(&["--port", "2000", "identity", "127.0.0.1"])).unwrap();
        assert_eq!(port, 2000);
        assert_eq!(
            cmd,
            Cmd::Identity {
                host: Ipv4Addr::LOCALHOST
            }
        );
        let (_, port) = parse_with_port(&a(&["identity", "127.0.0.1"])).unwrap();
        assert_eq!(port, 44818);
        assert!(parse_with_port(&a(&["identity", "127.0.0.1", "--port"])).is_err());
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(hex_bytes("de ad be ef").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(hex_bytes("abc").is_err());
        assert!(hex_bytes("zz").is_err());
    }

    #[test]
    fn empty_and_unknown() {
        assert_eq!(parse(&[]).unwrap(), Cmd::Help);
        assert!(parse(&a(&["frobnicate"])).is_err());
    }
}
