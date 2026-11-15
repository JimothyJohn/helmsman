//! Properties of the codec over every layout and every octet: what is
//! built reads back, and a damaged message is never accepted.

use helmsman_iolink::mseq::{Error, Layout, MAX_MESSAGE, MasterMessage, Mc, Reply};
use helmsman_iolink::page::{ProcessData, cycle_time, cycle_time_octet};
use proptest::prelude::*;

fn layouts() -> impl Strategy<Value = Layout> {
    let od = prop::sample::select(vec![1u8, 2, 8, 32]);
    prop_oneof![
        Just(Layout::TYPE_0),
        prop::sample::select(vec![2u8, 8, 32]).prop_map(|od| Layout::type1(od).unwrap()),
        (0u8..=32, od, 0u8..=32)
            .prop_filter_map("no Process Data", |(out, od, inp)| Layout::type2(out, od, inp)),
    ]
}

/// A layout, a control octet and enough octets for any part.
fn exchange() -> impl Strategy<Value = (Layout, u8, Vec<u8>)> {
    (layouts(), any::<u8>(), prop::collection::vec(any::<u8>(), 64))
}

fn master(layout: Layout, mc: u8, data: &[u8], out: &mut [u8]) -> usize {
    let mc = Mc::from_octet(mc);
    let (pd_out, rest) = data.split_at(layout.pd_out());
    let od = if mc.read() { &[][..] } else { &rest[..layout.od()] };
    MasterMessage { mc, pd_out, od }.encode(layout, out).unwrap()
}

fn reply(layout: Layout, read: bool, flags: (bool, bool), data: &[u8], out: &mut [u8]) -> usize {
    let (od, rest) = data.split_at(if read { layout.od() } else { 0 });
    Reply {
        od,
        pd_in: &rest[..layout.pd_in()],
        pd_valid: flags.0,
        event: flags.1,
    }
    .encode(layout, read, out)
    .unwrap()
}

proptest! {
    #[test]
    fn a_control_octet_reads_back(octet in any::<u8>()) {
        prop_assert_eq!(Mc::from_octet(octet).octet(), octet);
    }

    #[test]
    fn a_master_message_reads_back((layout, mc, data) in exchange()) {
        let mut out = [0; MAX_MESSAGE];
        let len = master(layout, mc, &data, &mut out);
        let read = mc & 0x80 != 0;
        prop_assert_eq!(len, layout.master_len(read));
        let got = MasterMessage::decode(layout, &out[..len]).unwrap();
        prop_assert_eq!(got.mc.octet(), mc);
        prop_assert_eq!(got.pd_out, &data[..layout.pd_out()]);
        let od = if read { 0 } else { layout.od() };
        prop_assert_eq!(got.od, &data[layout.pd_out()..layout.pd_out() + od]);
    }

    #[test]
    fn a_reply_reads_back(
        (layout, _, data) in exchange(),
        read in any::<bool>(),
        flags in any::<(bool, bool)>(),
    ) {
        let mut out = [0; MAX_MESSAGE];
        let len = reply(layout, read, flags, &data, &mut out);
        prop_assert_eq!(len, layout.reply_len(read));
        let got = Reply::decode(layout, read, &out[..len]).unwrap();
        prop_assert_eq!((got.pd_valid, got.event), flags);
        let od = if read { layout.od() } else { 0 };
        prop_assert_eq!(got.od, &data[..od]);
        prop_assert_eq!(got.pd_in, &data[od..od + layout.pd_in()]);
    }

    /// One flipped bit anywhere changes the checksum (each bit of the sum
    /// feeds two of the six), or the length or type the message claims.
    #[test]
    fn a_master_message_with_one_bit_flipped_is_refused(
        (layout, mc, data) in exchange(),
        at in any::<prop::sample::Index>(),
        bit in 0u8..8,
    ) {
        let mut out = [0; MAX_MESSAGE];
        let len = master(layout, mc, &data, &mut out);
        out[at.index(len)] ^= 1 << bit;
        prop_assert!(MasterMessage::decode(layout, &out[..len]).is_err());
    }

    #[test]
    fn a_reply_with_one_bit_flipped_is_refused(
        (layout, _, data) in exchange(),
        read in any::<bool>(),
        flags in any::<(bool, bool)>(),
        at in any::<prop::sample::Index>(),
        bit in 0u8..8,
    ) {
        let mut out = [0; MAX_MESSAGE];
        let len = reply(layout, read, flags, &data, &mut out);
        out[at.index(len)] ^= 1 << bit;
        let refused = matches!(
            Reply::decode(layout, read, &out[..len]),
            Err(Error::Checksum { .. })
        );
        prop_assert!(refused);
    }

    /// Whatever arrives, a decoder answers and never panics; a reply cut
    /// short or run long is a length error.
    #[test]
    fn any_octets_are_answered(
        layout in layouts(),
        read in any::<bool>(),
        octets in prop::collection::vec(any::<u8>(), 0..80),
    ) {
        let _ = MasterMessage::decode(layout, &octets);
        let got = Reply::decode(layout, read, &octets);
        if octets.len() != layout.reply_len(read) {
            prop_assert!(
                matches!(got, Err(Error::Length { .. })),
                "{} octets were read as a {}-octet reply", octets.len(), layout.reply_len(read)
            );
        }
    }

    /// The octet chosen for a cycle time is never shorter than asked, and
    /// a time the table has is kept.
    #[test]
    fn a_cycle_time_is_rounded_up_into_the_table(tenths in 0u16..=1328) {
        let got = cycle_time(cycle_time_octet(tenths)).unwrap();
        prop_assert!(got >= tenths.max(4));
        prop_assert!(got - tenths.max(4) < 16);
    }

    #[test]
    fn a_table_cycle_time_is_kept(octet in 4u8..0xC0) {
        prop_assert_eq!(cycle_time_octet(cycle_time(octet).unwrap()), octet);
    }

    #[test]
    fn process_data_is_at_most_32_octets(octet in any::<u8>()) {
        if let Ok(pd) = ProcessData::decode(octet) {
            prop_assert!(pd.octets() <= 32);
            prop_assert!(pd.bits == 0 || pd.octets() > 0);
        }
    }
}
