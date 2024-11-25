//! # Collect
//!
//! Collect is a dynamic CLI subcommand that allows collectors to register their arguments.

use std::{any::Any, path::PathBuf};

use anyhow::Result;
use clap::{
    error::Error as ClapError,
    {builder::PossibleValuesParser, error::ErrorKind, ArgMatches, Args, Command},
};

use super::CollectRunner;
use crate::{
    cli::{dynamic::DynamicCommand, SubCommand, *},
    collect::collector::*,
    events::SectionId,
};

#[derive(Args, Debug, Default)]
pub(crate) struct CollectArgs {
    #[arg(
        long,
        default_value = "false",
        help = "Include stack traces in the kernel events. The stack entries are limited and
not released. If exhausted, no stack trace will be included."
    )]
    pub(super) stack: bool,
    #[arg(
        long,
        help = "Execute a command and terminate the collection once done."
    )]
    pub(super) cmd: Option<String>,
    // Some of the options that we want for this arg are not available in clap's derive interface
    // so both the argument definition and the field population will be done manually.
    #[arg(
        short,
        long,
        value_parser=PossibleValuesParser::new([
            "skb-tracking", "skb", "skb-drop", "ovs", "nft", "ct",
        ]),
        value_delimiter=',',
        default_value="skb-tracking,skb,skb-drop,ovs,nft,ct",
        help = "Comma-separated list of collectors to enable. When not specified default to
auto-mode (all collectors are enabled unless a prerequisite is missing)."
    )]
    pub(super) collectors: Vec<String>,
    // Use the plural in the struct but singular for the cli parameter as we're
    // dealing with a list here.
    #[arg(
        id = "probe",
        short,
        long,
        help = "Add a probe on the given target. Can be used multiple times. Probes should
follow the [TYPE:]TARGET pattern.

When TYPE is not specified it is set to 'kprobe', except if a single ':' is found in TARGET
in which case 'raw_tracepoint' is set instead. Those default types might evolve over time.

Valid TYPEs:
- kprobe | k: kernel probes.
- kretprobe | kr: kernel return probes.
- raw_tracepoint | tp: kernel tracepoints.

Wildcards (*) can be used, eg. \"kprobe:tcp_*\" or \"tp:skb:*\".

Examples:
  --probe tp:skb:kfree_skb --probe kprobe:consume_skb
  --probe skb:kfree_skb --probe consume_skb"
    )]
    pub(super) probes: Vec<String>,
    #[arg(
        short,
        long,
        num_args = 0..=1,
        default_missing_value = "retis.data",
        help = "Write the events to a file rather than to sdout. If the flag is used without a file name,
defaults to \"retis.data\"."
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(
        long,
        help = "Write the events to stdout even if --out is used.",
        default_value = "false"
    )]
    pub(super) print: bool,
    #[arg(long, help = "Format used when printing an event.")]
    #[clap(value_enum, default_value_t=CliDisplayFormat::MultiLine)]
    pub(super) format: CliDisplayFormat,
    #[arg(long, help = "Print the time as UTC")]
    pub(super) utc: bool,
    #[arg(
        id = "filter-packet",
        short,
        long,
        help = r#"Add a packet filter to all targets. The syntax follows the structure of pcap-filer(7).

Example: --filter-packet "ip dst host 10.0.0.1""#
    )]
    pub(super) packet_filter: Option<String>,
    #[arg(
        id = "filter-meta",
        short = 'm',
        long,
        help = r#"Add a meta filter to all targets. A meta filter compares a field within a kernel structure against a user-provided input.
The syntax follows:
sk_buff.member1.[...].memberN.member_leaf [==|<=|>=|!=] value
With value ::= "string" | number.
"==" is the only operator valid for "string" assuming member_leaf type is a pointer to a char or array of chars.

Examples of meta filters:
--filter-meta 'sk_buff.dev.name == "eth0"'
--filter-meta 'sk_buff.dev.nd_net.net.ns.inum == 4026531840'"#
    )]
    pub(super) meta_filter: Option<String>,
    #[arg(
        long,
        default_value = "false",
        help = "When set, evaluates where Retis could add additional probes based on functions reported
