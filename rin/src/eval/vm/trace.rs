use super::*;

impl Runner<'_, '_> {
    #[cold]
    #[inline(never)]
    pub(super) fn record_unwind_frame(
        &self,
        err: &mut Error,
        entry: &FrameSource<'_>,
        tail_source: Option<&FrameSource<'_>>,
        tail_calls: usize,
        origin: Option<CallOrigin<'_>>,
        caller_module_id: Option<u32>,
    ) {
        let active_mod = match tail_source.unwrap_or(entry) {
            FrameSource::Proto(tp) => tp.module,
            FrameSource::Top(_) => self.active_module,
        };
        err.resolve_module(active_mod.map(|id| self.modules.name(id).to_owned()));
        let (Some(orig), FrameSource::Proto(proto)) = (origin, entry) else {
            return;
        };
        let module = proto.module.map(|id| self.modules.name(id).to_owned());
        let caller_module = caller_module_id.map(|id| self.modules.name(id).to_owned());
        let (tail_target_module, tail_target_name) = if tail_calls > 0
            && let Some(FrameSource::Proto(tp)) = tail_source
        {
            (
                tp.module.map(|id| self.modules.name(id).to_owned()),
                tp.name.clone(),
            )
        } else {
            (None, None)
        };
        let (at, from_host, via_builtin) = match orig {
            CallOrigin::Rin(s) => (s, false, None),
            CallOrigin::Host => (Span::default(), true, None),
            CallOrigin::Builtin { name, at } => (at, false, Some(name.to_string())),
        };
        err.push_frame(StackFrame {
            module,
            name: proto.name.clone(),
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
