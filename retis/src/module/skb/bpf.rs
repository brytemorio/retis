//! Rust<>BPF types definitions for the skb module. Fields are not translated in
//! the BPF part and can be represented in various orders, depending from where
//! they come from. Some handling might be needed in the unmarshalers.
//!
//! Please keep this file in sync with its BPF counterpart in bpf/skb_hook.bpf.c

use anyhow::bail;
use std::str;

use anyhow::{anyhow, Result};
use pnet_packet::{
    arp::ArpPacket, ethernet::*, icmp::IcmpPacket, icmpv6::Icmpv6Packet, ip::*, ipv4::*, ipv6::*,
    tcp::TcpPacket, udp::UdpPacket, Packet,
};

use crate::{
    core::events::{
        parse_raw_section, BpfRawSection, EventSectionFactory, FactoryId, RawEventSectionFactory,
    },
    event_byte_array, event_section_factory,
    events::{net::RawPacket, *},
    helpers, raw_event_section,
};

/// Valid raw event sections of the skb collector. We do not use an enum here as
/// they are difficult to work with for bitfields and C repr conversion.
pub(super) const SECTION_PACKET: u64 = 1;
pub(super) const SECTION_DEV: u64 = 2;
pub(super) const SECTION_NS: u64 = 3;
pub(super) const SECTION_META: u64 = 4;
pub(super) const SECTION_DATA_REF: u64 = 5;
pub(super) const SECTION_GSO: u64 = 6;

/// Global configuration passed down the BPF part.
#[repr(C, packed)]
pub(super) struct RawConfig {
    /// Bitfield of what to collect from skbs. Currently `1 << SECTION_x` is
    /// used to trigger retrieval of a given section.
    pub sections: u64,
}

pub(super) fn unmarshal_eth(eth: &EthernetPacket) -> Result<SkbEthEvent> {
    Ok(SkbEthEvent {
        etype: eth.get_ethertype().0,
        src: helpers::net::parse_eth_addr(&eth.get_source().octets())?,
        dst: helpers::net::parse_eth_addr(&eth.get_destination().octets())?,
    })
}

pub(super) fn unmarshal_arp(arp: &ArpPacket) -> Result<Option<SkbArpEvent>> {
    let operation = match arp.get_operation().0 {
        1 => ArpOperation::Request,
        2 => ArpOperation::Reply,
        // We only support ARP for IPv4 over Ethernet; request & reply */
        _ => return Ok(None),
    };

    Ok(Some(SkbArpEvent {
        operation,
        sha: helpers::net::parse_eth_addr(&arp.get_sender_hw_addr().octets())?,
        spa: helpers::net::parse_ipv4_addr(u32::from(arp.get_sender_proto_addr()))?,
        tha: helpers::net::parse_eth_addr(&arp.get_target_hw_addr().octets())?,
        tpa: helpers::net::parse_ipv4_addr(u32::from(arp.get_target_proto_addr()))?,
    }))
}

pub(super) fn unmarshal_ipv4(ip: &Ipv4Packet) -> Result<SkbIpEvent> {
    Ok(SkbIpEvent {
        saddr: helpers::net::parse_ipv4_addr(u32::from(ip.get_source()))?,
        daddr: helpers::net::parse_ipv4_addr(u32::from(ip.get_destination()))?,
        version: SkbIpVersion::V4(SkbIpv4Event {
            tos: ip.get_dscp(),
            flags: ip.get_flags(),
            id: ip.get_identification(),
            offset: ip.get_fragment_offset(),
        }),
        protocol: ip.get_next_level_protocol().0,
        len: ip.get_total_length(),
        ttl: ip.get_ttl(),
        ecn: ip.get_ecn(),
    })
}

pub(super) fn unmarshal_ipv6(ip: &Ipv6Packet) -> Result<SkbIpEvent> {
    Ok(SkbIpEvent {
        saddr: ip.get_source().to_string(),
        daddr: ip.get_destination().to_string(),
        version: SkbIpVersion::V6(SkbIpv6Event {
            flow_label: ip.get_flow_label(),
        }),
        protocol: ip.get_next_header().0,
        len: ip.get_payload_length(),
        ttl: ip.get_hop_limit(),
        ecn: ip.get_traffic_class() & 0x3,
    })
}