in the events stack traces (their display is still controlled by --stack). All matching
functions are probed at runtime using kprobes.

Notes:
- Using a filter is required (--filter-packet and/or --filter-meta).
- If no explicit probe is given, tp:skb:kfree_skb and tp:skb:consume_skb are used as a
  starting point.
- Additional probes are added only after events including them in their stack trace are
  reported; this means the first packets hitting a probe won't be reported.
- Packets will only be followed prior to the initial set of probes (as this mode uses
  stack traces). This also means the filter must match packets as they appear in the
  initial set of probes; packet transformation can't be automatically detected."
    )]
    pub(crate) probe_stack: bool,
    #[arg(
        long,
        default_value = "false",
        help = r#"Allow the tool to setup all the system changes needed to make the tracing
fully operational:

- Mounting debugfs to /sys/kernel/debug if not already mounted. If Retis mounted debugfs it
  will unmount it when stopped.

- In the case the nft module is used, creating a dummy table called "Retis_Table"
  as the following:

    table inet Retis_Table {
        chain Retis_Chain {
            meta nftrace set 1
        }
    }

  The table will be removed once the program gets stopped. Note that the tool tries to remove
  "Retis_Table" before creating it.
"#
    )]
    pub(crate) allow_system_changes: bool,
}

#[derive(Debug)]
pub(crate) struct Collect {
    args: CollectArgs,
    collectors: DynamicCommand,
    /// Was the collector list set from the default value (aka did the user not
    /// request any specific set of collectors)?
    pub(super) default_collectors_list: bool,
}

impl SubCommand for Collect {
    fn new() -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Collect {
            args: CollectArgs::default(),
            collectors: DynamicCommand::new(
                CollectArgs::augment_args(Command::new("collect")),
                "collector",
            )?,
            default_collectors_list: true,
        })
    }

    fn name(&self) -> String {
        "collect".to_string()
    }

    fn dynamic(&self) -> Option<&DynamicCommand> {
        Some(&self.collectors)
    }

    fn dynamic_mut(&mut self) -> Option<&mut DynamicCommand> {
        Some(&mut self.collectors)
    }

    fn full(&mut self) -> Result<Command> {
        self.collectors
            .register_module::<skb::SkbCollectorArgs>(SectionId::Skb)?;
        self.collectors
            .register_module_noargs(SectionId::SkbTracking)?;
        self.collectors.register_module_noargs(SectionId::SkbDrop)?;
        self.collectors
            .register_module::<ovs::OvsCollectorArgs>(SectionId::Ovs)?;
        self.collectors
            .register_module::<nft::NftCollectorArgs>(SectionId::Nft)?;
        self.collectors.register_module_noargs(SectionId::Ct)?;

        let long_about = "Collect events using 'collectors'.\n\n \
            Collectors are modules that extract \
            events from different places of the kernel or userspace daemons \
            using ebpf."
            .to_string();

        let full_command = self
            .collectors
            .command()
            .to_owned()
            .about("Collect network events")
            .long_about(long_about);

        Ok(full_command)
    }

    fn update_from_arg_matches(&mut self, args: &ArgMatches) -> Result<(), ClapError> {
        self.collectors
            .set_matches(args)
            .map_err(|_| ClapError::new(ErrorKind::InvalidValue))?;
        self.args = self
            .collectors
            .get_main::<CollectArgs>()
            .map_err(|_| ClapError::new(ErrorKind::InvalidValue))?;

        // Manually set collectors argument.
        self.default_collectors_list = args
            .value_source("collectors")
            .ok_or_else(|| ClapError::new(ErrorKind::MissingRequiredArgument))?
            == clap::parser::ValueSource::DefaultValue;
        self.args.collectors = args
            .get_many("collectors")
            .ok_or_else(|| ClapError::new(ErrorKind::MissingRequiredArgument))?
            .map(|x: &String| x.to_owned())
            .collect();
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn runner(&self) -> Result<Box<dyn SubCommandRunner>> {
        Ok(Box::new(CollectRunner {}))
    }
}

impl Collect {
    /// Returns the main Collect arguments
    pub(crate) fn args(&self) -> Result<&CollectArgs> {
        Ok(&self.args)
    }
}
