use std::mem;

use anyhow::{anyhow, bail, Result};
use clap::{arg, Parser};

use super::hooks;
use crate::{
    cli::{dynamic::DynamicCommand, CliConfig},
    collect::Collector,
    core::{
        inspect,
        kernel::Symbol,
        probe::{user::UsdtProbe, Hook, Probe, ProbeManager},
        user::proc::{Process, ThreadInfo},
    },
    module::ModuleId,
};

#[derive(Parser, Default)]
pub(crate) struct OvsCollectorArgs {
    #[arg(
        long,
        default_value = "false",
        help = "Enable OpenvSwitch upcall tracking. Requires USDT probes being enabled.
See https://docs.openvswitch.org/en/latest/topics/usdt-probes/ for instructions."
    )]
    ovs_track: bool,
}

#[derive(Default)]
pub(crate) struct OvsCollector {
    track: bool,
    inflight_upcalls_map: Option<libbpf_rs::Map>,
    inflight_exec_map: Option<libbpf_rs::Map>,
    inflight_enqueue_map: Option<libbpf_rs::Map>,
    /* Batch tracking maps. */
    upcall_batches: Option<libbpf_rs::Map>,
    pid_to_batch: Option<libbpf_rs::Map>,
}

impl Collector for OvsCollector {
    fn new() -> Result<OvsCollector> {
        Ok(OvsCollector::default())
    }

    fn register_cli(&self, cmd: &mut DynamicCommand) -> Result<()> {
        cmd.register_module::<OvsCollectorArgs>(ModuleId::Ovs)
    }

    // Check if the OvS collector can run. Some potential errors are silenced,
    // to avoid returning an error if we can't inspect a given area for some
    // reasons.
    fn can_run(&self, _: &CliConfig) -> Result<()> {
        let inspector = inspect::inspector()?;

        // Check if the OvS kernel module is available. We also check for loaded
        // module in case CONFIG_OPENVSWITCH=n because if might be out of tree.
        if inspector.kernel.get_config_option("CONFIG_OPENVSWITCH")? != Some("=y")
            && inspector.kernel.is_module_loaded("openvswitch") == Some(false)
        {
            bail!("Kernel module 'openvswitch' is not loaded")
        }

        Ok(())
    }

    fn init(&mut self, cli: &CliConfig, probes: &mut ProbeManager) -> Result<()> {
        self.track = cli
            .get_section::<OvsCollectorArgs>(ModuleId::Ovs)?
            .ovs_track;

        self.inflight_upcalls_map = Some(Self::create_inflight_upcalls_map()?);

        if self.track {
            self.inflight_enqueue_map = Some(Self::create_inflight_enqueue_cmd_map()?);
        }

        // Add targetted hooks.
        // Upcall related hooks:
        self.add_upcall_hooks(probes)?;
        // Exec related hooks
        self.add_exec_hooks(probes)?;

        // Add USDT hooks.
        if self.track {
            self.add_usdt_hooks(probes)?;
        }
        Ok(())
    }
}

impl OvsCollector {
    fn create_inflight_enqueue_cmd_map() -> Result<libbpf_rs::Map> {
        // Please keep in sync with its C counterpart in bpf/ovs_common.h
        let opts = libbpf_sys::bpf_map_create_opts {
            sz: mem::size_of::<libbpf_sys::bpf_map_create_opts>() as libbpf_sys::size_t,
            ..Default::default()
        };

        libbpf_rs::Map::create(
            libbpf_rs::MapType::Hash,
            Some("inflight_enqueue"),
            mem::size_of::<u32>() as u32,
            mem::size_of::<u32>() as u32,
            8192,
            &opts,
        )
        .or_else(|e| bail!("Could not create the inflight_enqueue map: {}", e))
    }

    fn create_inflight_exec_map() -> Result<libbpf_rs::Map> {
        // Please keep in sync with its C counterpart in bpf/ovs_common.h
        #[repr(C, packed)]
        struct ExecuteActionsContext {
            skb: u64,
        }
        let opts = libbpf_sys::bpf_map_create_opts {
            sz: mem::size_of::<libbpf_sys::bpf_map_create_opts>() as libbpf_sys::size_t,
            ..Default::default()
        };

        libbpf_rs::Map::create(
            libbpf_rs::MapType::Hash,
            Some("inflight_exec"),
            mem::size_of::<u64>() as u32,
            mem::size_of::<ExecuteActionsContext>() as u32,
            50,
            &opts,
        )
        .or_else(|e| bail!("Could not create the inflight_exec map: {}", e))
    }