pub(super) fn unmarshal_tcp(tcp: &TcpPacket) -> Result<SkbTcpEvent> {
    Ok(SkbTcpEvent {
        sport: tcp.get_source(),
        dport: tcp.get_destination(),
        seq: tcp.get_sequence(),
        ack_seq: tcp.get_acknowledgement(),
        window: tcp.get_window(),
        doff: tcp.get_data_offset(),
        flags: tcp.get_flags(),
    })
}

pub(super) fn unmarshal_udp(udp: &UdpPacket) -> Result<SkbUdpEvent> {
    Ok(SkbUdpEvent {
        sport: udp.get_source(),
        dport: udp.get_destination(),
        len: udp.get_length(),
    })
}

pub(super) fn unmarshal_icmp(icmp: &IcmpPacket) -> Result<SkbIcmpEvent> {
    Ok(SkbIcmpEvent {
        r#type: icmp.get_icmp_type().0,
        code: icmp.get_icmp_code().0,
    })
}

pub(super) fn unmarshal_icmpv6(icmp: &Icmpv6Packet) -> Result<SkbIcmpV6Event> {
    Ok(SkbIcmpV6Event {
        r#type: icmp.get_icmpv6_type().0,
        code: icmp.get_icmpv6_code().0,
    })
}

/// Net device information retrieved from skbs.
#[raw_event_section]
pub(crate) struct RawDevEvent {
    /// Net device name.
    dev_name: [u8; 16],
    /// Net device index.
    ifindex: u32,
    /// Original ifindex the packet arrived on.
    iif: u32,
}

/// Unmarshal net device info. Can return Ok(None) in case the info does not
/// look like it's genuine (see below).
pub(super) fn unmarshal_dev(raw_section: &BpfRawSection) -> Result<Option<SkbDevEvent>> {
    let raw = parse_raw_section::<RawDevEvent>(raw_section)?;

    // Retrieving information from `skb->dev` is tricky as this is inside an
    // union and there is no way we can know of the data is valid. Try our best
    // below to report an empty section if the data does not look like what it
    // should.
    let dev_name = match str::from_utf8(&raw.dev_name) {
        Ok(s) => s.trim_end_matches(char::from(0)),
        Err(_) => return Ok(None),
    };

    // Not much more we can do, construct the event section.
    let mut event = SkbDevEvent {
        name: dev_name.to_string(),
        ifindex: raw.ifindex,
        ..Default::default()
    };
    if raw.iif > 0 {
        event.rx_ifindex = Some(raw.iif);
    }

    Ok(Some(event))
}

/// Net namespace information retrieved from skbs.
#[raw_event_section]
pub(crate) struct RawNsEvent {
    /// Net namespace id.
    netns: u32,
}

pub(super) fn unmarshal_ns(raw_section: &BpfRawSection) -> Result<SkbNsEvent> {
    let raw = parse_raw_section::<RawNsEvent>(raw_section)?;

    Ok(SkbNsEvent { netns: raw.netns })
}

/// Skb metadata & related.
#[raw_event_section]
struct RawMetaEvent {
    len: u32,
    data_len: u32,
    hash: u32,
    ip_summed: u8,
    csum: u32,
    csum_level: u8,
    priority: u32,
}

pub(super) fn unmarshal_meta(raw_section: &BpfRawSection) -> Result<SkbMetaEvent> {
    let raw = parse_raw_section::<RawMetaEvent>(raw_section)?;

    Ok(SkbMetaEvent {
        len: raw.len,
        data_len: raw.data_len,
        hash: raw.hash,
        ip_summed: raw.ip_summed,
        csum: raw.csum,
        csum_level: raw.csum_level,
        priority: raw.priority,
    })
}

