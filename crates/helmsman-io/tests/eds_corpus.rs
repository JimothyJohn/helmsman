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

pub const MDRIVE: (&str, &str) = (
    "SEM_MDrivever1_2.eds",
    "b14f0fe268df5b37e9dcee6bd0288a88e59d428dbe6044b49dba8beaaeb86041",
);

pub const IMPACT67: (&str, &str) = (
    "55089_Impact67_E_DIO16.eds",
    "6eb94568f8b5fc73420611ac1633073da2c6e8cda75d81593bd73631807bef3d",
);
pub const IMPACT67_OMRON: (&str, &str) = (
    "55089_Impact67_E_DIO16_Omron.eds",
    "2320dbc1dded11586c969412e8e0e11182c010325a598af4bd823fc4c1c1f225",
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
    // Its exclusive owner sends 10 bytes of configuration: the EDS's
    // defaults when its configuration assembly spells them out, else
    // `config_data=` has to bring them.
    let spec = Spec::from_eds(&eds, Connection::ExclusiveOwner).unwrap();
    match spec.config_wanted {
        Some(n) => {
            assert_eq!(n, 10);
            let e = spec.clone().with_config_data(None).unwrap_err();
            assert!(e.contains("10 bytes of configuration data"), "{e}");
            assert_eq!(
                spec.with_config_data(Some(&[0; 10])).unwrap().config_data,
                [0; 10]
            );
        }
        None => assert_eq!(spec.config_data.len(), 10),
    }
    Spec::from_eds(&eds, Connection::InputOnly).unwrap();
}

/// Novanta's own file resolves to the connection the drive on the bench
/// took on 2026-11-16 (docs/devices/novanta-mdrive.md), and to the same
/// one as the fixture that restates it for the other tests.
#[test]
fn the_mdrive_eds_resolves_as_its_facts_fixture_does() {
    let Some(dir) = corpus() else {
        eprintln!("HELMSMAN_EDS_CORPUS not set: nothing to check");
        return;
    };
    let Some(text) = load(&dir, MDRIVE) else {
        eprintln!("{} not in the corpus", MDRIVE.0);
        return;
    };
    let eds = Eds::parse(&text).unwrap();
    let facts = Eds::parse(include_str!(
        "../../helmsman-proto/tests/fixtures/mdrive-facts.eds"
    ))
    .unwrap();
    let spec = Spec::from_eds(&eds, Connection::ExclusiveOwner).unwrap();
    assert_eq!(spec, Spec::from_eds(&facts, Connection::ExclusiveOwner).unwrap());
    assert_eq!(
        (
            spec.configuration,
            spec.o_to_t,
            spec.t_to_o,
            spec.o_to_t_len,
            spec.t_to_o_len
        ),
        (1, 112, 100, 36, 16)
    );
    // What the file says; the drive takes a run/idle header O->T, which
    // the driver learns from the refusal.
    assert!(!spec.o_to_t_run_idle && !spec.t_to_o_run_idle);
    assert_eq!((spec.rpi_min, spec.rpi_max, spec.rpi_default), (None, None, None));
    assert!(spec.config_data.is_empty() && spec.config_wanted.is_none());
    let id = spec.identity.unwrap();
    assert_eq!((id.vendor_id, id.device_type, id.product_code), (243, 43, 15362));
    assert_eq!(Spec::from_eds(&eds, Connection::InputOnly).unwrap().o_to_t, 128);
}

/// Murrelektronik's two files for the IMPACT67 E DIO8 resolve to the
/// connection the unit on the bench took from each on 2026-11-17
/// (docs/devices/murrelektronik-impact67-e.md): the first exclusive
/// owner, configuration instance 109, with the file's parameter defaults
/// as configuration data, three bytes in one file and four in the other.
#[test]
fn the_impact67_eds_files_resolve_with_their_configuration_defaults() {
    let Some(dir) = corpus() else {
        eprintln!("HELMSMAN_EDS_CORPUS not set: nothing to check");
        return;
    };
    for (file, config_bytes) in [(IMPACT67, 3usize), (IMPACT67_OMRON, 4)] {
        let Some(text) = load(&dir, file) else {
            eprintln!("{} not in the corpus", file.0);
            continue;
        };
        let eds = Eds::parse(&text).unwrap();
        let id = eds.identity().unwrap();
        assert_eq!((id.vendor_id, id.device_type, id.product_code), (640, 7, 55089));
        assert_eq!((id.major_revision, id.minor_revision), (1, 2));
        let connections = eds.connections().unwrap();
        assert_eq!(connections.len(), 6, "{}", file.0);
        let spec = Spec::from_eds(&eds, Connection::ExclusiveOwner).unwrap();
        assert_eq!(
            (
                spec.configuration,
                spec.o_to_t,
                spec.t_to_o,
                spec.o_to_t_len,
                spec.t_to_o_len
            ),
            (109, 35, 5, 2, 2),
            "{}",
            file.0
        );
        assert!(spec.o_to_t_run_idle && !spec.t_to_o_run_idle, "{}", file.0);
        assert_eq!(spec.config_data, vec![0; config_bytes], "{}", file.0);
        assert_eq!(spec.config_wanted, None);
        // The map's own setup must be the file's size.
        let own = vec![0xFF; config_bytes];
        assert_eq!(
            spec.clone().with_config_data(Some(&own)).unwrap().config_data,
            own
        );
        let e = spec.with_config_data(Some(&[0; 7])).unwrap_err();
        assert!(e.contains("7 bytes"), "{e}");
    }
}
