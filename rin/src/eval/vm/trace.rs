use super::*;
use crate::error::FrameInput;

impl Runner<'_, '_> {
    #[cold]
    #[inline(never)]
    pub(super) fn record_unwind_frame(
        &mut self,
        entry: &FrameSource<'_>,
        tail_source: Option<&FrameSource<'_>>,
        tail_calls: usize,
        origin: Option<CallOrigin>,
        caller_module_id: Option<u32>,
    ) {
        let active_mod = match tail_source.unwrap_or(entry) {
            FrameSource::Proto(tp) => tp.module,
            FrameSource::Top(_) => self.active_module,
        };
        self.pool
            .core
            .error
            .resolve_module(active_mod.map(|id| self.modules.name(id)));
        let (Some(orig), FrameSource::Proto(proto)) = (origin, entry) else {
            return;
        };
        let module = proto.module.map(|id| self.modules.name(id));
        let caller_module = caller_module_id.map(|id| self.modules.name(id));
        let (tail_target_module, tail_target_name) = if tail_calls > 0
            && let Some(FrameSource::Proto(tp)) = tail_source
        {
            (
                tp.module.map(|id| self.modules.name(id)),
                tp.name.as_ref().map(|s| s.as_str()),
            )
        } else {
            (None, None)
        };
        let (at, from_host, via_builtin) = match orig {
            CallOrigin::Rin(s) => (s, false, None),
            CallOrigin::Host => (Span::default(), true, None),
            CallOrigin::Builtin { name, at } => (at, false, Some(name)),
        };
        self.pool.core.error.push_frame(FrameInput {
            module,
            name: proto.name.as_ref().map(|s| s.as_str()),
            caller_module,
            at,
            tail_calls,
            tail_target_module,
            tail_target_name,
            repeat_count: 1,
            from_host,
            via_builtin,
        });
    }
}