/// Data & refcount information retrieved from skbs.
#[raw_event_section]
struct RawDataRefEvent {
    nohdr: u8,
    /// Is the skb a clone?
    cloned: u8,
    /// Is the skb a fast clone?
    fclone: u8,
    /// Users count.
    users: u8,
    /// Data refcount.
    dataref: u8,
}

pub(super) fn unmarshal_data_ref(raw_section: &BpfRawSection) -> Result<SkbDataRefEvent> {
    let raw = parse_raw_section::<RawDataRefEvent>(raw_section)?;

    Ok(SkbDataRefEvent {
        nohdr: raw.nohdr == 1,
        cloned: raw.cloned == 1,
        fclone: raw.fclone,
        users: raw.users,
        dataref: raw.dataref,
    })
}

/// GSO information.
#[raw_event_section]
struct RawGsoEvent {
    flags: u8,
    nr_frags: u8,
    gso_size: u32,
    gso_segs: u32,
    gso_type: u32,
}

pub(super) fn unmarshal_gso(raw_section: &BpfRawSection) -> Result<SkbGsoEvent> {
    let raw = parse_raw_section::<RawGsoEvent>(raw_section)?;

    Ok(SkbGsoEvent {
        flags: raw.flags,
        frags: raw.nr_frags,
        size: raw.gso_size,
        segs: raw.gso_segs,
        r#type: raw.gso_type,
    })
}

/// Raw packet and related metadata extracted from skbs.
#[raw_event_section]
pub(crate) struct RawPacketEvent {
    /// Length of the packet.
    len: u32,
    /// Lenght of the capture. <= len.
    capture_len: u32,
    /// Raw packet data.
    packet: RawBpfPacket,
    /// Is eth header fake (generated by the BPF hook)?
    fake_eth: u8,
}

event_byte_array!(RawBpfPacket, 255);

pub(super) fn unmarshal_packet(
    event: &mut SkbEvent,
    raw_section: &BpfRawSection,
    report_eth: bool,
) -> Result<()> {
    let raw = parse_raw_section::<RawPacketEvent>(raw_section)?;

    // First add the raw packet part in the event.
    event.packet = Some(SkbPacketEvent {
        len: raw.len,
        capture_len: raw.capture_len,
        packet: RawPacket(raw.packet.0[..(raw.capture_len as usize)].to_vec()),
    });

    // Then start parsing the raw packet to generate other sections.
    let eth =
        EthernetPacket::new(&raw.packet.0[..(raw.capture_len as usize)]).ok_or_else(|| {
            anyhow!("Could not parse Ethernet packet (buffer size less than minimal)")
        })?;

    if report_eth && raw.fake_eth == 0 {
        event.eth = Some(unmarshal_eth(&eth)?);
    }

    match eth.get_ethertype() {
        EtherTypes::Arp => {
            if let Some(eth) = ArpPacket::new(eth.payload()) {
                event.arp = unmarshal_arp(&eth)?;
            };
        }
        EtherTypes::Ipv4 => {
            if let Some(ip) = Ipv4Packet::new(eth.payload()) {
                event.ip = Some(unmarshal_ipv4(&ip)?);
                unmarshal_l4(event, ip.get_next_level_protocol(), ip.payload())?;
            };
        }
        EtherTypes::Ipv6 => {
            if let Some(ip) = Ipv6Packet::new(eth.payload()) {
                event.ip = Some(unmarshal_ipv6(&ip)?);
                unmarshal_l4(event, ip.get_next_header(), ip.payload())?;
            };
        }
        // If we did not generate any data in the skb section, this means we do
        // not support yet the protocol used. At least provide the ethertype.
        _ => {
            if event.eth.is_none() {
                event.eth = Some(unmarshal_eth(&eth)?);
            }
        }
    }

    Ok(())
}

