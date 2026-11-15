//! The IODD reader against a description written for this test (the
//! structures of `IODD1.1.xsd` it reads, and the ways a file can be
//! wrong), against arbitrary text, and, when a directory of real IODDs
//! is given, against those.

use helmsman_iolink::iodd::{Condition, Field, Kind, parse};
use helmsman_iolink::pd;
use proptest::prelude::*;

/// An IODD cut to what the reader looks at, with `body` as the content
/// of `DeviceFunction` and `texts` in the primary language.
fn iodd(body: &str, texts: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- a test description, not a real device -->
<IODevice xmlns="http://www.io-link.com/IODD/2010/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <DocumentInfo version="V1.0" releaseDate="2026-11-14" copyright="none"/>
  <ProfileBody>
    <DeviceIdentity vendorId="310" vendorName="Bench &amp; Co" deviceId="666">
      <VendorText textId="TI_VendorText"/>
      <DeviceName textId="TI_DeviceName"/>
    </DeviceIdentity>
    <DeviceFunction>{body}</DeviceFunction>
  </ProfileBody>
  <ExternalTextCollection>
    <PrimaryLanguage xml:lang="en">
      <Text id="TI_DeviceName" value="Distance sensor"/>
      {texts}
    </PrimaryLanguage>
    <Language xml:lang="de"><Text id="TI_DeviceName" value="Abstandssensor"/></Language>
  </ExternalTextCollection>
</IODevice>"#
    )
}

const RECORD: &str = r#"
<DatatypeCollection>
  <Datatype id="D_Switch" xsi:type="BooleanT"/>
  <Datatype id="D_In" xsi:type="RecordT" bitLength="32" subindexAccessSupported="false">
    <RecordItem subindex="3" bitOffset="0"><DatatypeRef datatypeId="D_Switch"/><Name textId="TN_Sw1"/></RecordItem>
    <RecordItem subindex="1" bitOffset="16">
      <SimpleDatatype xsi:type="IntegerT" bitLength="16"><ValueRange lowerValue="-10000" upperValue="10000"/></SimpleDatatype>
      <Name textId="TN_Distance"/>
    </RecordItem>
    <RecordItem subindex="2" bitOffset="8"><SimpleDatatype xsi:type="UIntegerT" bitLength="8"/><Name textId="TN_Quality"/></RecordItem>
    <RecordItem subindex="4" bitOffset="1"><DatatypeRef datatypeId="D_Switch"/><Name textId="TN_Sw2"/></RecordItem>
  </Datatype>
</DatatypeCollection>
<ProcessDataCollection>
  <ProcessData id="P_PD">
    <ProcessDataIn id="PI_In" bitLength="32"><DatatypeRef datatypeId="D_In"/><Name textId="TN_In"/></ProcessDataIn>
    <ProcessDataOut id="PO_Out" bitLength="8">
      <Datatype xsi:type="UIntegerT" bitLength="8"/>
      <Name textId="TN_Out"/>
    </ProcessDataOut>
  </ProcessData>
</ProcessDataCollection>"#;

const TEXTS: &str = r#"
<Text id="TN_In" value="Measurement"/>
<Text id="TN_Out" value="Teach"/>
<Text id="TN_Distance" value="Distance"/>
<Text id="TN_Quality" value="Signal quality"/>
<Text id="TN_Sw1" value="Switch 1"/>
"#;

fn field(name: &str, subindex: u8, bit_offset: u16, bit_length: u16, kind: Kind) -> Field {
    Field {
        name: name.to_string(),
        subindex,
        bit_offset,
        bit_length,
        kind,
    }
}

