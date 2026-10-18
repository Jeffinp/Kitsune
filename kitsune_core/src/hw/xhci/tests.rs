use super::context::*;
use super::ring::*;
use super::trb::*;
use super::*;

// ---- capability registers ----

/// A QEMU `qemu-xhci`-like controller: 1.0, CAPLENGTH 0x40, 64 slots, 16 interrupters, 8 ports.
fn qemu_caps() -> Capabilities {
    Capabilities::decode(
        0x0100_0040,
        0x0800_1040, // ports 8, interrupters 16, slots 64
        0x0000_0000,
        0x0000_0001, // AC64, no extended capabilities
        0x0000_2000,
        0x0000_1000,
    )
    .unwrap()
}

#[test]
fn capabilities_decode_the_fields() {
    let c = qemu_caps();
    assert_eq!(c.cap_length, 0x40);
    assert_eq!(c.version, 0x0100);
    assert_eq!(c.max_slots, 64);
    assert_eq!(c.max_interrupters, 16);
    assert_eq!(c.max_ports, 8);
    assert!(c.addresses_64);
    assert!(!c.context_64);
    assert_eq!(c.context_size(), 32);
    assert_eq!(c.doorbell_offset, 0x2000);
    assert_eq!(c.runtime_offset, 0x1000);
}

#[test]
fn capabilities_scratchpad_count_joins_the_two_fields() {
    // high part 1 (bits 25:21) and low part 3 (bits 31:27): 1 * 32 + 3
    let hcsparams2 = 1 << 21 | 3 << 27;
    let c = Capabilities::decode(0x0100_0020, 0x0100_0101, hcsparams2, 0, 0, 0).unwrap();
    assert_eq!(c.scratchpad_buffers, 35);
}

#[test]
fn capabilities_context_size_64_and_ext_caps_offset() {
    let hccparams1 = 0x0140_0000 | 0b100; // xECP = 0x140 dwords, CSZ
    let c = Capabilities::decode(0x0100_0020, 0x0100_0101, 0, hccparams1, 0, 0).unwrap();
    assert!(c.context_64);
    assert_eq!(c.context_size(), 64);
    assert_eq!(c.ext_caps_offset, 0x500);
}

#[test]
fn capabilities_reject_a_window_that_is_not_a_controller() {
    assert_eq!(Capabilities::decode(u32::MAX, 0, 0, 0, 0, 0), None); // nothing behind the BAR
    assert_eq!(
        Capabilities::decode(0x0100_0008, 0x0100_0101, 0, 0, 0, 0),
        None
    ); // CAPLENGTH too small
    assert_eq!(
        Capabilities::decode(0x0100_0020, 0x0100_0100, 0, 0, 0, 0),
        None
    ); // no slots
    assert_eq!(
        Capabilities::decode(0x0100_0020, 0x0001_0001, 0, 0, 0, 0),
        None
    ); // no ports
}

#[test]
fn register_offsets() {
    let c = qemu_caps();
    assert_eq!(c.doorbell(0), 0x2000);
    assert_eq!(c.doorbell(5), 0x2014);
    assert_eq!(c.interrupter(0), 0x1020);
    assert_eq!(c.interrupter(1), 0x1040);
    assert_eq!(c.portsc(1), 0x40 + 0x400);
    assert_eq!(c.portsc(3), 0x40 + 0x400 + 0x20);
}

#[test]
fn dcbaa_has_one_entry_per_slot_plus_the_scratchpad_pointer() {
    assert_eq!(dcbaa_bytes(64), 65 * 8);
    assert_eq!(dcbaa_bytes(1), 16);
}

#[test]
fn erst_entry_layout() {
    assert_eq!(erst_entry(0x1_2345_6000, 256), [0x2345_6000, 0x1, 256, 0]);
}

// ---- ports ----

