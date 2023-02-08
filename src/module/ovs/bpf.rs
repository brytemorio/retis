//! Rust<>BPF types definitions for the ovs module.
//! Please keep this file in sync with its BPF counterpart in bpf/.

use anyhow::{bail, Result};
use plain::Plain;

use crate::{
    core::events::{
        bpf::{parse_raw_section, BpfRawSection},
        EventField,
    },
    event_field,
};

/// Types of events that can be generated by the ovs module.
#[derive(Debug, Eq, Hash, PartialEq)]
pub(crate) enum OvsEventType {
    /// Upcall tracepoint.
    Upcall = 0,
}

impl OvsEventType {
    pub(super) fn from_u8(val: u8) -> Result<OvsEventType> {
        use OvsEventType::*;
        Ok(match val {
            0 => Upcall,
            x => bail!("Can't construct a OvsEventType from {}", x),
        })
    }
}

/// OVS Upcall data.
#[derive(Default)]
#[repr(C, packed)]
struct UpcallEvent {
    /// Upcall command. Holds OVS_PACKET_CMD:
    ///   OVS_PACKET_CMD_UNSPEC   = 0
    ///   OVS_PACKET_CMD_MISS     = 1
    ///   OVS_PACKET_CMD_ACTION   = 2
    ///   OVS_PACKET_CMD_EXECUTE  = 3
    cmd: u8,
    /// Upcall port.
    port: u32,
}
unsafe impl Plain for UpcallEvent {}

pub(super) fn unmarshall_upcall(raw: &BpfRawSection, fields: &mut Vec<EventField>) -> Result<()> {
    let event = parse_raw_section::<UpcallEvent>(raw)?;

    fields.push(event_field!("upcall_port", event.port));
    fields.push(event_field!("cmd", event.cmd));
    fields.push(event_field!("event_type", "upcall".to_string()));
    Ok(())
}