    fn create_inflight_upcalls_map() -> Result<libbpf_rs::Map> {
        // Please keep in sync with its C counterpart in bpf/ovs_common.h
        #[repr(C, packed)]
        struct UpcallContext {
            ts: u64,
            cpu: u32,
        }
        let opts = libbpf_sys::bpf_map_create_opts {
            sz: mem::size_of::<libbpf_sys::bpf_map_create_opts>() as libbpf_sys::size_t,
            ..Default::default()
        };

        libbpf_rs::Map::create(
            libbpf_rs::MapType::Hash,
            Some("inflight_upcalls"),
            mem::size_of::<u64>() as u32,
            mem::size_of::<UpcallContext>() as u32,
            50,
            &opts,
        )
        .or_else(|e| bail!("Could not create the inflight_upcalls config map: {}", e))
    }

    // Returns the upcall_batches array and the pid_to_batch hash.
    fn create_batch_maps(&mut self, ovs: &Process) -> Result<()> {
        let ovs_threads = ovs.thread_info()?;
        let handlers: Vec<&ThreadInfo> = ovs_threads
            .iter()
            .filter(|t| t.comm.contains("handler"))
            .collect();
        let nhandlers = handlers.len();

        // Please keep in sync with its C counterpart in bpf/ovs_operation.h
        #[repr(C, packed)]
        struct UserUpcallInfo {
            queue_id: u32,
            process_ops: u8,
            skip_event: u8,
        }
        #[repr(C, packed)]
        struct UpcallBatch {
            leater_ts: u64,
            processing: bool,
            current_upcall: u8,
            total: u8,
            upcalls: [UserUpcallInfo; 64],
        }

        let opts = libbpf_sys::bpf_map_create_opts {
            sz: mem::size_of::<libbpf_sys::bpf_map_create_opts>() as libbpf_sys::size_t,
            ..Default::default()
        };

        self.upcall_batches = Some(
            libbpf_rs::Map::create(
                libbpf_rs::MapType::Array,
                Some("upcall_batches"),
                mem::size_of::<u32>() as u32,
                mem::size_of::<UpcallBatch>() as u32,
                nhandlers as u32,
                &opts,
            )
            .or_else(|e| bail!("Could not create the upcall_batches map: {}", e))?,
        );

        let opts = libbpf_sys::bpf_map_create_opts {
            sz: mem::size_of::<libbpf_sys::bpf_map_create_opts>() as libbpf_sys::size_t,
            ..Default::default()
        };

        self.pid_to_batch = Some(
            libbpf_rs::Map::create(
                libbpf_rs::MapType::Hash,
                Some("pid_to_batch"),
                mem::size_of::<u32>() as u32,
                mem::size_of::<u32>() as u32,
                nhandlers as u32,
                &opts,
            )
            .or_else(|e| bail!("Could not create the upcall_batches map: {}", e))?,
        );

        /* Populate pid_to_batch map. */
        for (batch_idx, handler) in (0_u32..).zip(handlers.iter().as_ref().iter()) {
            self.pid_to_batch.as_mut().unwrap().update(
                &handler.pid.to_ne_bytes(),
                &batch_idx.to_ne_bytes(),
                libbpf_rs::MapFlags::NO_EXIST,
            )?;
        }
        Ok(())
    }

    /// Add upcall hooks.
    fn add_upcall_hooks(&self, probes: &mut ProbeManager) -> Result<()> {
        let inflight_upcalls_map = self
            .inflight_upcalls_map
            .as_ref()
            .ok_or_else(|| anyhow!("Inflight upcalls map not created"))?
            .fd();

        // Upcall probe.
        let mut kernel_upcall_tp_hook = Hook::from(hooks::kernel_upcall_tp::DATA);
        kernel_upcall_tp_hook.reuse_map("inflight_upcalls", inflight_upcalls_map)?;
        let mut probe = Probe::raw_tracepoint(Symbol::from_name("openvswitch:ovs_dp_upcall")?)?;
        probe.add_hook(kernel_upcall_tp_hook)?;
        probes.register_probe(probe)?;

        // Upcall return probe.
        let mut kernel_upcall_ret_hook = Hook::from(hooks::kernel_upcall_ret::DATA);
        kernel_upcall_ret_hook.reuse_map("inflight_upcalls", inflight_upcalls_map)?;
        let mut probe = Probe::kretprobe(Symbol::from_name("ovs_dp_upcall")?)?;
        probe.add_hook(kernel_upcall_ret_hook)?;
        probes.register_probe(probe)?;

        if self.track {
            // Upcall enqueue.
            let mut kernel_enqueue_hook = Hook::from(hooks::kernel_enqueue::DATA);
            kernel_enqueue_hook.reuse_map("inflight_upcalls", inflight_upcalls_map)?;
            kernel_enqueue_hook.reuse_map(
                "inflight_enqueue",
                self.inflight_enqueue_map
                    .as_ref()
                    .ok_or_else(|| anyhow!("inflight enqueue map not created"))?
                    .fd(),
            )?;
            let mut probe = Probe::kretprobe(Symbol::from_name("queue_userspace_packet")?)?;
            probe.add_hook(kernel_enqueue_hook)?;
            probes.register_probe(probe)?;
        }

        Ok(())
    }