#[test]
fn portsc_neutral_never_writes_a_write_one_to_clear_bit() {
    // connected, enabled, powered, high speed, connect change pending
    let read = PORTSC_CCS | PORTSC_PED | PORTSC_PP | 3 << 10 | PORTSC_CSC;
    let w = portsc_neutral(read);
    assert_eq!(w & PORTSC_PED, 0, "writing 1 to PED would disable the port");
    assert_eq!(
        w & PORTSC_CHANGES,
        0,
        "writing 1 to a change bit would lose it"
    );
    assert_eq!(w & PORTSC_PP, PORTSC_PP, "power must be kept");
    assert_eq!((w >> 10) & 0xF, 3);
}

#[test]
fn portsc_neutral_matches_the_linux_masks() {
    // RO (0, 3, 10:13, 30) and RWS (5:8, 9, 14:15, 25:27) survive; everything else, CAS (24) included, is zero
    let keep = 1 | 1 << 3 | 0xF << 10 | 1 << 30 | 0xF << 5 | 1 << 9 | 3 << 14 | 7 << 25;
    assert_eq!(portsc_neutral(u32::MAX), keep);
    assert_eq!(portsc_neutral(PORTSC_CAS), 0);
}

#[test]
fn portsc_reset_sets_only_pr_on_top_of_neutral() {
    let read = PORTSC_CCS | PORTSC_PP | PORTSC_PRC;
    assert_eq!(portsc_reset(read), PORTSC_CCS | PORTSC_PP | PORTSC_PR);
}

#[test]
fn portsc_ack_clears_only_the_requested_change_bits() {
    let read = PORTSC_CCS | PORTSC_PP | PORTSC_CSC | PORTSC_PRC;
    assert_eq!(
        portsc_ack(read, PORTSC_PRC),
        PORTSC_CCS | PORTSC_PP | PORTSC_PRC
    );
    // asking to ack a non-change bit (PED) must not disable the port
    assert_eq!(portsc_ack(read, PORTSC_PED) & PORTSC_PED, 0);
}

#[test]
fn speed_from_portsc() {
    assert_eq!(Speed::from_portsc(1 << 10), Some(Speed::Full));
    assert_eq!(Speed::from_portsc(2 << 10), Some(Speed::Low));
    assert_eq!(Speed::from_portsc(3 << 10), Some(Speed::High));
    assert_eq!(Speed::from_portsc(4 << 10), Some(Speed::Super));
    assert_eq!(Speed::from_portsc(0), None);
    assert_eq!(Speed::from_portsc(5 << 10), None);
}

#[test]
fn control_endpoint_packet_size() {
    assert_eq!(Speed::Low.initial_ep0_max_packet(), 8);
    assert_eq!(Speed::Full.initial_ep0_max_packet(), 8);
    assert_eq!(Speed::High.initial_ep0_max_packet(), 64);
    assert_eq!(Speed::Super.initial_ep0_max_packet(), 512);
    assert_eq!(Speed::Super.ep0_max_packet(9), Some(512));
    assert_eq!(Speed::Super.ep0_max_packet(64), None); // super speed uses an exponent
    assert_eq!(Speed::Full.ep0_max_packet(32), Some(32));
    assert_eq!(Speed::Full.ep0_max_packet(12), None);
    assert_eq!(Speed::High.ep0_max_packet(64), Some(64));
    assert_eq!(Speed::High.ep0_max_packet(8), None);
    assert_eq!(Speed::Low.ep0_max_packet(8), Some(8));
    assert_eq!(Speed::Low.ep0_max_packet(64), None);
}

// ---- extended capabilities ----

/// Extended capability list in a fake window: (byte offset, header dword).
fn window(entries: &[(u32, u32)]) -> impl Fn(u32) -> u32 + '_ {
    move |off| {
        entries
            .iter()
            .find(|(o, _)| *o == off)
            .map_or(0, |(_, v)| *v)
    }
}

#[test]
fn find_ext_cap_follows_the_chain() {
    // at 0x500: id 2, next +4 dwords; at 0x510: id 1 (legacy support), last
    let w = window(&[(0x500, 0x0402), (0x510, 0x0001)]);
    assert_eq!(find_ext_cap(0x500, EXT_CAP_LEGACY_SUPPORT, &w), Some(0x510));
    assert_eq!(
        find_ext_cap(0x500, EXT_CAP_SUPPORTED_PROTOCOL, &w),
        Some(0x500)
    );
    assert_eq!(find_ext_cap(0x500, 0x44, &w), None);
}

