use std::fmt;

use anyhow::{bail, Result};

use super::bpf::*;
use crate::{
    core::events::{bpf::BpfRawSection, *},
    event_section, event_section_factory,
};

/// Skb event section.
#[event_section]
pub(crate) struct SkbEvent {
    /// Ethernet fields, if any.
    pub(crate) eth: Option<SkbEthEvent>,
    /// IPv4 or IPv6 fields, if any.
    pub(crate) ip: Option<SkbIpEvent>,
    /// TCP fields, if any.
    pub(crate) tcp: Option<SkbTcpEvent>,
    /// UDP fields, if any.
    pub(crate) udp: Option<SkbUdpEvent>,
    /// ICMP fields, if any.
    pub(crate) icmp: Option<SkbIcmpEvent>,
    /// Net device data, if any.
    pub(crate) dev: Option<SkbDevEvent>,
    /// Net namespace data, if any.
    pub(crate) ns: Option<SkbNsEvent>,
    /// Skb metadata, if any.
    pub(crate) meta: Option<SkbMetaEvent>,
    /// Skb data-related and refcnt information, if any.
    pub(crate) data_ref: Option<SkbDataRefEvent>,
}

impl EventFmt for SkbEvent {
    fn event_fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut len = 0;

        if let Some(ns) = &self.ns {
            write!(f, " ns {}", ns.netns)?;
        }

        if let Some(dev) = &self.dev {
            if dev.ifindex > 0 {
                write!(f, " if {}", dev.ifindex)?;
                if !dev.name.is_empty() {
                    write!(f, " ({})", dev.name)?;
                }
            }
            if let Some(rx_ifindex) = dev.rx_ifindex {
                write!(f, " rxif {}", rx_ifindex)?;
            }
        }

        if let Some(eth) = &self.eth {
            // FIXME.
            let ethertype = match eth.etype {
                0x0800 => " IPv4",
                0x0806 => " ARP",
                0x86dd => " IPv6",
                0x88e5 => " MACsec",
                0x88f7 => " PTP",
                _ => "",
            };
            write!(
                f,
                " {} > {} ethertype{} ({:#06x})",
                eth.src, eth.dst, ethertype, eth.etype
            )?;
        }

        if let Some(ip) = &self.ip {
            len = ip.len;

            if let Some(tcp) = &self.tcp {
                write!(
                    f,
                    " {}.{} > {}.{}",
                    ip.saddr, tcp.sport, ip.daddr, tcp.dport
                )?;
            } else if let Some(udp) = &self.udp {
                write!(
                    f,
                    " {}.{} > {}.{}",
                    ip.saddr, udp.sport, ip.daddr, udp.dport
                )?;
            } else {
                write!(f, " {} > {}", ip.saddr, ip.daddr)?;
            }

            // FIXME.
            let protocol = match ip.protocol {
                1 => " ICMP",
                2 => " IGMP",
                6 => " TCP",
                17 => " UDP",
                41 => " IPv6",
                47 => " GRE",
                50 => " ESP",
                58 => " ICMPv6",
                94 => " IPIP",
                _ => "",
            };

            write!(f, " len {} proto{} ({})", ip.len, protocol, ip.protocol)?;
        }

        if let Some(tcp) = &self.tcp {
            let mut flags = Vec::new();
            if tcp.flags & 1 << 0 != 0 {
                flags.push('F');
            }
            if tcp.flags & 1 << 1 != 0 {
                flags.push('S');
            }
            if tcp.flags & 1 << 2 != 0 {
                flags.push('R');
            }
            if tcp.flags & 1 << 3 != 0 {
                flags.push('P');
            }
            if tcp.flags & 1 << 4 != 0 {
                flags.push('.');
            }
            if tcp.flags & 1 << 5 != 0 {
                flags.push('U');
            }
            write!(f, " flags [{}]", flags.into_iter().collect::<String>())?;

            let len = len - tcp.doff as u16;
            if len > 0 {
                write!(f, " seq {}:{}", tcp.seq, tcp.seq + len as u32)?;
            } else {
                write!(f, " seq {}", tcp.seq)?;
            }

            if tcp.flags & 1 << 4 != 0 {
                write!(f, " ack {}", tcp.ack_seq)?;
            }

            write!(f, " win {}", tcp.window)?;
        }

        if let Some(udp) = &self.udp {
            write!(f, " len {}", udp.len)?;
        }