#[test]
fn a_record_is_laid_out_by_name() {
    let got = parse(&iodd(RECORD, TEXTS)).unwrap();
    assert_eq!((got.vendor_id, got.device_id), (310, 666));
    assert_eq!(got.vendor_name, "Bench & Co");
    assert_eq!(got.device_name, "Distance sensor");
    assert_eq!(got.process_data.len(), 1);
    let pd = &got.process_data[0];
    assert_eq!(pd.id, "P_PD");
    assert_eq!(pd.condition, None);
    let input = pd.input.as_ref().unwrap();
    assert_eq!((input.id.as_str(), input.name.as_str()), ("PI_In", "Measurement"));
    assert_eq!((input.bit_length, input.page_bits()), (32, 32));
    assert_eq!(
        input.fields,
        [
            field("Distance", 1, 16, 16, Kind::Integer),
            field("Signal quality", 2, 8, 8, Kind::UInteger),
            field("Switch 1", 3, 0, 1, Kind::Boolean),
            // No text in the primary language: the reference stands in.
            field("TN_Sw2", 4, 1, 1, Kind::Boolean),
        ]
    );
    let output = pd.output.as_ref().unwrap();
    assert_eq!(output.fields, [field("Teach", 0, 0, 8, Kind::UInteger)]);
    assert_eq!(output.page_bits(), 8);

    // The layout applied to four octets off the wire.
    let octets = [0xFF, 0x38, 0x5A, 0x02];
    let read = |f: &Field| pd::get(&octets, f.bit_offset.into(), f.bit_length.into());
    assert_eq!(pd::get_signed(&octets, 16, 16), Some(-200));
    assert_eq!(read(&input.fields[1]), Some(0x5A));
    assert_eq!(read(&input.fields[2]), Some(0));
    assert_eq!(read(&input.fields[3]), Some(1));
}

#[test]
fn an_array_packs_its_highest_subindex_at_the_right() {
    let body = r#"
<ProcessDataCollection><ProcessData id="P">
  <ProcessDataIn id="PI" bitLength="15">
    <Datatype xsi:type="ArrayT" count="5"><SimpleDatatype xsi:type="IntegerT" bitLength="3"/></Datatype>
    <Name textId="TN_In"/>
  </ProcessDataIn>
</ProcessData></ProcessDataCollection>"#;
    let got = parse(&iodd(body, TEXTS)).unwrap();
    let input = got.process_data[0].input.as_ref().unwrap();
    let offsets: Vec<_> = input
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.subindex, f.bit_offset))
        .collect();
    // IOL Table F.18: offsets 12, 9, 6, 3, 0 for subindex 1 to 5.
    assert_eq!(
        offsets,
        [
            ("Measurement 1", 1, 12),
            ("Measurement 2", 2, 9),
            ("Measurement 3", 3, 6),
            ("Measurement 4", 4, 3),
            ("Measurement 5", 5, 0)
        ]
    );
    assert_eq!(got.process_data[0].output, None);
    assert_eq!(input.page_bits(), 15);
}