#[test]
fn find_ext_cap_without_a_list_or_with_garbage() {
    assert_eq!(find_ext_cap(0, 1, |_| 0x0001), None);
    assert_eq!(find_ext_cap(0x500, 1, |_| u32::MAX), None);
}

#[test]
fn find_ext_cap_stops_on_a_list_that_never_ends() {
    // every entry has id 7 and points 1 dword ahead, forever
    let mut reads = 0;
    let found = find_ext_cap(0x500, 1, |_| {
        reads += 1;
        0x0107
    });
    assert_eq!(found, None);
    assert!(reads <= 64);
}

#[test]
fn legacy_handoff_protocol() {
    let bios_owns = 1 << 16 | 1;
    let asked = legacy_request_os_ownership(bios_owns);
    assert_eq!(asked & (1 << 24), 1 << 24);
    assert!(!legacy_handoff_done(bios_owns));
    assert!(!legacy_handoff_done(asked)); // both semaphores set: firmware has not let go yet
    assert!(legacy_handoff_done(asked & !(1 << 16)));
}

#[test]
fn legacy_smi_disable_clears_the_enables_and_acks_the_events() {
    let v = legacy_disable_smi(u32::MAX);
    assert_eq!(v & 1, 0, "the USB SMI enable bit is cleared");
    assert_eq!(
        v >> 29,
        0b111,
        "the SMI status bits are written as ones to clear them"
    );
    assert_eq!(v & (1 << 4), 0, "bits outside the kept mask are cleared");
    assert_eq!(legacy_disable_smi(0), 0xE000_0000);
    // the reserved bits the specification says to preserve (1..=3, 5..=12, 17..=19) survive
    for bit in [1, 2, 3, 5, 12, 17, 19] {
        assert_ne!(legacy_disable_smi(1 << bit) & (1 << bit), 0, "bit {bit}");
    }
}

// ---- TRBs ----

#[test]
fn trb_dwords_roundtrip() {
    let t = Trb::new(0x1122_3344_5566_7788, 0x99AA_BBCC, 0xDDEE_FF01);
    assert_eq!(
        t.to_dwords(),
        [0x5566_7788, 0x1122_3344, 0x99AA_BBCC, 0xDDEE_FF01]
    );
    assert_eq!(Trb::from_dwords(t.to_dwords()), t);
}

#[test]
fn trb_cycle_bit_and_type() {
    let t = Trb::enable_slot();
    assert_eq!(t.trb_type(), TYPE_ENABLE_SLOT);
    assert!(!t.cycle());
    let c = t.with_cycle(true);
    assert!(c.cycle());
    assert_eq!(c.trb_type(), TYPE_ENABLE_SLOT);
    assert!(!c.with_cycle(false).cycle());
}

#[test]
fn command_trbs_match_the_specification_encoding() {
    assert_eq!(Trb::enable_slot().control, 9 << 10);
    assert_eq!(Trb::disable_slot(3).control, 10 << 10 | 3 << 24);
    assert_eq!(Trb::no_op_command().control, 23 << 10);
    let a = Trb::address_device(0x1000_0040, 5, false);
    assert_eq!(a.param, 0x1000_0040);
    assert_eq!(a.control, 11 << 10 | 5 << 24);
    let b = Trb::address_device(0x1000_0040, 5, true);
    assert_eq!(b.control, 11 << 10 | 1 << 9 | 5 << 24);
    assert_eq!(
        Trb::configure_endpoint(0x2000, 2).control,
        12 << 10 | 2 << 24
    );
    assert_eq!(Trb::evaluate_context(0x2000, 2).control, 13 << 10 | 2 << 24);
    assert_eq!(
        Trb::reset_endpoint(2, 3).control,
        14 << 10 | 3 << 16 | 2 << 24
    );
}

#[test]
fn input_context_pointers_lose_their_low_bits() {
    assert_eq!(
        Trb::address_device(0x1000_0049, 1, false).param,
        0x1000_0040
    );
}

