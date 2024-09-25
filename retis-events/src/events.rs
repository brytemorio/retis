//! Internal representation of events. Those events can be marshaled/unmarshaled
//! to other formats to be stored or displayed. We currently support: JSON.
//!
//! As an example, a full JSON output should look like:
//!
//! {
//!     "version": "0.1.0",
//!     "hostname": "mymachine",
//!     "kernel": "6.0.8-300.fc37.x86_64",
//!     "events": [
//!         {
//!              "common": {
//!                  "symbol": "kfree_skb_reason",
//!                  "timestamp": "7322460997041"
//!              },
//!              "skb_tracking": {
//!                  "timestamp": "7322460997041",
//!                  "orig_head": "18446623346735780864",
//!                  "skb": "18446623349161350912",
//!                  "drop_reason": "0",
//!              },
//!              "skb": {
//!                  "etype": "34525"
//!              },
//!              "ovs": {
//!                  "ovs": "2.5.90",
//!                  "foo": "bar"
//!              }
//!         },
//!         ...
//!     ]
//! }

#![allow(dead_code)] // FIXME
#![allow(clippy::wrong_self_convention)]

use std::{any::Any, collections::HashMap, fmt};

use anyhow::{anyhow, bail, Result};
use log::debug;
use once_cell::sync::OnceCell;

use crate::{display::*, *};

/// Full event. Internal representation. The first key is the collector from
/// which the event sections originate. The second one is the field name of a
/// given (collector) event field.
#[derive(Default)]
pub struct Event(HashMap<SectionId, Box<dyn EventSection>>);

impl Event {
    pub fn new() -> Event {
        Event::default()
    }

    pub fn from_json(line: String) -> Result<Event> {
        let mut event = Event::new();

        let mut event_js: HashMap<String, serde_json::Value> = serde_json::from_str(line.as_str())
            .map_err(|e| anyhow!("Failed to parse json event at line {line}: {e}"))?;

        for (owner, value) in event_js.drain() {
            let parser = event_sections()?
                .get(&owner)
                .ok_or_else(|| anyhow!("json contains an unsupported event {}", owner))?;

            debug!("Unmarshaling event section {owner}: {value}");
            let section = parser(value).map_err(|e| {
                anyhow!("Failed to create EventSection for owner {owner} from json: {e}")
            })?;
            event.insert_section(SectionId::from_u8(section.section_id())?, section)?;
        }
        Ok(event)
    }

    /// Insert a new event field into an event.
    pub fn insert_section(
        &mut self,
        owner: SectionId,
        section: Box<dyn EventSection>,
    ) -> Result<()> {
        if self.0.contains_key(&owner) {
            bail!("Section for {} already found in the event", owner);
        }

        self.0.insert(owner, section);
        Ok(())
    }

    /// Get a reference to an event field by its owner and key.
    pub fn get_section<T: EventSection + 'static>(&self, owner: SectionId) -> Option<&T> {
        match self.0.get(&owner) {
            Some(section) => section.as_any().downcast_ref::<T>(),
            None => None,
        }
    }

    /// Get a reference to an event field by its owner and key.
    pub fn get_section_mut<T: EventSection + 'static>(
        &mut self,
        owner: SectionId,
    ) -> Option<&mut T> {
        match self.0.get_mut(&owner) {
            Some(section) => section.as_any_mut().downcast_mut::<T>(),
            None => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut event = serde_json::Map::new();

        for (owner, section) in self.0.iter() {
            event.insert(owner.to_str().to_string(), section.to_json());
        }

        serde_json::Value::Object(event)
    }
}

impl EventFmt for Event {
    fn event_fmt(&self, f: &mut Formatter, format: &DisplayFormat) -> std::fmt::Result {
        // First format the first event line starting with the always-there
        // {common} section, followed by the {kernel} or {user} one.
        self.0
            .get(&SectionId::Common)
            .unwrap()
            .event_fmt(f, format)?;
        if let Some(kernel) = self.0.get(&SectionId::Kernel) {
            write!(f, " ")?;
            kernel.event_fmt(f, format)?;
        } else if let Some(user) = self.0.get(&SectionId::Userspace) {
            write!(f, " ")?;
            user.event_fmt(f, format)?;
        }

        // If we do have tracking and/or drop sections, put them there too.
        // Special case the global tracking information from here for now.
        if let Some(tracking) = self.0.get(&SectionId::Tracking) {
            write!(f, " ")?;
            tracking.event_fmt(f, format)?;
        } else if let Some(skb_tracking) = self.0.get(&SectionId::SkbTracking) {
            write!(f, " ")?;
            skb_tracking.event_fmt(f, format)?;
        }
        if let Some(skb_drop) = self.0.get(&SectionId::SkbDrop) {
            write!(f, " ")?;
            skb_drop.event_fmt(f, format)?;
        }

        // Separator between each following sections.
        let sep = if format.multiline { '\n' } else { ' ' };

        // If we have a stack trace, show it.
        if let Some(kernel) = self.get_section::<KernelEvent>(SectionId::Kernel) {
            if let Some(stack) = &kernel.stack_trace {
                f.conf.inc_level(4);
                write!(f, "{sep}")?;
                stack.event_fmt(f, format)?;
                f.conf.reset_level();
            }
        }

        f.conf.inc_level(2);

        // Finally show all sections.
        (SectionId::Skb as u8..SectionId::_MAX as u8)
            .collect::<Vec<u8>>()
            .iter()
            .filter_map(|id| self.0.get(&SectionId::from_u8(*id).unwrap()))
            .try_for_each(|section| {
                write!(f, "{sep}")?;
                section.event_fmt(f, format)
            })?;

        f.conf.reset_level();
        Ok(())
    }
}

