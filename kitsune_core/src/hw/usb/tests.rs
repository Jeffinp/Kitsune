use super::*;

/// Device descriptor of a QEMU `usb-kbd`-like device.
fn device_bytes() -> [u8; 18] {
    [
        18, 1, 0x00, 0x02, 0, 0, 0, 64, 0x27, 0x06, 0x01, 0x00, 0x00, 0x00, 1, 2, 3, 1,
    ]
}

fn interface(number: u8, alt: u8, endpoints: u8, class: u8, subclass: u8, protocol: u8) -> Vec<u8> {
    vec![9, 4, number, alt, endpoints, class, subclass, protocol, 0]
}

fn hid_class() -> Vec<u8> {
    vec![9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0]
}

fn endpoint(address: u8, attributes: u8, max_packet: u16, interval: u8) -> Vec<u8> {
    vec![
        7,
        5,
        address,
        attributes,
        max_packet as u8,
        (max_packet >> 8) as u8,
        interval,
    ]
}

/// Header + the given descriptors, with `wTotalLength` filled in.
fn config(value: u8, parts: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = parts.concat();
    let total = 9 + body.len();
    let mut v = vec![9, 2, total as u8, (total >> 8) as u8, 1, value, 0, 0xA0, 50];
    v.extend(body);
    v
}

fn blank() -> BootHid {
    BootHid {
        configuration: 0,
        interface: 0,
        protocol: BootProtocol::Keyboard,
        endpoint: 0,
        max_packet: 0,
        interval: 0,
    }
}

#[test]
fn parse_device_fields() {
    let d = parse_device(&device_bytes()).unwrap();
    assert_eq!(d.usb_version, 0x0200);
    assert_eq!((d.class, d.subclass, d.protocol), (0, 0, 0));
    assert_eq!(d.max_packet_size0, 64);
    assert_eq!((d.vendor, d.product), (0x0627, 0x0001));
    assert_eq!(d.configurations, 1);
}

#[test]
fn parse_device_rejects_short_or_wrong_type() {
    assert_eq!(parse_device(&device_bytes()[..17]), None);
    let mut wrong = device_bytes();
    wrong[1] = 2;
    assert_eq!(parse_device(&wrong), None);
    let mut short_len = device_bytes();
    short_len[0] = 8;
    assert_eq!(parse_device(&short_len), None);
    assert_eq!(parse_device(&[]), None);
}

#[test]
fn max_packet_size0_from_the_first_eight_bytes() {
    assert_eq!(max_packet_size0_of(&device_bytes()[..8]), Some(64));
    assert_eq!(max_packet_size0_of(&device_bytes()[..7]), None);
    assert_eq!(max_packet_size0_of(&[8, 2, 0, 0, 0, 0, 0, 64]), None);
}

#[test]
fn config_header_and_request_length() {
    let c = config(1, &[interface(0, 0, 1, 3, 1, 1)]);
    let h = parse_config_header(&c).unwrap();
    assert_eq!(
        (h.total_length as usize, h.interfaces, h.value),
        (c.len(), 1, 1)
    );
    assert_eq!(parse_config_header(&c[..8]), None);
    assert_eq!(config_request_len(0), 9);
    assert_eq!(config_request_len(34), 34);
    assert_eq!(config_request_len(60_000), MAX_CONFIG_LEN);
}

#[test]
fn finds_a_boot_keyboard() {
    let c = config(
        1,
        &[
            interface(0, 0, 1, 3, 1, 1),
            hid_class(),
            endpoint(0x81, 3, 8, 10),
        ],
    );
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 1);
    assert_eq!(
        out[0],
        BootHid {
            configuration: 1,
            interface: 0,
            protocol: BootProtocol::Keyboard,
            endpoint: 0x81,
            max_packet: 8,
            interval: 10
        }
    );
}

#[test]
fn finds_a_boot_mouse_and_a_keyboard_in_one_device() {
    let c = config(
        2,
        &[
            interface(0, 0, 1, 3, 1, 1),
            hid_class(),
            endpoint(0x81, 3, 8, 8),
            interface(1, 0, 1, 3, 1, 2),
            hid_class(),
            endpoint(0x82, 3, 4, 4),
        ],
    );
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 2);
    assert_eq!(out[0].protocol, BootProtocol::Keyboard);
    assert_eq!(
        (out[1].protocol, out[1].interface, out[1].endpoint),
        (BootProtocol::Mouse, 1, 0x82)
    );
    assert_eq!(out[1].configuration, 2);
}