#[test]
fn link_trb_toggles_the_cycle_when_asked() {
    let l = Trb::link(0x4000, true);
    assert_eq!(l.trb_type(), TYPE_LINK);
    assert_eq!(l.param, 0x4000);
    assert_eq!(l.control & 0b10, 0b10);
    assert_eq!(Trb::link(0x4000, false).control & 0b10, 0);
}

#[test]
fn setup_packet_packs_little_endian() {
    let s = SetupPacket::get_descriptor(DESCRIPTOR_DEVICE, 0, 18);
    // bmRequestType 0x80, bRequest 6, wValue 0x0100, wIndex 0, wLength 18
    assert_eq!(s.pack(), 0x0012_0000_0100_0680);
    assert!(s.is_in());
}

#[test]
fn hid_class_requests() {
    let p = SetupPacket::hid_set_protocol(1, 0);
    assert_eq!(
        (p.request_type, p.request, p.value, p.index),
        (0x21, 0x0B, 0, 1)
    );
    assert!(!p.is_in());
    let i = SetupPacket::hid_set_idle(0, 0, 0);
    assert_eq!((i.request_type, i.request, i.value), (0x21, 0x0A, 0));
    let c = SetupPacket::set_configuration(1);
    assert_eq!((c.request_type, c.request, c.value), (0x00, 9, 1));
}

#[test]
fn control_transfer_with_data_in_has_three_stages_and_an_out_status() {
    let setup = SetupPacket::get_descriptor(DESCRIPTOR_DEVICE, 0, 18);
    let (trbs, n) = control_transfer(setup, 0x8000);
    assert_eq!(n, 3);
    assert_eq!(trbs[0].trb_type(), TYPE_SETUP_STAGE);
    assert_eq!(trbs[0].status, 8);
    assert_eq!(
        trbs[0].control & (1 << 6),
        1 << 6,
        "setup bytes are immediate data"
    );
    assert_eq!(
        (trbs[0].control >> 16) & 3,
        3,
        "transfer type: IN data stage"
    );
    assert_eq!(trbs[1].trb_type(), TYPE_DATA_STAGE);
    assert_eq!(trbs[1].param, 0x8000);
    assert_eq!(trbs[1].status, 18);
    assert_eq!(trbs[1].control & (1 << 16), 1 << 16, "data direction IN");
    assert_eq!(trbs[2].trb_type(), TYPE_STATUS_STAGE);
    assert_eq!(
        trbs[2].control & (1 << 16),
        0,
        "status runs OUT after an IN data stage"
    );
    assert_eq!(
        trbs[2].control & (1 << 5),
        1 << 5,
        "interrupt on completion"
    );
}

#[test]
fn control_transfer_without_data_has_two_stages_and_an_in_status() {
    let (trbs, n) = control_transfer(SetupPacket::set_configuration(1), 0);
    assert_eq!(n, 2);
    assert_eq!((trbs[0].control >> 16) & 3, 0, "transfer type: no data");
    assert_eq!(trbs[1].trb_type(), TYPE_STATUS_STAGE);
    assert_eq!(
        trbs[1].control & (1 << 16),
        1 << 16,
        "status runs IN when there is no data"
    );
}

#[test]
fn control_transfer_with_data_out_has_an_in_status() {
    let setup = SetupPacket {
        request_type: 0x21,
        request: 9,
        value: 0x0200,
        index: 0,
        length: 1,
    };
    let (trbs, n) = control_transfer(setup, 0x9000);
    assert_eq!(n, 3);
    assert_eq!(
        (trbs[0].control >> 16) & 3,
        2,
        "transfer type: OUT data stage"
    );
    assert_eq!(trbs[1].control & (1 << 16), 0);
    assert_eq!(trbs[2].control & (1 << 16), 1 << 16);
}

#[test]
fn normal_trb_asks_for_an_event_on_short_packets() {
    let t = Trb::normal(0xA000, 8);
    assert_eq!(t.trb_type(), TYPE_NORMAL);
    assert_eq!((t.param, t.status), (0xA000, 8));
    assert_eq!(t.control & (1 << 2), 1 << 2);
    assert_eq!(t.control & (1 << 5), 1 << 5);
}