    /// Add exec hooks.
    fn add_exec_hooks(&mut self, probes: &mut ProbeManager) -> Result<()> {
        let mut exec_action_hook = Hook::from(hooks::kernel_exec_tp::DATA);

        if self.track {
            let inflight_exec_map = Self::create_inflight_exec_map()?;

            exec_action_hook.reuse_map("inflight_exec", inflight_exec_map.fd())?;

            let mut exec_cmd_hook = Hook::from(hooks::kernel_exec_actions::DATA);
            let ovs_execute_actions_sym = Symbol::from_name("ovs_execute_actions")?;
            exec_cmd_hook.reuse_map("inflight_exec", inflight_exec_map.fd())?;
            let mut probe = Probe::kprobe(ovs_execute_actions_sym.clone())?;
            probe.add_hook(exec_cmd_hook)?;
            probes.register_probe(probe)?;

            let mut exec_cmd_ret_hook = Hook::from(hooks::kernel_exec_actions_ret::DATA);
            exec_cmd_ret_hook.reuse_map("inflight_exec", inflight_exec_map.fd())?;
            let mut probe = Probe::kprobe(ovs_execute_actions_sym)?;
            probe.add_hook(exec_cmd_ret_hook)?;
            probes.register_probe(probe)?;

            self.inflight_exec_map = Some(inflight_exec_map);
        }

        // Action execute probe.
        let mut probe =
            Probe::raw_tracepoint(Symbol::from_name("openvswitch:ovs_do_execute_action")?)?;
        probe.add_hook(exec_action_hook)?;
        probes.register_probe(probe)?;

        Ok(())
    }

    /// Add USDT hooks.
    fn add_usdt_hooks(&mut self, probes: &mut ProbeManager) -> Result<()> {
        let ovs = Process::from_cmd("ovs-vswitchd")?;
        if !ovs.is_usdt("main::run_start")? {
            bail!(
                "Cannot find USDT probes in ovs-vswitchd. Was it built with --enable-usdt-probes?"
            );
        }
        self.create_batch_maps(&ovs)?;
        let upcall_batches_fd = self
            .upcall_batches
            .as_ref()
            .ok_or_else(|| anyhow!("upcall batches map not created"))?
            .fd();
        let pid_to_batch_fd = self
            .pid_to_batch
            .as_ref()
            .ok_or_else(|| anyhow!("pid_to_batch map not created"))?
            .fd();

        let mut user_recv_hook = Hook::from(hooks::user_recv_upcall::DATA);
        user_recv_hook.reuse_map(
            "inflight_enqueue",
            self.inflight_enqueue_map
                .as_ref()
                .ok_or_else(|| anyhow!("inflight enqueue map not created"))?
                .fd(),
        )?;

        let mut batch_probes = vec![
            (
                Probe::usdt(UsdtProbe::new(&ovs, "dpif_recv::recv_upcall")?)?,
                user_recv_hook,
            ),
            (
                Probe::usdt(UsdtProbe::new(
                    &ovs,
                    "dpif_netlink_operate__::op_flow_execute",
                )?)?,
                Hook::from(hooks::user_op_exec::DATA),
            ),
            (
                Probe::usdt(UsdtProbe::new(&ovs, "dpif_netlink_operate__::op_flow_put")?)?,
                Hook::from(hooks::user_op_put::DATA),
            ),
        ];

        while let Some((mut probe, mut hook)) = batch_probes.pop() {
            hook.reuse_map("upcall_batches", upcall_batches_fd)?
                .reuse_map("pid_to_batch", pid_to_batch_fd)?;
            probe.add_hook(hook)?;
            probes.register_probe(probe)?;
        }
        Ok(())
    }
}