        if let Some(icmp) = &self.icmp {
            // TODO: text version
            write!(f, " type {} code {}", icmp.r#type, icmp.code)?;
        }

        if self.meta.is_some() || self.data_ref.is_some() {
            write!(f, " skb [")?;

            if let Some(meta) = &self.meta {
                if meta.csum != 0 {
                    write!(f, "csum {:#x} ", meta.csum)?;
                }
                if meta.hash != 0 {
                    write!(f, "hash {:#x} ", meta.hash)?;
                }
                write!(f, "len {} ", meta.len,)?;
                if meta.data_len != 0 {
                    write!(f, "data_len {} ", meta.data_len)?;
                }
                write!(f, "priority {} ", meta.priority)?;
            }

            if let Some(dataref) = &self.data_ref {
                if dataref.nohdr {
                    write!(f, "nohdr ")?;
                }
                if dataref.cloned {
                    write!(f, "cloned ")?;
                }
                if dataref.fclone > 0 {
                    write!(f, "fclone {} ", dataref.fclone)?;
                }
                write!(f, "users {} dataref {}", dataref.users, dataref.dataref)?;
            }

            write!(f, "]")?;
        }

        Ok(())
    }
}

/// Ethernet fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbEthEvent {
    /// Ethertype.
    pub(crate) etype: u16,
    /// Source MAC address.
    pub(crate) src: String,
    /// Destination MAC address.
    pub(crate) dst: String,
}

/// IPv4/IPv6 fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbIpEvent {
    /// Source IP address.
    pub(crate) saddr: String,
    /// Destination IP address.
    pub(crate) daddr: String,
    /// IP version: 4 or 6.
    pub(crate) version: u8,
    /// L4 protocol, from IPv4 "protocol" field or IPv6 "next header" one.
    pub(crate) protocol: u8,
    /// "total len" from the IPv4 header or "payload length" from the IPv6 one.
    pub(crate) len: u16,
}

/// TCP fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbTcpEvent {
    /// Source port.
    pub(crate) sport: u16,
    /// Destination port.
    pub(crate) dport: u16,
    pub(crate) seq: u32,
    pub(crate) ack_seq: u32,
    pub(crate) window: u16,
    /// Data offset.
    pub(crate) doff: u8,
    /// Bitfield of TCP flags as defined in `struct tcphdr` in the kernel.
    pub(crate) flags: u8,
}

/// UDP fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbUdpEvent {
    /// Source port.
    pub(crate) sport: u16,
    /// Destination port.
    pub(crate) dport: u16,
    /// Length from the UDP header.
    pub(crate) len: u16,
}

/// ICMP fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbIcmpEvent {
    pub(crate) r#type: u8,
    pub(crate) code: u8,
}

/// Network device fields.
#[serde_with::skip_serializing_none]
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbDevEvent {
    /// Net device name associated with the packet, from `skb->dev->name`.
    pub(crate) name: String,
    /// Net device ifindex associated with the packet, from `skb->dev->ifindex`.
    pub(crate) ifindex: u32,
    /// Index if the net device the packet arrived on, from `skb->skb_iif`.
    pub(crate) rx_ifindex: Option<u32>,
}

/// Network namespace fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbNsEvent {
    /// Id of the network namespace associated with the packet, from the device
    /// or the associated socket (in that order).
    pub(crate) netns: u32,
}

/// Skb metadata & releated fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbMetaEvent {
    /// Total number of bytes in the packet.
    pub(crate) len: u32,
    /// Total number of bytes in the page buffer area.
    pub(crate) data_len: u32,
    /// Packet hash (!= hash of the packet data).
    pub(crate) hash: u32,
    /// Packet checksum.
    pub(crate) csum: u32,
    /// QoS priority.
    pub(crate) priority: u32,
}

/// Skb data & refcnt fields.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct SkbDataRefEvent {
    /// Payload reference only.
    pub(crate) nohdr: bool,
    /// Is the skb a clone?
    pub(crate) cloned: bool,
    /// Skb fast clone information.
    pub(crate) fclone: u8,
    /// Users count.
    pub(crate) users: u8,
    /// Data refcount.
    pub(crate) dataref: u8,
}

#[derive(Default)]
#[event_section_factory(SkbEvent)]
pub(crate) struct SkbEventFactory {}

impl RawEventSectionFactory for SkbEventFactory {
    fn from_raw(&mut self, raw_sections: Vec<BpfRawSection>) -> Result<Box<dyn EventSection>> {
        let mut event = SkbEvent::default();

        for section in raw_sections.iter() {
            match section.header.data_type as u64 {
                SECTION_ETH => event.eth = Some(unmarshal_eth(section)?),
                SECTION_IPV4 => event.ip = Some(unmarshal_ipv4(section)?),
                SECTION_IPV6 => event.ip = Some(unmarshal_ipv6(section)?),
                SECTION_TCP => event.tcp = Some(unmarshal_tcp(section)?),
                SECTION_UDP => event.udp = Some(unmarshal_udp(section)?),
                SECTION_ICMP => event.icmp = Some(unmarshal_icmp(section)?),
                SECTION_DEV => event.dev = Some(unmarshal_dev(section)?),
                SECTION_NS => event.ns = Some(unmarshal_ns(section)?),
                SECTION_META => event.meta = Some(unmarshal_meta(section)?),
                SECTION_DATA_REF => event.data_ref = Some(unmarshal_data_ref(section)?),
                _ => bail!("Unknown data type"),
            }
        }

        Ok(Box::new(event))
    }
}