// ---- events ----

#[test]
fn completion_codes() {
    assert_eq!(CompletionCode::from_u8(1), CompletionCode::Success);
    assert_eq!(CompletionCode::from_u8(6), CompletionCode::StallError);
    assert_eq!(CompletionCode::from_u8(13), CompletionCode::ShortPacket);
    assert_eq!(CompletionCode::from_u8(200), CompletionCode::Other(200));
    assert!(CompletionCode::Success.is_ok());
    assert!(CompletionCode::ShortPacket.is_ok());
    assert!(!CompletionCode::StallError.is_ok());
    assert!(!CompletionCode::Other(0).is_ok());
}

#[test]
fn decode_command_completion() {
    let t = Trb::new(
        0x7000,
        1 << 24,
        (TYPE_COMMAND_COMPLETION as u32) << 10 | 4 << 24 | 1,
    );
    assert_eq!(
        Event::decode(&t),
        Event::CommandCompletion {
            command_trb: 0x7000,
            code: CompletionCode::Success,
            slot: 4
        }
    );
}

#[test]
fn decode_transfer_event() {
    // short packet (13), 5 bytes not transferred, endpoint 3, slot 2
    let t = Trb::new(
        0x8010,
        13 << 24 | 5,
        (TYPE_TRANSFER_EVENT as u32) << 10 | 3 << 16 | 2 << 24 | 1,
    );
    assert_eq!(
        Event::decode(&t),
        Event::Transfer {
            trb: 0x8010,
            code: CompletionCode::ShortPacket,
            slot: 2,
            endpoint: 3,
            residual: 5
        }
    );
}

#[test]
fn decode_port_status_change() {
    // the port id is in bits 31:24 of the parameter
    let t = Trb::new(3 << 24, 1 << 24, (TYPE_PORT_STATUS_CHANGE as u32) << 10 | 1);
    assert_eq!(
        Event::decode(&t),
        Event::PortStatusChange {
            port: 3,
            code: CompletionCode::Success
        }
    );
}

#[test]
fn decode_unknown_event_type() {
    let t = Trb::new(0, 0, 37 << 10);
    assert_eq!(Event::decode(&t), Event::Other { trb_type: 37 });
}

#[test]
fn decode_never_panics_on_arbitrary_words() {
    // deterministic scramble over many bit patterns
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..10_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let _ = Event::decode(&Trb::new(x, (x >> 8) as u32, (x >> 16) as u32));
    }
}

// ---- rings ----

#[test]
fn producer_ring_rejects_bad_geometry() {
    assert!(ProducerRing::new(0x1008, 16).is_none()); // misaligned
    assert!(ProducerRing::new(0x1000, 1).is_none()); // no room for data
    assert!(ProducerRing::new(0x1000, 2).is_some());
}

#[test]
fn producer_ring_stamps_the_cycle_and_advances() {
    let mut r = ProducerRing::new(0x1000, 4).unwrap();
    assert_eq!(r.capacity(), 3);
    assert_eq!(r.bytes(), 64);
    assert_eq!(r.pointer(), 0x1000 | 1);
    let p = r.push(Trb::enable_slot());
    assert_eq!(p.index, 0);
    assert!(p.trb.cycle());
    assert!(p.link.is_none());
    assert_eq!(r.pointer(), 0x1010 | 1);
}

#[test]
fn producer_ring_wraps_through_the_link_and_toggles_the_cycle() {
    let mut r = ProducerRing::new(0x1000, 4).unwrap(); // slots 0..2 data, 3 link
    r.push(Trb::enable_slot());
    r.push(Trb::enable_slot());
    let third = r.push(Trb::enable_slot());
    assert_eq!(third.index, 2);
    let (link_index, link) = third
        .link
        .expect("the last data slot is followed by the link");
    assert_eq!(link_index, 3);
    assert_eq!(link.trb_type(), TYPE_LINK);
    assert_eq!(link.param, 0x1000, "the link points back to the start");
    assert!(
        link.cycle(),
        "the link carries the cycle of the lap that ended"
    );
    assert_eq!(link.control & 0b10, 0b10, "and toggles the cycle");
    // second lap: slot 0 again, cycle now 0
    assert_eq!(r.pointer(), 0x1000);
    let again = r.push(Trb::enable_slot());
    assert_eq!(again.index, 0);
    assert!(!again.trb.cycle());
}