#[test]
fn ignores_non_boot_hid_other_classes_and_alternate_settings() {
    let c = config(
        1,
        &[
            interface(0, 0, 1, 3, 0, 0), // HID without boot subclass (media keys)
            hid_class(),
            endpoint(0x81, 3, 8, 10),
            interface(1, 0, 1, 8, 6, 0x50), // mass storage
            endpoint(0x82, 2, 64, 0),
            interface(2, 1, 1, 3, 1, 1), // boot keyboard, but alternate setting 1
            endpoint(0x83, 3, 8, 10),
        ],
    );
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 0);
}

#[test]
fn a_boot_interface_without_an_interrupt_in_endpoint_is_dropped() {
    let c = config(
        1,
        &[
            interface(0, 0, 1, 3, 1, 1),
            endpoint(0x01, 3, 8, 10), // interrupt OUT
            interface(1, 0, 1, 3, 1, 2),
            endpoint(0x82, 2, 8, 0), // bulk IN
            interface(2, 0, 1, 3, 1, 2),
            endpoint(0x83, 3, 4, 4), // the good one
        ],
    );
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 1);
    assert_eq!(out[0].interface, 2);
}

#[test]
fn the_first_interrupt_in_endpoint_wins() {
    let c = config(
        1,
        &[
            interface(0, 0, 2, 3, 1, 1),
            endpoint(0x81, 3, 8, 10),
            endpoint(0x82, 3, 64, 1),
        ],
    );
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 1);
    assert_eq!(out[0].endpoint, 0x81);
}

#[test]
fn max_packet_keeps_only_the_size_bits() {
    let c = config(
        1,
        &[interface(0, 0, 1, 3, 1, 1), endpoint(0x81, 3, 0x1808, 10)],
    );
    let mut out = [blank(); 4];
    boot_hid_interfaces(&c, &mut out);
    assert_eq!(out[0].max_packet, 8);
}

#[test]
fn stops_when_the_output_is_full() {
    let mut parts = Vec::new();
    for n in 0..6 {
        parts.push(interface(n, 0, 1, 3, 1, 1));
        parts.push(endpoint(0x81 + n, 3, 8, 10));
    }
    let c = config(1, &parts);
    let mut out = [blank(); 2];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 2);
    assert_eq!(boot_hid_interfaces(&c, &mut []), 0);
}

#[test]
fn an_endpoint_before_any_interface_is_ignored() {
    let c = config(1, &[endpoint(0x81, 3, 8, 10), interface(0, 0, 0, 3, 1, 1)]);
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 0);
}

#[test]
fn a_descriptor_with_length_zero_or_one_does_not_loop_forever() {
    for bad in [0u8, 1] {
        let c = config(1, &[interface(0, 0, 1, 3, 1, 1), vec![bad, 5, 0x81, 3]]);
        let mut out = [blank(); 4];
        assert_eq!(boot_hid_interfaces(&c, &mut out), 0);
    }
}

#[test]
fn a_descriptor_that_runs_past_the_end_stops_the_walk() {
    let mut c = config(1, &[interface(0, 0, 1, 3, 1, 1), endpoint(0x81, 3, 8, 10)]);
    let n = c.len();
    c[n - 7] = 200; // the endpoint claims 200 bytes
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 0);
}

#[test]
fn a_total_length_larger_than_the_data_is_clamped_to_the_data() {
    let mut c = config(1, &[interface(0, 0, 1, 3, 1, 1), endpoint(0x81, 3, 8, 10)]);
    c[2] = 0xFF;
    c[3] = 0xFF;
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 1);
}

#[test]
fn a_total_length_smaller_than_the_data_hides_the_rest() {
    let mut c = config(1, &[interface(0, 0, 1, 3, 1, 1), endpoint(0x81, 3, 8, 10)]);
    c[2] = 9 + 9; // stops after the interface descriptor
    c[3] = 0;
    let mut out = [blank(); 4];
    assert_eq!(boot_hid_interfaces(&c, &mut out), 0);
}

#[test]
fn garbage_never_panics() {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut out = [blank(); 4];
    for len in 0..300usize {
        let mut bytes = vec![0u8; len];
        for b in &mut bytes {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        // make the header plausible so the walk goes deeper than the first check
        if len >= 9 {
            bytes[0] = 9;
            bytes[1] = 2;
            bytes[2] = len as u8;
            bytes[3] = (len >> 8) as u8;
        }
        let _ = boot_hid_interfaces(&bytes, &mut out);
        let _ = parse_device(&bytes);
        let _ = parse_config_header(&bytes);
        let _ = max_packet_size0_of(&bytes);
    }
}
