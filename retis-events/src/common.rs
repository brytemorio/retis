use std::fmt;

use chrono::{DateTime, Utc};

use crate::{event_section, event_type, *};

#[event_section("md_common")]
pub struct CommonEventMd {
    pub retis_version: String,
    /// CLOCK_MONOTONIC offset in regards to local machine time.
    pub clock_monotonic_offset: TimeSpec,
}

impl EventFmt for CommonEventMd {
    fn event_fmt(&self, f: &mut fmt::Formatter, _: DisplayFormat) -> fmt::Result {
        write!(f, "Retis version {}", self.retis_version)
    }
}

#[event_type]
#[derive(Default)]
pub struct TaskEvent {
    /// Process id.
    pub pid: i32,
    /// Thread group id.
    pub tgid: i32,
    /// Name of the current task.
    pub comm: String,
}

/// Common event section.
#[event_section("common")]
pub struct CommonEvent {
    /// Timestamp of when the event was generated.
    pub timestamp: u64,
    /// SMP processor id.
    pub smp_id: u32,
    pub task: Option<TaskEvent>,
}

impl EventFmt for CommonEvent {
    fn event_fmt(&self, f: &mut fmt::Formatter, format: DisplayFormat) -> fmt::Result {
        match format.time_format {
            TimeFormat::MonotonicTimestamp => write!(f, "{}", self.timestamp)?,
            TimeFormat::UtcDate => match format.monotonic_offset {
                Some(offset) => {
                    let timestamp = TimeSpec::new(0, self.timestamp as i64) + offset;
                    let time: DateTime<Utc> = timestamp.into();
                    write!(f, "{}", time.format("%F %T.%6f"))?;
                }
                None => write!(f, "{}", self.timestamp)?,
            },
        }

        write!(f, " ({})", self.smp_id)?;

        if let Some(current) = &self.task {
            write!(f, " [{}] ", current.comm)?;
            if current.tgid != current.pid {
                write!(f, "{}/", current.pid)?;
            }
            write!(f, "{}", current.tgid)?;
        }

        Ok(())
    }
}