#[test]
fn the_fixed_lengths_and_the_page_length_follow_the_specifications() {
    let item = |ty: &str, bits: u16| {
        format!(
            r#"<ProcessDataCollection><ProcessData id="P">
              <ProcessDataIn id="PI" bitLength="{bits}"><Datatype xsi:type="{ty}/><Name textId="TN_In"/></ProcessDataIn>
            </ProcessData></ProcessDataCollection>"#
        )
    };
    for (ty, bits, kind, page_bits) in [
        (r#"Float32T""#, 32, Kind::Float32, 32),
        (r#"TimeT""#, 64, Kind::Time, 64),
        (r#"TimeSpanT""#, 64, Kind::TimeSpan, 64),
        (
            r#"StringT" fixedLength="3" encoding="UTF-8""#,
            24,
            Kind::String,
            24,
        ),
        (r#"OctetStringT" fixedLength="32""#, 256, Kind::OctetString, 256),
        (r#"UIntegerT" bitLength="17""#, 17, Kind::UInteger, 24),
        (r#"BooleanT""#, 1, Kind::Boolean, 1),
    ] {
        let got = parse(&iodd(&item(ty, bits), TEXTS)).unwrap_or_else(|e| panic!("{ty}: {e}"));
        let input = got.process_data[0].input.as_ref().unwrap();
        assert_eq!(input.fields, [field("Measurement", 0, 0, bits, kind)], "{ty}");
        assert_eq!(input.page_bits(), page_bits, "{ty}");
    }
}

#[test]
fn conditional_process_data_keeps_each_layout_with_its_condition() {
    let body = r#"
<ProcessDataCollection>
  <ProcessData id="P0"><Condition variableId="V_Select" value="0"/>
    <ProcessDataIn id="PI0" bitLength="8"><Datatype xsi:type="UIntegerT" bitLength="8"/><Name textId="TN_In"/></ProcessDataIn>
  </ProcessData>
  <ProcessData id="P1"><Condition variableId="V_Select" subindex="2" value="255"/>
    <ProcessDataIn id="PI1" bitLength="8"><Datatype xsi:type="IntegerT" bitLength="8"/><Name textId="TN_In"/></ProcessDataIn>
  </ProcessData>
</ProcessDataCollection>"#;
    let got = parse(&iodd(body, TEXTS)).unwrap();
    let conditions: Vec<_> = got.process_data.iter().map(|pd| pd.condition.clone()).collect();
    assert_eq!(
        conditions,
        [
            Some(Condition {
                variable: "V_Select".to_string(),
                subindex: None,
                value: 0
            }),
            Some(Condition {
                variable: "V_Select".to_string(),
                subindex: Some(2),
                value: 255
            }),
        ]
    );
    let without = body.replace(r#"<Condition variableId="V_Select" value="0"/>"#, "");
    let e = parse(&iodd(&without, TEXTS)).unwrap_err();
    assert!(e.msg.contains("without a Condition"), "{e}");
}

/// Each change makes the description one a controller cannot lay out,
/// and the message says which part.
#[test]
fn a_wrong_description_is_refused_with_its_reason() {
    let cases: &[(&str, &str, &str)] = &[
        (
            r#"bitOffset="8"><SimpleDatatype xsi:type="UIntegerT" bitLength="8"/>"#,
            r#"bitOffset="12"><SimpleDatatype xsi:type="UIntegerT" bitLength="8"/>"#,
            "shares bits",
        ),
        (
            r#"<RecordItem subindex="1" bitOffset="16">"#,
            r#"<RecordItem subindex="1" bitOffset="17">"#,
            "ends at bit 33 of 32",
        ),
        (
            r#"<ProcessDataIn id="PI_In" bitLength="32">"#,
            r#"<ProcessDataIn id="PI_In" bitLength="24">"#,
            "a record of 32 bits as 24",
        ),
        (
            r#"<ProcessDataIn id="PI_In" bitLength="32">"#,
            r#"<ProcessDataIn id="PI_In" bitLength="257">"#,
            "not a number from 1 to 256",
        ),
        (
            r#"<ProcessDataIn id="PI_In" bitLength="32">"#,
            r#"<ProcessDataIn id="PI_In" bitLength="-1">"#,
            "not a number",
        ),
        (
            r#"<ProcessDataIn id="PI_In" bitLength="32">"#,
            r#"<ProcessDataIn id="PI_In">"#,
            "no bitLength=",
        ),
        (
            r#"<DatatypeRef datatypeId="D_In"/>"#,
            r#"<DatatypeRef datatypeId="D_Gone"/>"#,
            "no Datatype with id \"D_Gone\"",
        ),
        (r#"<DatatypeRef datatypeId="D_In"/>"#, "", "has no data type"),
        (
            r#"<Datatype id="D_Switch" xsi:type="BooleanT"/>"#,
            r#"<Datatype id="D_Switch" xsi:type="RecordT" bitLength="1"/>"#,
            "where a simple type is needed",
        ),
        (
            r#"<Datatype xsi:type="UIntegerT" bitLength="8"/>"#,
            r#"<Datatype xsi:type="ProcessDataOutUnionT"/>"#,
            "where a simple type is needed",
        ),
        (
            r#"<Datatype xsi:type="UIntegerT" bitLength="8"/>"#,
            r#"<Datatype xsi:type="UIntegerT" bitLength="65"/>"#,
            "not a number from 2 to 64",
        ),
        (
            r#"<Datatype xsi:type="UIntegerT" bitLength="8"/>"#,
            r#"<Datatype xsi:type="UIntegerT" bitLength="16"/>"#,
            "ends at bit 16 of 8",
        ),
        (
            r#"<Datatype xsi:type="UIntegerT" bitLength="8"/>"#,
            r#"<Datatype bitLength="8"/>"#,
            "no type=",
        ),
        (r#"<Name textId="TN_Out"/>"#, "", "has no <Name>"),
        (
            r#"vendorId="310""#,
            r#"vendorId="0""#,
            "not a number from 1 to 65535",
        ),
        (
            r#"deviceId="666""#,
            r#"deviceId="16777216""#,
            "not a number from 1 to 16777215",
        ),
        (
            "<ProcessDataCollection>",
            "<ProcessDataCollection><!--",
            "comment",
        ),
        (
            "<IODevice ",
            "<!DOCTYPE x [<!ENTITY a \"aaaa\">]><IODevice ",
            "DOCTYPE",
        ),
        ("IODevice", "IODeviceX", "not an IODD"),
        ("<PrimaryLanguage", "<SecondaryLanguage", "PrimaryLanguage"),
    ];
    let good = iodd(RECORD, TEXTS);
    for (from, to, why) in cases {
        assert!(good.contains(from), "the test's own text lacks {from}");
        let e = parse(&good.replace(from, to)).unwrap_err();
        assert!(
            e.msg.to_lowercase().contains(&why.to_lowercase()),
            "{from} -> {to}: expected \"{why}\", got \"{e}\""
        );
        assert!(e.msg.len() < 300, "a message of {} bytes", e.msg.len());
    }
}

proptest! {
    /// Any text is answered without a panic.
    #[test]
    fn any_text_is_answered(text in ".{0,400}") {
        let _ = parse(&text);
        let _ = parse(&iodd(&text, &text));
    }

    /// The good description cut anywhere, or with one character
    /// replaced, is read or refused, never a panic, and an error keeps
    /// to a line of the file.
    #[test]
    fn a_damaged_description_is_answered(cut in any::<prop::sample::Index>(), with in any::<char>()) {
        let good = iodd(RECORD, TEXTS);
        let chars: Vec<char> = good.chars().collect();
        let at = cut.index(chars.len());
        let cut_short: String = chars[..at].iter().collect();
        prop_assert!(parse(&cut_short).is_err());
        let mut replaced = chars.clone();
        replaced[at] = with;
        if let Err(e) = parse(&replaced.iter().collect::<String>()) {
            prop_assert!(e.pos.line as usize <= good.lines().count() + 1, "{}", e);
        }
    }
}

/// Real IODDs, fetched and not committed (docs/SOURCES.md): every
/// `*-IODD1.1.xml` under `HELMSMAN_IODD_DIR` is read, and the IO-Link
/// Community's example 17 has the layout its file states.
///
/// `HELMSMAN_IODD_DIR=.../Examples cargo test -p helmsman-iolink --test iodd -- --ignored --nocapture`
#[test]
#[ignore = "needs HELMSMAN_IODD_DIR, a directory of IODD files"]
fn real_iodds_are_read() {
    let dir = std::env::var("HELMSMAN_IODD_DIR").expect("HELMSMAN_IODD_DIR");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("IODD1.1.xml"))
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no *IODD1.1.xml in {dir}");
    let mut failed = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match parse(&std::fs::read_to_string(path).unwrap()) {
            Ok(got) => {
                let items = got
                    .process_data
                    .iter()
                    .flat_map(|pd| pd.input.iter().chain(&pd.output))
                    .map(|item| format!("{}:{}b/{}", item.id, item.bit_length, item.fields.len()))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!(
                    "{name}: {}/{} {} [{items}]",
                    got.vendor_id, got.device_id, got.device_name
                );
                if name.contains("-17-ComplexProcessData") {
                    let input = got.process_data[0].input.as_ref().unwrap();
                    let layout: Vec<_> = input
                        .fields
                        .iter()
                        .map(|f| (f.subindex, f.bit_offset, f.bit_length, f.kind))
                        .collect();
                    assert_eq!(
                        layout,
                        [
                            (1, 16, 16, Kind::Integer),
                            (2, 8, 8, Kind::Integer),
                            (3, 0, 1, Kind::Boolean),
                            (4, 1, 1, Kind::Boolean)
                        ]
                    );
                    assert_eq!((got.vendor_id, got.device_id), (65535, 17));
                }
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} not read:\n{}",
        failed.len(),
        paths.len(),
        failed.join("\n")
    );
}
