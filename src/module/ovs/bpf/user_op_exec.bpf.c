#include <vmlinux.h>

#include <user_common.h>
#include <ovs_operation.h>

/* Hook for usdt:dpif_netlink_operate__::op_flow_execute. */
DEFINE_USDT_HOOK (
	struct ovs_operation_event *op;
	batch_process_op(OVS_OP_EXEC, event, &op);

	if (op) {
		uint32_t zero = 0;
		if (bpf_map_update_elem(&flow_exec_tracking, &op->queue_id, &zero,
					BPF_NOEXIST)) {
			/* The entry already existed. This means an exec operation
			 * was enqueued with the same queue_id and it was not
			 * unqueued from the kernel yet.
			 * It is likely we will have problems correlating
			 * events. TODO: report the error.*/
			return 1;
		}
	}
	return 0;
)

char __license[] SEC("license") = "GPL";