fn unmarshal_l4(
    event: &mut SkbEvent,
    protocol: IpNextHeaderProtocol,
    payload: &[u8],
) -> Result<()> {
    match protocol {
        IpNextHeaderProtocols::Tcp => {
            if let Some(tcp) = TcpPacket::new(payload) {
                event.tcp = Some(unmarshal_tcp(&tcp)?);
            }
        }
        IpNextHeaderProtocols::Udp => {
            if let Some(udp) = UdpPacket::new(payload) {
                event.udp = Some(unmarshal_udp(&udp)?);
            }
        }
        IpNextHeaderProtocols::Icmp => {
            if let Some(icmp) = IcmpPacket::new(payload) {
                event.icmp = Some(unmarshal_icmp(&icmp)?);
            }
        }
        IpNextHeaderProtocols::Icmpv6 => {
            if let Some(icmpv6) = Icmpv6Packet::new(payload) {
                event.icmpv6 = Some(unmarshal_icmpv6(&icmpv6)?);
            }
        }
        _ => (),
    }

    Ok(())
}

#[event_section_factory(FactoryId::Skb)]
#[derive(Default)]
pub(crate) struct SkbEventFactory {
    // Should we report the Ethernet header.
    pub(super) report_eth: bool,
}

impl RawEventSectionFactory for SkbEventFactory {
    fn create(&mut self, raw_sections: Vec<BpfRawSection>) -> Result<Box<dyn EventSection>> {
        let mut event = SkbEvent::default();

        for section in raw_sections.iter() {
            match section.header.data_type as u64 {
                SECTION_DEV => event.dev = unmarshal_dev(section)?,
                SECTION_NS => event.ns = Some(unmarshal_ns(section)?),
                SECTION_META => event.meta = Some(unmarshal_meta(section)?),
                SECTION_DATA_REF => event.data_ref = Some(unmarshal_data_ref(section)?),
                SECTION_GSO => event.gso = Some(unmarshal_gso(section)?),
                SECTION_PACKET => unmarshal_packet(&mut event, section, self.report_eth)?,
                x => bail!("Unknown data type ({x})"),
            }
        }

        Ok(Box::new(event))
    }
}

#[cfg(feature = "benchmark")]
pub(crate) mod benchmark {
    use anyhow::Result;

    use super::*;
    use crate::{benchmark::helpers::*, events::SectionId};

    impl RawSectionBuilder for RawDevEvent {
        fn build_raw(out: &mut Vec<u8>) -> Result<()> {
            let data = RawDevEvent {
                dev_name: [
                    b'e', b't', b'h', b'0', b'\0', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                ],
                ..Default::default()
            };
            build_raw_section(
                out,
                SectionId::Skb.to_u8(),
                SECTION_DEV as u8,
                &mut as_u8_vec(&data),
            );
            Ok(())
        }
    }

    impl RawSectionBuilder for RawNsEvent {
        fn build_raw(out: &mut Vec<u8>) -> Result<()> {
            let data = RawNsEvent::default();
            build_raw_section(
                out,
                SectionId::Skb.to_u8(),
                SECTION_NS as u8,
                &mut as_u8_vec(&data),
            );
            Ok(())
        }
    }

    impl RawSectionBuilder for RawPacketEvent {
        fn build_raw(out: &mut Vec<u8>) -> Result<()> {
            let data = RawPacketEvent {
                len: 66,
                capture_len: 66,
                packet: RawBpfPacket([
                    46, 137, 59, 254, 34, 122, 42, 186, 90, 193, 129, 79, 8, 0, 69, 0, 0, 52, 32,
                    32, 64, 0, 55, 6, 237, 160, 1, 1, 1, 1, 10, 0, 42, 2, 1, 187, 157, 12, 31, 149,
                    22, 86, 145, 251, 180, 241, 128, 17, 0, 8, 17, 72, 0, 0, 1, 1, 8, 10, 28, 109,
                    231, 120, 127, 134, 144, 92, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                ]),
                ..Default::default()
            };
            build_raw_section(
                out,
                SectionId::Skb.to_u8(),
                SECTION_PACKET as u8,
                &mut as_u8_vec(&data),
            );
            Ok(())
        }
    }
}
