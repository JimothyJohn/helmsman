//! Vendors' own EDS files, which are not in this repository: the Moxa
//! ioLogik E1213's (its terms don't say it may be redistributed; TODO.md
//! `e1213-eds-license`) and `OpENer`'s sample application's. Each file
//! under `$HELMSMAN_EDS_CORPUS` is checked against its SHA-256, parsed,
//! and resolved to what this code builds in: the E1213 to its profile
//! (`Spec::moxa_e1213`, `DeviceConfig::moxa_e1213`), `OpENer`'s to the
//! sample layout `helmsman-adapter` copies.
//!
//! `scripts/eds-corpus.sh` collects the files and runs this. Without the
//! variable there is nothing to read and the test passes; a file that is
//! there with another hash fails it.

use std::path::{Path, PathBuf};

use helmsman_adapter::DeviceConfig;
use helmsman_io::enip::{Connection, Spec};
use helmsman_proto::eds::{AppType, Eds, RealTimeFormat};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

pub const E1213: (&str, &str) = (
    "moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds",
    "3895fbca8bb036c1b31a9d6b68dea69eca6892fe0a6bce7409ab76311cfd470d",
);
pub const OPENER: (&str, &str) = (
    "opener_sample_app.eds",
    "0e0da5ddd62f571ea319dc29254be08cac5134236b389174983d80f0e370760f",
);

fn corpus() -> Option<PathBuf> {
    std::env::var_os("HELMSMAN_EDS_CORPUS").map(PathBuf::from)
}

/// The file's text, if present, after checking its hash.
fn load(dir: &Path, (name, sha): (&str, &str)) -> Option<String> {
    let bytes = std::fs::read(dir.join(name)).ok()?;
    let got = Sha256::digest(&bytes).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    assert_eq!(got, sha, "{name}: not the file the citations are to");
    Some(String::from_utf8(bytes).expect("ASCII"))
}

#[test]
fn the_e1213_eds_resolves_to_the_built_in_profile() {
    let Some(dir) = corpus() else {
        eprintln!("HELMSMAN_EDS_CORPUS not set: nothing to check");
        return;
    };
    let Some(text) = load(&dir, E1213) else {
        eprintln!("{} not in the corpus", E1213.0);
        return;
    };
    let eds = Eds::parse(&text).unwrap();
    for c in [Connection::ExclusiveOwner, Connection::InputOnly] {
        let mut spec = Spec::from_eds(&eds, c).unwrap();
        let profile = Spec::moxa_e1213(c);
        spec.di_channels = profile.di_channels;
        spec.do_channels = profile.do_channels;
        assert_eq!(spec, profile, "{c:?}");
    }
    let mut from_eds = DeviceConfig::from_eds(&eds).unwrap();
    let profile = DeviceConfig::moxa_e1213();
    from_eds.identity.serial_number = profile.identity.serial_number;
    from_eds.max_connections = profile.max_connections;
    from_eds.connection_id_base = profile.connection_id_base;
    assert_eq!(from_eds, profile);

    // The lines the device doc cites.
    let c = eds.connections().unwrap();
    assert_eq!((c[0].line, c[1].line), (484, 529));
    assert_eq!(eds.assembly(101).unwrap().line, 460);
    assert_eq!(eds.section("Device").unwrap().get("ProdCode").unwrap().line, 18);
    // Rewriting it loses nothing that resolves.
    let again = Eds::parse(&eds.to_text()).unwrap();
    assert_eq!(again.identity().unwrap(), eds.identity().unwrap());
    assert_eq!(again.assemblies().unwrap().len(), 2);
}

#[test]
fn the_opener_eds_resolves_to_the_sample_layout() {
    let Some(dir) = corpus() else {
        eprintln!("HELMSMAN_EDS_CORPUS not set: nothing to check");
        return;
    };
    let Some(text) = load(&dir, OPENER) else {
        eprintln!("{} not in the corpus", OPENER.0);
        return;
    };
    let eds = Eds::parse(&text).unwrap();
    let c = eds.connections().unwrap();
    let got: Vec<_> = c
        .iter()
        .map(|c| {
            (
                c.app_types.clone(),
                c.configuration,
                c.o_to_t_point,
                c.t_to_o_point,
                c.o_to_t.size,
                c.t_to_o.size,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (vec![AppType::ExclusiveOwner], 151, 150, 100, 32, 32),
            (vec![AppType::InputOnly], 151, 152, 100, 0, 32),
            (vec![AppType::ListenOnly], 151, 153, 100, 0, 32),
        ]
    );
    assert_eq!(c[0].o_to_t.format, RealTimeFormat::RunIdle);
    assert_eq!(c[0].t_to_o.format, RealTimeFormat::Modeless);
    assert_eq!(c[1].o_to_t.format, RealTimeFormat::Heartbeat);
    assert_eq!(c[0].config_size, 10);
    let sample = DeviceConfig::opener_sample();
    let points: Vec<_> = c
        .iter()
        .map(|c| (c.configuration, c.o_to_t_point, c.t_to_o_point))
        .collect();
    let want: Vec<_> = sample
        .points
        .iter()
        .map(|p| (p.configuration, p.consumed, p.produced))
        .collect();
    assert_eq!(points, want);
    // Its exclusive owner sends 10 bytes of configuration, which neither
    // the driver nor a target built from an EDS does yet.
    let e = Spec::from_eds(&eds, Connection::ExclusiveOwner).unwrap_err();
    assert!(e.contains("configuration data"), "{e}");
    Spec::from_eds(&eds, Connection::InputOnly).unwrap();
}