#[test]
fn producer_ring_two_slots_wraps_every_push() {
    let mut r = ProducerRing::new(0x2000, 2).unwrap();
    let a = r.push(Trb::no_op_command());
    assert!(a.link.is_some());
    assert!(a.trb.cycle());
    let b = r.push(Trb::no_op_command());
    assert!(!b.trb.cycle());
    assert!(!b.link.unwrap().1.cycle());
}

#[test]
fn producer_ring_pointer_for_an_endpoint_context() {
    let r = ProducerRing::new(0x1_0000_1000, 8).unwrap();
    let p = r.pointer();
    assert_eq!(p & !0xF, 0x1_0000_1000);
    assert_eq!(p & 1, 1, "dequeue cycle state");
}

#[test]
fn event_ring_reads_only_trbs_with_the_matching_cycle() {
    let mut r = EventRing::new(0x3000, 2).unwrap();
    let fresh = Trb::new(0, 0, 0).with_cycle(true);
    let stale = Trb::new(0, 0, 0).with_cycle(false);
    assert!(r.is_new(&fresh));
    assert!(!r.is_new(&stale));
    r.advance();
    assert_eq!(r.dequeue_index(), 1);
    assert_eq!(r.pointer(), 0x3010);
    r.advance(); // wraps: the cycle state flips
    assert_eq!(r.dequeue_index(), 0);
    assert!(r.is_new(&stale));
    assert!(!r.is_new(&fresh));
}

#[test]
fn event_ring_rejects_bad_geometry() {
    assert!(EventRing::new(0x3004, 16).is_none());
    assert!(EventRing::new(0x3000, 0).is_none());
}

/// A producer and an event ring simulated together: the consumer sees each TRB exactly once across
/// several laps, the way a controller and the driver would.
#[test]
fn producer_and_consumer_agree_across_many_laps() {
    const SIZE: u16 = 8;
    let mut memory = [Trb::default(); SIZE as usize];
    let mut producer = ProducerRing::new(0x1000, SIZE).unwrap();
    let mut consumer = EventRing::new(0x1000, SIZE - 1).unwrap(); // the link slot is not an event
    let mut seen = 0u64;
    for n in 0..100u64 {
        let p = producer.push(Trb::new(n, 0, 0));
        memory[p.index as usize] = p.trb;
        if let Some((i, l)) = p.link {
            memory[i as usize] = l;
        }
        let slot = consumer.dequeue_index() as usize;
        assert!(consumer.is_new(&memory[slot]), "TRB {n} not visible");
        assert_eq!(memory[slot].param, n);
        consumer.advance();
        seen += 1;
        // nothing else is pending
        assert!(!consumer.is_new(&memory[consumer.dequeue_index() as usize]));
    }
    assert_eq!(seen, 100);
}

// ---- contexts ----

#[test]
fn context_offsets_for_32_and_64_byte_contexts() {
    assert_eq!(input_context_bytes(32), 32 * 33);
    assert_eq!(input_context_bytes(64), 64 * 33);
    assert_eq!(device_context_bytes(32), 32 * 32);
    assert_eq!(input_slot_offset(32), 32);
    assert_eq!(input_endpoint_offset(32, 1), 64);
    assert_eq!(input_endpoint_offset(64, 3), 64 * 4);
}

#[test]
fn endpoint_dci_from_the_address() {
    assert_eq!(endpoint_dci(0x00), 1); // default control endpoint
    assert_eq!(endpoint_dci(0x80), 1);
    assert_eq!(endpoint_dci(0x01), 2); // EP1 OUT
    assert_eq!(endpoint_dci(0x81), 3); // EP1 IN
    assert_eq!(endpoint_dci(0x82), 5); // EP2 IN
    assert_eq!(endpoint_dci(0x8F), 31);
}