/// List of unique event sections owners.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SectionId {
    Common = 1,
    Kernel = 2,
    Userspace = 3,
    Tracking = 4,
    SkbTracking = 5,
    SkbDrop = 6,
    Skb = 7,
    Ovs = 8,
    Nft = 9,
    Ct = 10,
    Startup = 11,
    // TODO: use std::mem::variant_count once in stable.
    _MAX = 12,
}

impl SectionId {
    /// Constructs an SectionId from a section unique identifier
    pub fn from_u8(val: u8) -> Result<SectionId> {
        use SectionId::*;
        Ok(match val {
            1 => Common,
            2 => Kernel,
            3 => Userspace,
            4 => Tracking,
            5 => SkbTracking,
            6 => SkbDrop,
            7 => Skb,
            8 => Ovs,
            9 => Nft,
            10 => Ct,
            11 => Startup,
            x => bail!("Can't construct a SectionId from {}", x),
        })
    }

    /// Converts an SectionId to a section unique str identifier.
    pub fn to_str(self) -> &'static str {
        use SectionId::*;
        match self {
            Common => "common",
            Kernel => "kernel",
            Userspace => "userspace",
            Tracking => "tracking",
            SkbTracking => "skb-tracking",
            SkbDrop => "skb-drop",
            Skb => "skb",
            Ovs => "ovs",
            Nft => "nft",
            Ct => "ct",
            Startup => "startup",
            _MAX => "_max",
        }
    }
}

// Allow using SectionId in log messages.
impl fmt::Display for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

type EventSectionMap = HashMap<String, fn(serde_json::Value) -> Result<Box<dyn EventSection>>>;
static EVENT_SECTIONS: OnceCell<EventSectionMap> = OnceCell::new();

macro_rules! insert_section {
    ($events: expr, $ty: ty) => {
        $events.insert(
            SectionId::from_u8(<$ty>::SECTION_ID)?.to_str().to_string(),
            |v| Ok(Box::new(serde_json::from_value::<$ty>(v)?)),
        );
    };
}

fn event_sections() -> Result<&'static EventSectionMap> {
    EVENT_SECTIONS.get_or_try_init(|| {
        let mut events = EventSectionMap::new();

        insert_section!(events, CommonEvent);
        insert_section!(events, KernelEvent);
        insert_section!(events, UserEvent);
        insert_section!(events, SkbTrackingEvent);
        insert_section!(events, SkbDropEvent);
        insert_section!(events, SkbEvent);
        insert_section!(events, OvsEvent);
        insert_section!(events, NftEvent);
        insert_section!(events, CtEvent);
        insert_section!(events, StartupEvent);

        Ok(events)
    })
}

/// The return value of EventFactory::next_event()
pub enum EventResult {
    /// The Factory was able to create a new event.
    Event(Event),
    /// The source has been consumed.
    Eof,
    /// The timeout went off but a new attempt to retrieve an event might succeed.
    Timeout,
}

/// Per-module event section, should map 1:1 with a SectionId. Requiring specific
/// traits to be implemented helps handling those sections in the core directly
/// without requiring all modules to serialize and deserialize their events by
/// hand (except for the special case of BPF section events as there is an n:1
/// mapping there).
///
/// Please use `#[retis_derive::event_section]` to implement the common traits.
///
/// The underlying objects are free to hold their data in any way, although
/// having a proper structure is encouraged as it allows easier consumption at
/// post-processing. Those objects can also define their own specialized
/// helpers.
pub trait EventSection: EventSectionInternal + for<'a> EventDisplay<'a> {}
impl<T> EventSection for T where T: EventSectionInternal + for<'a> EventDisplay<'a> {}

/// EventSection helpers defined in the core for all events. Common definition
/// needs Sized but that is a requirement for all EventSection.
///
/// There should not be a need to have per-object implementations for this.
pub trait EventSectionInternal {
    fn section_id(&self) -> u8;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn to_json(&self) -> serde_json::Value;
}

// We need this as the value given as the input when deserializing something
// into an event could be mapped to (), e.g. serde_json::Value::Null.
impl EventSectionInternal for () {
    fn section_id(&self) -> u8 {
        SectionId::_MAX as u8
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}
