//! The programs shipped in programs/ (what scripts/pi-deploy-plc.sh puts
//! on a box): each compiles, and the line demo moves on its own.

use std::path::PathBuf;
use std::time::Duration;

use helmsman_iec::{Image, ImageSizes, compile};

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 0 };

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs")
}

#[test]
fn every_shipped_program_compiles() {
    let mut n = 0;
    for e in std::fs::read_dir(dir()).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("st")) {
            let src = std::fs::read_to_string(&path).unwrap();
            compile(&src, SIZES).unwrap_or_else(|d| panic!("{}: {d:#?}", path.display()));
            n += 1;
        }
    }
    assert!(n >= 2, "echo.st and line.st at least");
}

#[test]
fn the_line_demo_moves_with_nothing_wired_and_a_jam_trips_station_1() {
    let src = std::fs::read_to_string(dir().join("line.st")).unwrap();
    let mut p = compile(&src, SIZES).unwrap();
    let mut img = Image::new(SIZES);
    // %QX0.0 station 1 busy, %QX0.1 station 2 busy, %QX0.2 alarm.
    let q = |img: &Image, bit: u8| img.q[0] & (1 << bit) != 0;
    let (mut busy, mut idle) = ([false; 2], [false; 2]);
    // 10 s of 10 ms scans: both feeders flip several times.
    for t in 0..1000u64 {
        p.scan_at(&mut img, Duration::from_millis(t * 10)).unwrap();
        for (k, bit) in [0u8, 1].into_iter().enumerate() {
            if q(&img, bit) {
                busy[k] = true;
            } else if t > 50 {
                idle[k] = true;
            }
        }
        assert!(!q(&img, 2), "no jam, no alarm (t={t})");
    }
    assert_eq!(
        (busy, idle),
        ([true; 2], [true; 2]),
        "each station runs and rests"
    );
    // A jam on %IX0.0 trips station 1's infeed the next time it runs.
    img.i[0] |= 1;
    let mut tripped = false;
    for t in 1000..1500u64 {
        p.scan_at(&mut img, Duration::from_millis(t * 10)).unwrap();
        tripped |= q(&img, 2);
    }
    assert!(tripped, "the jam raised the alarm");
}