#[test]
fn flags_and_input_control_context() {
    assert_eq!(FLAG_SLOT, 1);
    assert_eq!(flag_endpoint(1), 2);
    assert_eq!(flag_endpoint(3), 8);
    let c = input_control(0, FLAG_SLOT | flag_endpoint(1));
    assert_eq!(c[0], 0);
    assert_eq!(c[1], 0b11);
}

#[test]
fn slot_context_fields() {
    let c = slot_context(Speed::High, 4, 0, 1);
    assert_eq!(c[0] >> 20 & 0xF, 3, "speed");
    assert_eq!(c[0] >> 27, 1, "context entries");
    assert_eq!(c[0] & 0xF_FFFF, 0, "route string");
    assert_eq!(c[1] >> 16 & 0xFF, 4, "root hub port number");
    let low = slot_context(Speed::Low, 1, 0, 3);
    assert_eq!(low[0] >> 20 & 0xF, 2);
    assert_eq!(low[0] >> 27, 3);
}

#[test]
fn control_endpoint_context_fields() {
    let ring = ProducerRing::new(0x1_0000_2000, 16).unwrap().pointer();
    let c = control_endpoint_context(64, ring);
    assert_eq!(c[1] >> 3 & 7, 4, "endpoint type: control");
    assert_eq!(c[1] >> 1 & 3, 3, "error count");
    assert_eq!(c[1] >> 16, 64, "max packet size");
    assert_eq!(c[2], 0x0000_2001, "low half of the ring pointer + cycle");
    assert_eq!(c[3], 1, "high half");
    assert_eq!(c[4] & 0xFFFF, 8, "average TRB length");
}

#[test]
fn interrupt_in_endpoint_context_fields() {
    let ring = ProducerRing::new(0x4000, 16).unwrap().pointer();
    let c = interrupt_in_endpoint_context(8, 6, ring);
    assert_eq!(c[0] >> 16 & 0xFF, 6, "interval");
    assert_eq!(c[1] >> 3 & 7, 7, "endpoint type: interrupt IN");
    assert_eq!(c[1] >> 16, 8);
    assert_eq!(c[2], 0x4001);
    assert_eq!(c[4] & 0xFFFF, 8);
    assert_eq!(c[4] >> 16, 8, "max ESIT payload");
}

#[test]
fn interrupt_interval_per_speed() {
    // high and super speed: bInterval is already an exponent (1..=16), the field is one less
    assert_eq!(interrupt_interval(Speed::High, 1), 0);
    assert_eq!(interrupt_interval(Speed::High, 4), 3);
    assert_eq!(interrupt_interval(Speed::Super, 16), 15);
    assert_eq!(interrupt_interval(Speed::High, 0), 0); // out of range is clamped
    assert_eq!(interrupt_interval(Speed::High, 200), 15);
    // low and full speed: milliseconds -> floor(log2(8 * ms)), kept within 3..=10
    assert_eq!(interrupt_interval(Speed::Low, 10), 6); // 80 -> 6
    assert_eq!(interrupt_interval(Speed::Full, 1), 3); // 8 -> 3
    assert_eq!(interrupt_interval(Speed::Full, 8), 6); // 64 -> 6
    assert_eq!(interrupt_interval(Speed::Full, 0), 3);
    assert_eq!(interrupt_interval(Speed::Full, 255), 10); // 2040 -> 10
}

#[test]
fn write_context_places_the_dwords_and_checks_bounds() {
    let mut buf = [0u32; 24]; // three 32-byte contexts
    let ctx = [1, 2, 3, 4, 5, 6, 7, 8];
    assert!(write_context(&mut buf, 32, &ctx));
    assert_eq!(&buf[8..16], &ctx);
    assert_eq!(&buf[0..8], &[0; 8]);
    assert!(!write_context(&mut buf, 64 + 4, &ctx), "runs past the end");
    assert!(!write_context(&mut buf, 6, &ctx), "unaligned");
    assert!(
        !write_context(&mut buf, usize::MAX - 3, &ctx),
        "huge offset must not overflow"
    );
}
